use std::cell::Cell;
use std::collections::HashSet;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{Result, bail};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLMHF_INJECTED, MSG,
    MSLLHOOKSTRUCT, PostQuitMessage, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WHEEL_DELTA, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEWHEEL,
    WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SYSKEYDOWN,
};

use crate::keys::Key;
use crate::platform::RawInput;
use crate::script::Button;

type Event = (Instant, RawInput);

/// Destino da gravação atual; só a thread da geração vigente envia.
static SINK: Mutex<Option<Sender<Event>>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static MY_GENERATION: Cell<u64> = const { Cell::new(0) };
    static HELD: std::cell::RefCell<HashSet<u32>> = std::cell::RefCell::new(HashSet::new());
}

/// Hooks globais de teclado e mouse numa thread com laço de mensagens.
pub fn listen_input() -> Result<mpsc::Receiver<Event>> {
    let (sender, receiver) = mpsc::channel();
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    *SINK.lock().expect("trava da gravação") = Some(sender);
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::spawn(move || unsafe {
        MY_GENERATION.with(|g| g.set(generation));
        let module = GetModuleHandleW(std::ptr::null());
        let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0);
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0);
        let _ = ready_tx.send(!keyboard.is_null() && !mouse.is_null());
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        UnhookWindowsHookEx(keyboard);
        UnhookWindowsHookEx(mouse);
    });
    if !ready_rx.recv().unwrap_or(false) {
        bail!("o Windows recusou os hooks de teclado e mouse");
    }
    Ok(receiver)
}

/// Repassa o evento; se a gravação acabou ou foi substituída, encerra a thread.
fn deliver(input: RawInput) {
    let current = MY_GENERATION.with(Cell::get) == GENERATION.load(Ordering::SeqCst);
    let delivered = current
        && SINK
            .lock()
            .ok()
            .and_then(|sink| sink.as_ref().map(|s| s.send((Instant::now(), input)).is_ok()))
            .unwrap_or(false);
    if !delivered {
        unsafe { PostQuitMessage(0) };
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        let repeat = HELD.with(|held| {
            let mut held = held.borrow_mut();
            if down {
                !held.insert(info.vkCode)
            } else {
                held.remove(&info.vkCode);
                false
            }
        });
        if info.flags & LLKHF_INJECTED == 0
            && !repeat
            && let Some(key) = Key::from_vk(info.vkCode as u16)
        {
            deliver(RawInput::Key { key, down });
        }
    }
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
        if info.flags & LLMHF_INJECTED == 0 {
            let input = match wparam as u32 {
                WM_LBUTTONDOWN => Some(RawInput::Button { button: Button::Left, down: true }),
                WM_LBUTTONUP => Some(RawInput::Button { button: Button::Left, down: false }),
                WM_RBUTTONDOWN => Some(RawInput::Button { button: Button::Right, down: true }),
                WM_RBUTTONUP => Some(RawInput::Button { button: Button::Right, down: false }),
                WM_MBUTTONDOWN => Some(RawInput::Button { button: Button::Middle, down: true }),
                WM_MBUTTONUP => Some(RawInput::Button { button: Button::Middle, down: false }),
                WM_MOUSEWHEEL => {
                    let delta = (info.mouseData >> 16) as u16 as i16;
                    Some(RawInput::Wheel(i32::from(delta) / WHEEL_DELTA as i32))
                }
                _ => None,
            };
            if let Some(input) = input {
                deliver(input);
            }
        }
    }
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}
