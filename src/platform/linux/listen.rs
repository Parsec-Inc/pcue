use std::sync::mpsc;
use std::time::Instant;

use anyhow::{Result, bail};
use evdev::{Device, InputEventKind, RelativeAxisType};

use crate::constants::IGNORED_DEVICE_PREFIXES;
use crate::keys::Key;
use crate::platform::RawInput;
use crate::script::Button;

/// Uma thread por teclado/mouse físico, todos no mesmo canal.
pub fn listen_input() -> Result<mpsc::Receiver<(Instant, RawInput)>> {
    let (sender, receiver) = mpsc::channel();
    let mut count = 0;
    for (_, mut device) in evdev::enumerate() {
        if !is_relevant(&device) {
            continue;
        }
        count += 1;
        let sender = sender.clone();
        std::thread::spawn(move || {
            while let Ok(events) = device.fetch_events() {
                for event in events {
                    let Some(input) = to_raw(event.kind(), event.value()) else { continue };
                    if sender.send((Instant::now(), input)).is_err() {
                        return;
                    }
                }
            }
        });
    }
    if count == 0 {
        bail!("nenhum teclado ou mouse legível em /dev/input (o usuário está no grupo input?)");
    }
    Ok(receiver)
}

/// Converte o evento evdev; repetição de tecla e movimento são ignorados.
fn to_raw(kind: InputEventKind, value: i32) -> Option<RawInput> {
    match kind {
        InputEventKind::Key(key) => {
            let button = match key {
                evdev::Key::BTN_LEFT => Some(Button::Left),
                evdev::Key::BTN_RIGHT => Some(Button::Right),
                evdev::Key::BTN_MIDDLE => Some(Button::Middle),
                _ => None,
            };
            match (button, value) {
                (Some(button), 1) => Some(RawInput::Button(button)),
                (Some(_), _) | (None, 2) => None,
                (None, _) => Some(RawInput::Key {
                    key: Key(key.code()),
                    down: value == 1,
                }),
            }
        }
        InputEventKind::RelAxis(RelativeAxisType::REL_WHEEL) => Some(RawInput::Wheel(value)),
        _ => None,
    }
}

/// Teclado ou mouse físico, fora os dispositivos virtuais conhecidos.
fn is_relevant(device: &Device) -> bool {
    let name = device.name().unwrap_or_default().to_lowercase();
    if IGNORED_DEVICE_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return false;
    }
    device
        .supported_keys()
        .is_some_and(|keys| keys.contains(evdev::Key::KEY_A) || keys.contains(evdev::Key::BTN_LEFT))
}
