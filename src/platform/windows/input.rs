use std::mem::size_of;

use anyhow::{Result, anyhow};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, MOUSE_EVENT_FLAGS, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL,
    MOUSEINPUT, MapVirtualKeyW, SendInput, VK_RETURN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::WHEEL_DELTA;

use crate::constants::KEY_STEP_DELAY;
use crate::keys::Key;
use crate::script::{Button, Input};

/// Teclas que o Windows marca como estendidas (setas, navegação, lado direito).
const EXTENDED_VKS: &[u16] = &[
    0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x2C, 0x2D, 0x2E, 0x5B, 0x5C, 0x5D, 0x6F, 0x90, 0xA3, 0xA5,
];

/// Teclado e mouse simulados com `SendInput`.
pub struct VirtualInput;

impl VirtualInput {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    /// Pressiona o atalho em ordem e solta ao contrário.
    pub fn combo(&mut self, keys: &[Key]) -> Result<()> {
        let vks = keys.iter().map(|key| vk_of(*key)).collect::<Result<Vec<u16>>>()?;
        for vk in &vks {
            send(&[key_input(*vk, false)])?;
        }
        for vk in vks.iter().rev() {
            send(&[key_input(*vk, true)])?;
        }
        Ok(())
    }

    /// Digita o texto como Unicode, independente do layout do teclado.
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        for ch in text.chars() {
            if ch == '\n' {
                send(&[key_input(VK_RETURN, false)])?;
                send(&[key_input(VK_RETURN, true)])?;
                continue;
            }
            let mut units = [0u16; 2];
            for unit in ch.encode_utf16(&mut units).iter() {
                send(&[unicode_input(*unit, false), unicode_input(*unit, true)])?;
            }
        }
        Ok(())
    }

    /// Clica com o botão onde o cursor estiver.
    pub fn click(&mut self, button: Button) -> Result<()> {
        self.press(Input::Mouse(button))?;
        self.release(Input::Mouse(button))
    }

    /// Aperta e mantém a tecla ou o botão.
    pub fn press(&mut self, input: Input) -> Result<()> {
        send(&[input_event(input, false)?])
    }

    /// Solta a tecla ou o botão.
    pub fn release(&mut self, input: Input) -> Result<()> {
        send(&[input_event(input, true)?])
    }

    /// Rola a roda; positivo sobe, negativo desce.
    pub fn scroll(&mut self, amount: i32) -> Result<()> {
        let tick = amount.signum() * WHEEL_DELTA as i32;
        for _ in 0..amount.unsigned_abs() {
            send(&[mouse_input(MOUSEEVENTF_WHEEL, tick)])?;
        }
        Ok(())
    }
}

fn vk_of(key: Key) -> Result<u16> {
    key.vk().ok_or_else(|| anyhow!("tecla sem equivalente no Windows: {key:?}"))
}

fn input_event(input: Input, up: bool) -> Result<INPUT> {
    Ok(match input {
        Input::Key(key) => key_input(vk_of(key)?, up),
        Input::Mouse(button) => {
            let (down_flag, up_flag) = match button {
                Button::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
                Button::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
                Button::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
            };
            mouse_input(if up { up_flag } else { down_flag }, 0)
        }
    })
}

fn key_input(vk: u16, up: bool) -> INPUT {
    let mut flags = if up { KEYEVENTF_KEYUP } else { 0 };
    if EXTENDED_VKS.contains(&vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    let scan = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) } as u16;
    keyboard(vk, scan, flags)
}

fn unicode_input(unit: u16, up: bool) -> INPUT {
    let flags = KEYEVENTF_UNICODE | if up { KEYEVENTF_KEYUP } else { 0 };
    keyboard(0, unit, flags)
}

fn keyboard(vk: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn mouse_input(flags: MOUSE_EVENT_FLAGS, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Envia os eventos e espera o intervalo entre teclas.
fn send(inputs: &[INPUT]) -> Result<()> {
    let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
    anyhow::ensure!(sent as usize == inputs.len(), "o Windows bloqueou a entrada simulada");
    std::thread::sleep(KEY_STEP_DELAY);
    Ok(())
}
