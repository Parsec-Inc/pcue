use anyhow::{Context, Result, anyhow};
use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{AttributeSet, EventType, InputEvent, RelativeAxisType};

use crate::constants::{KEY_STEP_DELAY, VIRTUAL_DEVICE_NAME, VIRTUAL_DEVICE_SETTLE};
use crate::keys::{Key, char_to_key};
use crate::script::Button;

/// Último código de tecla de teclado comum emitido pelo dispositivo.
const MAX_KEYBOARD_CODE: u16 = 248;

/// Teclado e mouse virtuais via uinput.
pub struct VirtualInput {
    device: VirtualDevice,
}

impl VirtualInput {
    /// Cria o dispositivo e espera o compositor reconhecê-lo.
    pub fn new() -> Result<Self> {
        let mut keys = AttributeSet::<evdev::Key>::new();
        for code in 1..=MAX_KEYBOARD_CODE {
            keys.insert(evdev::Key::new(code));
        }
        for button in [Button::Left, Button::Right, Button::Middle] {
            keys.insert(button_code(button));
        }
        let mut axes = AttributeSet::<RelativeAxisType>::new();
        for axis in [RelativeAxisType::REL_X, RelativeAxisType::REL_Y, RelativeAxisType::REL_WHEEL] {
            axes.insert(axis);
        }
        let device = VirtualDeviceBuilder::new()
            .context("sem acesso a /dev/uinput (precisa do grupo input)")?
            .name(VIRTUAL_DEVICE_NAME)
            .with_keys(&keys)?
            .with_relative_axes(&axes)?
            .build()
            .context("falha ao criar o dispositivo virtual")?;
        std::thread::sleep(VIRTUAL_DEVICE_SETTLE);
        Ok(Self { device })
    }

    /// Pressiona o atalho em ordem e solta ao contrário.
    pub fn combo(&mut self, keys: &[Key]) -> Result<()> {
        for key in keys {
            self.key(*key, 1)?;
        }
        for key in keys.iter().rev() {
            self.key(*key, 0)?;
        }
        Ok(())
    }

    /// Digita o texto no layout US.
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        for ch in text.chars() {
            let (key, shift) =
                char_to_key(ch).ok_or_else(|| anyhow!("caractere sem tecla no layout US: {ch:?}"))?;
            if shift {
                self.combo(&[Key::LEFTSHIFT, key])?;
            } else {
                self.combo(&[key])?;
            }
        }
        Ok(())
    }

    /// Clica com o botão onde o cursor estiver.
    pub fn click(&mut self, button: Button) -> Result<()> {
        let code = button_code(button);
        self.emit(EventType::KEY, code.code(), 1)?;
        self.emit(EventType::KEY, code.code(), 0)
    }

    /// Rola a roda; positivo sobe, negativo desce.
    pub fn scroll(&mut self, amount: i32) -> Result<()> {
        let tick = amount.signum();
        for _ in 0..amount.unsigned_abs() {
            self.emit(EventType::RELATIVE, RelativeAxisType::REL_WHEEL.0, tick)?;
        }
        Ok(())
    }

    fn key(&mut self, key: Key, value: i32) -> Result<()> {
        self.emit(EventType::KEY, key.0, value)
    }

    fn emit(&mut self, kind: EventType, code: u16, value: i32) -> Result<()> {
        self.device.emit(&[InputEvent::new(kind, code, value)])?;
        std::thread::sleep(KEY_STEP_DELAY);
        Ok(())
    }
}

/// Código evdev do botão.
pub fn button_code(button: Button) -> evdev::Key {
    match button {
        Button::Left => evdev::Key::BTN_LEFT,
        Button::Right => evdev::Key::BTN_RIGHT,
        Button::Middle => evdev::Key::BTN_MIDDLE,
    }
}
