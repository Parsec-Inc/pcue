use std::ptr::null_mut;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;

use anyhow::{Result, bail};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::RemoteDesktop::{NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, PBT_APMRESUMEAUTOMATIC, RegisterClassW,
    TranslateMessage, WM_DISPLAYCHANGE, WM_POWERBROADCAST, WM_WTSSESSION_CHANGE, WNDCLASSW, WTS_SESSION_LOCK,
    WTS_SESSION_UNLOCK,
};

use super::win::{monitors, wide};
use crate::platform::{Event, EventSource, Notification};

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
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    bail!("o laço de mensagens terminou")
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
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
    if let Some(event) = event
        && let Ok(events) = EVENTS.lock()
        && let Some(sender) = events.as_ref()
    {
        let _ = sender.send(Event::Named(event.to_string()));
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Notificações de apps ainda não são lidas no Windows.
pub fn watch_notifications(_on: impl FnMut(Notification) -> bool) -> Result<()> {
    bail!("gatilho por notificação só funciona no Linux por enquanto")
}
