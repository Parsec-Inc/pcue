use std::ptr::null_mut;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;

use anyhow::{Result, bail};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::RemoteDesktop::{NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DBT_DEVICEARRIVAL, DBT_DEVICEREMOVECOMPLETE, DBT_DEVTYP_DEVICEINTERFACE,
    DEV_BROADCAST_DEVICEINTERFACE_W, DEV_BROADCAST_HDR, DEVICE_NOTIFY_WINDOW_HANDLE, DefWindowProcW, DispatchMessageW,
    GetMessageW, MSG, PBT_APMRESUMEAUTOMATIC, RegisterClassW, RegisterDeviceNotificationW, TranslateMessage,
    WM_DEVICECHANGE, WM_DISPLAYCHANGE, WM_POWERBROADCAST, WM_WTSSESSION_CHANGE, WNDCLASSW, WTS_SESSION_LOCK,
    WTS_SESSION_UNLOCK,
};
use windows_sys::core::GUID;

use super::win::{monitors, wide};
use crate::constants::USB_DEVICE_INTERFACE;
use crate::platform::{Device, Event, EventSource, Notification};

/// Destino dos eventos da janela oculta.
static EVENTS: Mutex<Option<Sender<Event>>> = Mutex::new(None);
/// Monitores conectados na última mudança de tela.
static MONITORS: AtomicUsize = AtomicUsize::new(0);

/// Bloqueio, desbloqueio, retorno da suspensão e monitores.
pub fn event_sources() -> Vec<EventSource> {
    vec![("sessão do Windows", watch_session)]
}

/// Janela oculta que recebe as mensagens de sessão, energia e tela.
fn watch_session(sender: Sender<Event>) -> Result<()> {
    *EVENTS.lock().expect("trava dos eventos") = Some(sender);
    MONITORS.store(monitors().len(), Ordering::SeqCst);
    let class = wide("pcue-events");
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let mut window_class: WNDCLASSW = std::mem::zeroed();
        window_class.lpfnWndProc = Some(window_proc);
        window_class.hInstance = instance;
        window_class.lpszClassName = class.as_ptr();
        RegisterClassW(&window_class);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            bail!("não foi possível criar a janela de eventos");
        }
        if WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) == 0 {
            crate::utils::log("sem aviso de bloqueio da sessão: lock/unlock indisponíveis");
        }
        let mut filter: DEV_BROADCAST_DEVICEINTERFACE_W = std::mem::zeroed();
        filter.dbcc_size = std::mem::size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>() as u32;
        filter.dbcc_devicetype = DBT_DEVTYP_DEVICEINTERFACE;
        filter.dbcc_classguid = GUID::from_u128(USB_DEVICE_INTERFACE);
        if RegisterDeviceNotificationW(hwnd, (&raw const filter).cast(), DEVICE_NOTIFY_WINDOW_HANDLE).is_null() {
            crate::utils::log("sem aviso de dispositivos USB");
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    bail!("o laço de mensagens terminou")
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_DEVICECHANGE
        && let Some(device) = unsafe { usb_change(wparam, lparam) }
    {
        send(Event::Device(device));
    }
    let event = match msg {
        WM_WTSSESSION_CHANGE => match wparam as u32 {
            WTS_SESSION_LOCK => Some("lock"),
            WTS_SESSION_UNLOCK => Some("unlock"),
            _ => None,
        },
        WM_POWERBROADCAST if wparam as u32 == PBT_APMRESUMEAUTOMATIC => Some("resume"),
        WM_DISPLAYCHANGE => {
            let now = monitors().len();
            let before = MONITORS.swap(now, Ordering::SeqCst);
            match now.cmp(&before) {
                std::cmp::Ordering::Greater => Some("monitor_added"),
                std::cmp::Ordering::Less => Some("monitor_removed"),
                std::cmp::Ordering::Equal => None,
            }
        }
        _ => None,
    };
    if let Some(event) = event {
        send(Event::Named(event.to_string()));
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn send(event: Event) {
    if let Ok(events) = EVENTS.lock()
        && let Some(sender) = events.as_ref()
    {
        let _ = sender.send(event);
    }
}

/// Dispositivo USB do `WM_DEVICECHANGE`; o nome amigável não vem, então nome = id.
unsafe fn usb_change(wparam: WPARAM, lparam: LPARAM) -> Option<Device> {
    let removed = match wparam as u32 {
        DBT_DEVICEARRIVAL => false,
        DBT_DEVICEREMOVECOMPLETE => true,
        _ => return None,
    };
    let header = lparam as *const DEV_BROADCAST_HDR;
    if header.is_null() || unsafe { (*header).dbch_devicetype } != DBT_DEVTYP_DEVICEINTERFACE {
        return None;
    }
    let interface = lparam as *const DEV_BROADCAST_DEVICEINTERFACE_W;
    let name = unsafe { (&raw const (*interface).dbcc_name).cast::<u16>() };
    let len = (0..).take_while(|&i| unsafe { *name.add(i) } != 0).count();
    let path = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) }).to_ascii_lowercase();
    let hex = |key: &str| path.split(['#', '&']).find_map(|part| part.strip_prefix(key)).map(str::to_string);
    let id = format!("{}:{}", hex("vid_")?, hex("pid_")?);
    Some(Device { name: id.clone(), id, removed })
}

/// Captura de dispositivo só pela janela do daemon no Windows.
pub fn watch_devices(_on: impl FnMut(Device) -> bool) -> Result<()> {
    bail!("captura de dispositivo só funciona no Linux por enquanto")
}

/// Notificações de apps ainda não são lidas no Windows.
pub fn watch_notifications(_on: impl FnMut(Notification) -> bool) -> Result<()> {
    bail!("gatilho por notificação só funciona no Linux por enquanto")
}
