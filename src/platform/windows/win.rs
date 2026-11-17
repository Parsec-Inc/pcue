//! Chamadas Win32 usadas pelo resto do módulo, com o `unsafe` concentrado aqui.

use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use anyhow::{Context, Result, bail};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HWND, LPARAM, POINT, RECT, STILL_ACTIVE};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows_sys::Win32::System::Console::GetConsoleWindow;
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_MENU,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, FindWindowW, GW_OWNER, GWL_EXSTYLE, GetClassNameW, GetCursorPos,
    GetForegroundWindow, GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, IsZoomed, SW_MAXIMIZE, SW_RESTORE, SWP_NOACTIVATE,
    SWP_NOSIZE, SWP_NOZORDER, SetCursorPos, SetForegroundWindow, SetWindowPos, ShowWindow, WS_EX_TOOLWINDOW,
};

use crate::platform::Window;

/// Monitor com a área total e a área útil (sem a barra de tarefas).
#[derive(Clone, Copy)]
pub struct Monitor {
    handle: HMONITOR,
    work: RECT,
}

impl Monitor {
    pub fn center(&self) -> (i32, i32) {
        ((self.work.left + self.work.right) / 2, (self.work.top + self.work.bottom) / 2)
    }
}

/// Texto UTF-16 terminado em zero.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Identificador textual da janela.
pub fn window_id(hwnd: HWND) -> String {
    format!("{:x}", hwnd as usize)
}

/// Volta do identificador textual para o handle.
pub fn parse_id(id: &str) -> Result<HWND> {
    let raw = usize::from_str_radix(id, 16).with_context(|| format!("id de janela inválido: {id}"))?;
    Ok(raw as HWND)
}

/// Janelas de topo em ordem z (a da frente primeiro).
pub fn top_level_windows() -> Vec<HWND> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> i32 {
        let list = unsafe { &mut *(lparam as *mut Vec<HWND>) };
        list.push(hwnd);
        1
    }
    let mut list: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(collect), &mut list as *mut Vec<HWND> as LPARAM) };
    list
}

/// Janela que aparece no Alt+Tab: visível, sem dona, com título e não escondida pelo DWM.
pub fn is_app_window(hwnd: HWND) -> bool {
    unsafe {
        if IsWindowVisible(hwnd) == 0 || !GetWindow(hwnd, GW_OWNER).is_null() {
            return false;
        }
        if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 {
            return false;
        }
        let mut cloaked: u32 = 0;
        let ok = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as u32,
            &mut cloaked as *mut u32 as *mut c_void,
            size_of::<u32>() as u32,
        );
        if ok == 0 && cloaked != 0 {
            return false;
        }
        GetWindowTextLengthW(hwnd) > 0
    }
}

/// Executável e criação do processo, guardados por PID.
type ProcessInfo = (String, u64);

/// Converte o handle para o formato comum.
pub fn describe(hwnd: HWND, rank: i32, monitors: &[Monitor], cache: &mut HashMap<u32, ProcessInfo>) -> Window {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    let (path, created) = cache
        .entry(pid)
        .or_insert_with(|| (process_path(pid).unwrap_or_default(), process_created(pid)))
        .clone();
    let class = Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut rect: RECT = unsafe { zeroed() };
    unsafe { GetWindowRect(hwnd, &mut rect) };
    Window {
        id: window_id(hwnd),
        class,
        alt_class: class_name(hwnd),
        title: title(hwnd),
        workspace: monitor_number(hwnd, monitors).to_string(),
        at: [rect.left, rect.top],
        size: [rect.right - rect.left, rect.bottom - rect.top],
        pid: i64::from(pid),
        opened: (created, hwnd as usize as u64),
        focus_rank: rank,
        on_screen: unsafe { IsIconic(hwnd) } == 0,
    }
}

use std::path::Path;

fn title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let len = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

/// Caminho completo do executável do processo.
pub fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Instante de criação do processo, para ordenar janelas por abertura.
fn process_created(pid: u32) -> u64 {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return u64::MAX;
        }
        let (mut created, mut exited, mut kernel, mut user): (FILETIME, FILETIME, FILETIME, FILETIME) =
            (zeroed(), zeroed(), zeroed(), zeroed());
        let ok = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user);
        CloseHandle(handle);
        if ok == 0 {
            return u64::MAX;
        }
        (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime)
    }
}

/// O processo ainda roda?
pub fn process_alive(pid: u32) -> bool {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE as u32
    }
}

/// Encerra o processo.
pub fn terminate(pid: u32) -> Result<()> {
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            bail!("processo {pid} não encontrado");
        }
        let ok = TerminateProcess(handle, 1);
        CloseHandle(handle);
        anyhow::ensure!(ok != 0, "não foi possível encerrar o processo {pid}");
    }
    Ok(())
}

/// Monitores ordenados da esquerda para a direita (depois de cima para baixo).
pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn collect(handle: HMONITOR, _: HDC, _: *mut RECT, lparam: LPARAM) -> i32 {
        let list = unsafe { &mut *(lparam as *mut Vec<(Monitor, RECT)>) };
        let mut info: MONITORINFO = unsafe { zeroed() };
        info.cbSize = size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(handle, &mut info) } != 0 {
            list.push((Monitor { handle, work: info.rcWork }, info.rcMonitor));
        }
        1
    }
    let mut list: Vec<(Monitor, RECT)> = Vec::new();
    unsafe {
        EnumDisplayMonitors(null_mut(), null(), Some(collect), &mut list as *mut Vec<(Monitor, RECT)> as LPARAM)
    };
    list.sort_by_key(|(_, area)| (area.left, area.top));
    list.into_iter().map(|(monitor, _)| monitor).collect()
}

/// Número (1, 2...) do monitor onde a janela está.
pub fn monitor_number(hwnd: HWND, monitors: &[Monitor]) -> usize {
    let handle = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    monitors.iter().position(|m| m.handle == handle).map_or(1, |i| i + 1)
}

/// Move a janela para o monitor, na mesma posição relativa à área útil.
pub fn move_to_monitor(hwnd: HWND, target: &Monitor) {
    unsafe {
        let maximized = IsZoomed(hwnd) != 0;
        if maximized {
            ShowWindow(hwnd, SW_RESTORE);
        }
        let mut rect: RECT = zeroed();
        GetWindowRect(hwnd, &mut rect);
        let source = monitors()
            .into_iter()
            .find(|m| m.handle == MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST));
        let (dx, dy) = source.map_or((0, 0), |s| (rect.left - s.work.left, rect.top - s.work.top));
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        let x = (target.work.left + dx).min(target.work.right - width).max(target.work.left);
        let y = (target.work.top + dy).min(target.work.bottom - height).max(target.work.top);
        SetWindowPos(hwnd, null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        if maximized {
            ShowWindow(hwnd, SW_MAXIMIZE);
        }
    }
}

/// Janela em primeiro plano.
pub fn foreground() -> HWND {
    unsafe { GetForegroundWindow() }
}

/// Traz a janela para frente; o toque no Alt libera o `SetForegroundWindow`.
pub fn force_foreground(hwnd: HWND) -> Result<()> {
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        let alt = |flags| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_MENU,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let inputs = [alt(0), alt(KEYEVENTF_KEYUP)];
        SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32);
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
    anyhow::ensure!(foreground() == hwnd, "o Windows não deixou focar a janela");
    Ok(())
}

pub fn cursor() -> Result<(i32, i32)> {
    let mut point = POINT { x: 0, y: 0 };
    anyhow::ensure!(unsafe { GetCursorPos(&mut point) } != 0, "posição do cursor indisponível");
    Ok((point.x, point.y))
}

pub fn set_cursor(x: i32, y: i32) -> Result<()> {
    anyhow::ensure!(unsafe { SetCursorPos(x, y) } != 0, "não foi possível mover o cursor");
    Ok(())
}

/// A barra de tarefas do Explorer já existe?
pub fn taskbar_ready() -> bool {
    let class = wide("Shell_TrayWnd");
    !unsafe { FindWindowW(class.as_ptr(), null()) }.is_null()
}

/// O processo tem uma janela de console visível?
pub fn has_console_window() -> bool {
    !unsafe { GetConsoleWindow() }.is_null()
}
