use std::collections::HashSet;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use evdev::{Device, InputEvent, InputEventKind, Key, RelativeAxisType};

use crate::constants::{
    DEFAULT_WAIT_TIMEOUT, IGNORED_DEVICE_PREFIXES, RECORD_COUNTDOWN_SECS, RECORD_MIN_SLEEP, RECORD_SLEEP_ROUNDING_MS,
};
use crate::hypr::Hypr;
use crate::launcher::{class_regex, windows_of};
use crate::keys::{MODIFIERS, combo_name, is_modifier, is_shift, key_to_char};
use crate::script::{Button, ClickTarget, Step};

/// Atalhos da gravação (sempre com SUPER).
const PAUSE_KEY: Key = Key::KEY_F9;
const STOP_KEY: Key = Key::KEY_F10;

/// Grava teclado e mouse até SUPER+F10 e devolve os passos.
pub fn record(hypr: Option<&Hypr>) -> Result<Vec<String>> {
    let receiver = listen_devices()?;
    println!("Gravando em {RECORD_COUNTDOWN_SECS}s. Troque para onde vai agir.");
    println!("  SUPER+F9  pausa/retoma (use antes de digitar senhas)");
    println!("  SUPER+F10 encerra e salva");
    if hypr.is_none() {
        println!("  sem Hyprland: cliques não terão posição e viram comentário");
    }
    for remaining in (1..=RECORD_COUNTDOWN_SECS).rev() {
        println!("  {remaining}...");
        std::thread::sleep(Duration::from_secs(1));
    }
    let start = Instant::now();
    println!("● gravando");

    let mut recorder = Recorder::new(hypr, start);
    for (time, event) in receiver {
        if time < start {
            continue;
        }
        if recorder.handle(time, &event) == Flow::Stop {
            break;
        }
    }
    recorder.flush();
    println!("■ gravação encerrada: {} passo(s)", recorder.steps.len());
    Ok(recorder.steps)
}

/// Uma thread por teclado/mouse, todos no mesmo canal.
fn listen_devices() -> Result<mpsc::Receiver<(Instant, InputEvent)>> {
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
                    if sender.send((Instant::now(), event)).is_err() {
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

/// Teclado ou mouse físico, fora os dispositivos virtuais conhecidos.
fn is_relevant(device: &Device) -> bool {
    let name = device.name().unwrap_or_default().to_lowercase();
    if IGNORED_DEVICE_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return false;
    }
    device
        .supported_keys()
        .is_some_and(|keys| keys.contains(Key::KEY_A) || keys.contains(Key::BTN_LEFT))
}

#[derive(PartialEq, Eq)]
enum Flow {
    Continue,
    Stop,
}

/// Acumula eventos crus em passos legíveis.
struct Recorder<'a> {
    hypr: Option<&'a Hypr>,
    steps: Vec<String>,
    held: Vec<Key>,
    lone_modifier: Option<Key>,
    text: String,
    text_span: Option<(Instant, Instant)>,
    scroll: Option<(Instant, Instant, i32)>,
    last: Instant,
    paused: bool,
    waited: HashSet<String>,
}

impl<'a> Recorder<'a> {
    fn new(hypr: Option<&'a Hypr>, start: Instant) -> Self {
        Self {
            hypr,
            steps: Vec::new(),
            held: Vec::new(),
            lone_modifier: None,
            text: String::new(),
            text_span: None,
            scroll: None,
            last: start,
            paused: false,
            waited: HashSet::new(),
        }
    }

    fn handle(&mut self, time: Instant, event: &InputEvent) -> Flow {
        match event.kind() {
            InputEventKind::Key(key) => self.on_key(time, key, event.value()),
            InputEventKind::RelAxis(RelativeAxisType::REL_WHEEL) if !self.paused => {
                self.on_scroll(time, event.value());
                Flow::Continue
            }
            _ => Flow::Continue,
        }
    }

    fn on_key(&mut self, time: Instant, key: Key, value: i32) -> Flow {
        if is_modifier(key) {
            self.on_modifier(time, key, value);
            return Flow::Continue;
        }
        if value != 1 {
            return Flow::Continue;
        }
        self.lone_modifier = None;
        if self.super_held() && key == STOP_KEY {
            return Flow::Stop;
        }
        if self.super_held() && key == PAUSE_KEY {
            self.flush();
            self.paused = !self.paused;
            println!("{}", if self.paused { "‖ pausado" } else { "● gravando" });
            return Flow::Continue;
        }
        if self.paused {
            return Flow::Continue;
        }
        if let Some(button) = Button::from_key(key) {
            self.on_click(time, button);
            return Flow::Continue;
        }
        self.on_press(time, key);
        Flow::Continue
    }

    /// Modificador sozinho (ex.: só SUPER) vira um passo próprio.
    fn on_modifier(&mut self, time: Instant, key: Key, value: i32) {
        match value {
            1 => {
                if !self.held.contains(&key) {
                    self.held.push(key);
                }
                self.lone_modifier = Some(key);
            }
            0 => {
                self.held.retain(|k| *k != key);
                if self.lone_modifier.take() == Some(key) && !self.paused {
                    self.push(time, Step::Key(vec![key]).to_string());
                }
            }
            _ => {}
        }
    }

    /// Tecla comum: atalho, caractere ou tecla especial.
    fn on_press(&mut self, time: Instant, key: Key) {
        let shift = self.held.iter().any(|k| is_shift(*k));
        let chord = self.held.iter().any(|k| !is_shift(*k));
        if !chord {
            if let Some(ch) = key_to_char(key, shift) {
                self.flush_scroll();
                self.text.push(ch);
                let first = self.text_span.map_or(time, |(first, _)| first);
                self.text_span = Some((first, time));
                return;
            }
            if key == Key::KEY_BACKSPACE && self.text.pop().is_some() {
                return;
            }
        }
        self.flush();
        let mut combo: Vec<Key> = MODIFIERS
            .iter()
            .copied()
            .filter(|m| self.held.contains(m))
            .collect();
        combo.push(key);
        self.push(time, format!("key {}", combo_name(&combo)));
    }

    /// Clique relativo à janela sob o cursor, com espera pela janela antes.
    fn on_click(&mut self, time: Instant, button: Button) {
        self.flush();
        let Some(hypr) = self.hypr else {
            self.push(time, format!("# clique {button:?} não gravado (sem Hyprland)"));
            return;
        };
        let Ok((x, y)) = hypr.cursor_pos() else {
            self.push(time, "# clique não gravado (cursor indisponível)".into());
            return;
        };
        let target = match hypr.window_at(x, y).ok().flatten() {
            Some(window) => {
                let class = literal_class(&window.class);
                let nth = class_regex(&class)
                    .ok()
                    .zip(hypr.clients().ok())
                    .and_then(|(re, clients)| {
                        windows_of(clients, &re)
                            .iter()
                            .position(|c| c.address == window.address)
                    })
                    .map_or(1, |index| index + 1);
                if self.waited.insert(class.clone()) {
                    let wait = Step::WaitWindow {
                        class: class.clone(),
                        timeout: DEFAULT_WAIT_TIMEOUT,
                    };
                    self.push(time, wait.to_string());
                }
                ClickTarget::Window {
                    class,
                    nth,
                    x: x - window.at[0],
                    y: y - window.at[1],
                }
            }
            None => ClickTarget::Absolute { x, y },
        };
        self.push(time, Step::Click { button, target }.to_string());
    }

    fn on_scroll(&mut self, time: Instant, value: i32) {
        self.flush_text();
        self.scroll = Some(match self.scroll {
            Some((first, _, total)) => (first, time, total + value),
            None => (time, time, value),
        });
    }

    fn super_held(&self) -> bool {
        self.held
            .iter()
            .any(|k| *k == Key::KEY_LEFTMETA || *k == Key::KEY_RIGHTMETA)
    }

    fn flush(&mut self) {
        self.flush_text();
        self.flush_scroll();
    }

    fn flush_text(&mut self) {
        if let Some((first, last)) = self.text_span.take() {
            let text = std::mem::take(&mut self.text);
            if !text.is_empty() {
                self.push(first, Step::Type(text).to_string());
            }
            self.last = last;
        }
    }

    fn flush_scroll(&mut self) {
        if let Some((first, last, total)) = self.scroll.take() {
            if total != 0 {
                self.push(first, Step::Scroll(total).to_string());
            }
            self.last = last;
        }
    }

    /// Acrescenta o passo, com `sleep` se houve pausa perceptível antes.
    fn push(&mut self, time: Instant, line: String) {
        let gap = time.saturating_duration_since(self.last);
        if !self.steps.is_empty() && gap >= RECORD_MIN_SLEEP {
            let rounded = gap.as_millis().div_ceil(RECORD_SLEEP_ROUNDING_MS) * RECORD_SLEEP_ROUNDING_MS;
            self.steps
                .push(Step::Sleep(Duration::from_millis(rounded as u64)).to_string());
        }
        self.steps.push(line);
        self.last = time;
    }
}

/// Class como regex literal, sem escapar o `-` que não precisa.
fn literal_class(class: &str) -> String {
    regex::escape(class).replace("\\-", "-")
}
