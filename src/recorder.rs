use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use evdev::{Device, InputEvent, InputEventKind, Key, RelativeAxisType};

use crate::constants::{
    DEFAULT_WAIT_TIMEOUT, IGNORED_DEVICE_PREFIXES, RECORD_COUNTDOWN_SECS, RECORD_MIN_SLEEP, RECORD_SLEEP_ROUNDING_MS,
};
use crate::hypr::Hypr;
use crate::launcher::{anchored_regex, windows_of};
use crate::keys::{MODIFIERS, combo_name, is_modifier, is_shift, key_to_char};
use crate::script::{Button, ClickTarget, Step};

/// Atalhos da gravação (sempre com SUPER).
const PAUSE_KEY: Key = Key::KEY_F9;
const STOP_KEY: Key = Key::KEY_F10;
/// Frequência com que a gravação confere o pedido de parada.
const STOP_POLL: Duration = Duration::from_millis(100);

/// O que a gravação reporta enquanto acontece.
#[derive(Debug, Clone)]
pub enum RecordEvent {
    Countdown(u64),
    Started,
    Paused(bool),
    Step(String),
    Done(Vec<String>),
    Failed(String),
}

/// Gravação rodando numa thread própria.
pub struct Session {
    pub events: mpsc::Receiver<RecordEvent>,
    stop: Arc<AtomicBool>,
}

impl Session {
    /// Encerra a gravação como SUPER+F10.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Começa a gravar teclado e mouse até SUPER+F10 ou `Session::stop`.
pub fn start() -> Session {
    let (sender, events) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    std::thread::spawn(move || {
        let result = record(&sender, &flag);
        let _ = sender.send(match result {
            Ok(steps) => RecordEvent::Done(steps),
            Err(err) => RecordEvent::Failed(format!("{err:#}")),
        });
    });
    Session { events, stop }
}

fn record(sender: &mpsc::Sender<RecordEvent>, stop: &AtomicBool) -> Result<Vec<String>> {
    let receiver = listen_devices()?;
    let hypr = Hypr::detect();
    for remaining in (1..=RECORD_COUNTDOWN_SECS).rev() {
        let _ = sender.send(RecordEvent::Countdown(remaining));
        std::thread::sleep(Duration::from_secs(1));
        if stop.load(Ordering::Relaxed) {
            return Ok(Vec::new());
        }
    }
    let start = Instant::now();
    let _ = sender.send(RecordEvent::Started);

    let mut recorder = Recorder::new(hypr.as_ref(), start, sender);
    while !stop.load(Ordering::Relaxed) {
        let (time, event) = match receiver.recv_timeout(STOP_POLL) {
            Ok(received) => received,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if time >= start && recorder.handle(time, &event) == Flow::Stop {
            break;
        }
    }
    recorder.flush();
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
    sender: &'a mpsc::Sender<RecordEvent>,
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
    fn new(hypr: Option<&'a Hypr>, start: Instant, sender: &'a mpsc::Sender<RecordEvent>) -> Self {
        Self {
            hypr,
            sender,
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
            let _ = self.sender.send(RecordEvent::Paused(self.paused));
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
                let nth = anchored_regex(&class)
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
                        nth: 1,
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
            self.emit(Step::Sleep(Duration::from_millis(rounded as u64)).to_string());
        }
        self.emit(line);
        self.last = time;
    }

    fn emit(&mut self, line: String) {
        let _ = self.sender.send(RecordEvent::Step(line.clone()));
        self.steps.push(line);
    }
}

/// Class como regex literal, sem escapar o `-` que não precisa.
fn literal_class(class: &str) -> String {
    regex::escape(class).replace("\\-", "-")
}
