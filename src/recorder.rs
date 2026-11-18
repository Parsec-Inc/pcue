use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::constants::{DESKTOP_POLL, DRAG_THRESHOLD, RECORD_COUNTDOWN_SECS, RECORDED_DRAG_MAX, RECORDED_DRAG_MIN};
use crate::draft::{self, Action, ActionKind, Change, DesktopEvent, Draft, Seen};
use crate::keys::{Key, MODIFIERS, is_modifier, is_shift, is_super, key_to_char};
use crate::launcher::class_and_nth;
use crate::platform::{Desktop, RawInput, Window, listen_input};
use crate::script::{Button, ClickTarget, Step};

/// Atalhos da gravação (sempre com SUPER).
const PAUSE_KEY: Key = Key::F9;
const STOP_KEY: Key = Key::F10;
/// Frequência com que a gravação confere o pedido de parada.
const STOP_POLL: Duration = Duration::from_millis(100);

/// O que a gravação reporta enquanto acontece.
#[derive(Debug, Clone)]
pub enum RecordEvent {
    Countdown(u64),
    Started,
    Paused(bool),
    /// Ação crua gravada.
    Step(String),
    /// Mudança percebida no desktop (janela aberta, foco, workspace).
    Desktop(String),
    Done(Draft),
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
            Ok(draft) => RecordEvent::Done(draft),
            Err(err) => RecordEvent::Failed(format!("{err:#}")),
        });
    });
    Session { events, stop }
}

fn record(sender: &mpsc::Sender<RecordEvent>, stop: &AtomicBool) -> Result<Draft> {
    let receiver = listen_input()?;
    let desktop = Desktop::connect();
    for remaining in (1..=RECORD_COUNTDOWN_SECS).rev() {
        let _ = sender.send(RecordEvent::Countdown(remaining));
        std::thread::sleep(Duration::from_secs(1));
        if stop.load(Ordering::Relaxed) {
            return Ok(Draft::default());
        }
    }
    let start = Instant::now();
    let _ = sender.send(RecordEvent::Started);

    let mut recorder = Recorder::new(&desktop, sender);
    let mut watcher = desktop.manager().map(|_| Watcher::new(&desktop));
    while !stop.load(Ordering::Relaxed) {
        if let Some(watcher) = watcher.as_mut()
            && watcher.last_poll.elapsed() >= DESKTOP_POLL
        {
            for event in watcher.poll() {
                if !recorder.paused {
                    let _ = sender.send(RecordEvent::Desktop(describe_change(&event.change)));
                    recorder.changes.push(event);
                }
            }
        }
        let wait = watcher
            .as_ref()
            .map_or(STOP_POLL, |w| DESKTOP_POLL.saturating_sub(w.last_poll.elapsed()).min(STOP_POLL));
        let (time, input) = match receiver.recv_timeout(wait) {
            Ok(received) => received,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if time >= start && recorder.handle(time, input) == Flow::Stop {
            break;
        }
    }
    recorder.flush();
    Ok(draft::build(&recorder.actions, &recorder.changes))
}

/// Texto curto da mudança para a tela de gravação.
fn describe_change(change: &Change) -> String {
    match change {
        Change::Opened { window, .. } => format!("abriu {} no workspace {}", window.class, window.workspace),
        Change::Focused { window: Some(window), .. } => format!("foco em {}", window.class),
        Change::Focused { workspace, .. } => format!("workspace {workspace}"),
    }
}

/// Acompanha janelas, foco e workspace consultando o desktop.
struct Watcher<'a> {
    desktop: &'a Desktop,
    known: HashMap<String, Window>,
    active: Option<String>,
    workspace: String,
    last_poll: Instant,
}

impl<'a> Watcher<'a> {
    fn new(desktop: &'a Desktop) -> Self {
        let mut watcher = Self {
            desktop,
            known: HashMap::new(),
            active: None,
            workspace: String::new(),
            last_poll: Instant::now(),
        };
        watcher.poll();
        watcher
    }

    /// Mudanças desde a última consulta (na primeira, só tira a foto).
    fn poll(&mut self) -> Vec<DesktopEvent> {
        let first = self.workspace.is_empty();
        self.last_poll = Instant::now();
        let at = self.last_poll;
        let Ok(windows) = self.desktop.windows() else { return Vec::new() };
        let mut events = Vec::new();
        let opened = windows.iter().filter(|w| !first && !self.known.contains_key(&w.id));
        for window in opened {
            let others = self.known.values().any(|w| w.class == window.class);
            let new_process = !self.known.values().any(|w| w.pid == window.pid);
            events.push(DesktopEvent {
                at,
                change: Change::Opened {
                    window: self.seen(window, &windows),
                    command: new_process.then(|| self.desktop.launch_command(window)).flatten(),
                    others,
                    new_process,
                },
            });
        }
        let active = self.desktop.active_window().ok().flatten();
        let workspace = self.desktop.active_workspace().unwrap_or_default();
        let active_id = active.as_ref().map(|w| w.id.clone());
        if !first && (active_id != self.active || workspace != self.workspace) {
            events.push(DesktopEvent {
                at,
                change: Change::Focused {
                    window: active.as_ref().map(|w| self.seen(w, &windows)),
                    workspace: self.desktop.workspace_for(&workspace).0,
                    workspace_changed: workspace != self.workspace,
                },
            });
        }
        self.known = windows.into_iter().map(|w| (w.id.clone(), w)).collect();
        self.active = active_id;
        self.workspace = workspace;
        events
    }

    /// Janela no formato dos passos: class literal e posição entre as da class.
    fn seen(&self, window: &Window, all: &[Window]) -> Seen {
        let (class, nth) = class_and_nth(window, all);
        Seen {
            id: window.id.clone(),
            class,
            nth,
            workspace: self.desktop.workspace_for(&window.workspace).0,
        }
    }
}

#[derive(PartialEq, Eq)]
enum Flow {
    Continue,
    Stop,
}

/// Botão apertado, esperando soltar para virar clique ou arrasto.
struct Press {
    button: Button,
    time: Instant,
    screen: (i32, i32),
    target: ClickTarget,
    modifiers: Vec<Key>,
}

/// Acumula eventos crus em ações legíveis.
struct Recorder<'a> {
    desktop: &'a Desktop,
    sender: &'a mpsc::Sender<RecordEvent>,
    actions: Vec<Action>,
    changes: Vec<DesktopEvent>,
    held: Vec<Key>,
    press: Option<Press>,
    lone_modifier: Option<Key>,
    text: String,
    text_span: Option<(Instant, Instant)>,
    scroll: Option<(Instant, Instant, i32)>,
    paused: bool,
}

impl<'a> Recorder<'a> {
    fn new(desktop: &'a Desktop, sender: &'a mpsc::Sender<RecordEvent>) -> Self {
        Self {
            desktop,
            sender,
            actions: Vec::new(),
            changes: Vec::new(),
            held: Vec::new(),
            press: None,
            lone_modifier: None,
            text: String::new(),
            text_span: None,
            scroll: None,
            paused: false,
        }
    }

    fn handle(&mut self, time: Instant, input: RawInput) -> Flow {
        match input {
            RawInput::Key { key, down } => return self.on_key(time, key, down),
            RawInput::Button { button, down } => {
                self.lone_modifier = None;
                match (self.paused, down) {
                    (true, _) => {}
                    (false, true) => self.on_button_down(time, button),
                    (false, false) => self.on_button_up(time, button),
                }
            }
            RawInput::Wheel(amount) if !self.paused => self.on_scroll(time, amount),
            RawInput::Wheel(_) => {}
        }
        Flow::Continue
    }

    fn on_key(&mut self, time: Instant, key: Key, down: bool) -> Flow {
        if is_modifier(key) {
            self.on_modifier(time, key, down);
            return Flow::Continue;
        }
        if !down {
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
        if !self.paused {
            self.on_press(time, key);
        }
        Flow::Continue
    }

    /// Modificador sozinho (ex.: só SUPER) vira um passo próprio.
    fn on_modifier(&mut self, time: Instant, key: Key, down: bool) {
        if down {
            if !self.held.contains(&key) {
                self.held.push(key);
            }
            self.lone_modifier = Some(key);
            return;
        }
        self.held.retain(|k| *k != key);
        if self.lone_modifier.take() == Some(key) && !self.paused {
            self.flush();
            self.push(time, time, ActionKind::Combo, Step::Key(vec![key]));
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
            if key == Key::BACKSPACE && self.text.pop().is_some() {
                return;
            }
        }
        self.flush();
        let mut combo = self.held_modifiers();
        combo.push(key);
        self.push(time, time, ActionKind::Combo, Step::Key(combo));
    }

    /// Guarda onde o botão desceu, relativo à janela sob o cursor.
    fn on_button_down(&mut self, time: Instant, button: Button) {
        if self.press.is_some() {
            return;
        }
        self.flush();
        let desktop = self.desktop;
        if desktop.manager().is_none() {
            let _ = self.sender.send(RecordEvent::Desktop(format!(
                "clique {button:?} não gravado (sem gerenciador de janelas)"
            )));
            return;
        }
        let Ok((x, y)) = desktop.cursor_pos() else { return };
        let windows = desktop.windows().unwrap_or_default();
        let under = windows
            .iter()
            .filter(|w| w.on_screen && w.contains(x, y))
            .min_by_key(|w| w.focus_rank);
        let target = match under {
            Some(window) => ClickTarget {
                window: Some(class_and_nth(window, &windows)),
                x: x - window.at[0],
                y: y - window.at[1],
            },
            None => ClickTarget { window: None, x, y },
        };
        let modifiers = self.held_modifiers();
        self.press = Some(Press { button, time, screen: (x, y), target, modifiers });
    }

    /// Botão solto: arrasto se o cursor andou, senão clique (com os modificadores segurados).
    fn on_button_up(&mut self, time: Instant, button: Button) {
        let Some(press) = self.press.take_if(|press| press.button == button) else { return };
        let (x, y) = self.desktop.cursor_pos().unwrap_or(press.screen);
        let moved = (x - press.screen.0).abs().max((y - press.screen.1).abs()) > DRAG_THRESHOLD;
        let hold = press.modifiers;
        if !moved {
            self.push(press.time, time, ActionKind::Click, Step::Click { button, hold, target: press.target });
            return;
        }
        let ClickTarget { window, x: from_x, y: from_y } = press.target;
        let (from, to) = ((from_x, from_y), (from_x + x - press.screen.0, from_y + y - press.screen.1));
        let elapsed = time.saturating_duration_since(press.time).clamp(RECORDED_DRAG_MIN, RECORDED_DRAG_MAX);
        let step = Step::Drag { button, hold, window, from, to, duration: draft::round_recorded(elapsed) };
        self.push(press.time, time, ActionKind::Click, step);
    }

    fn on_scroll(&mut self, time: Instant, value: i32) {
        self.flush_text();
        self.scroll = Some(match self.scroll {
            Some((first, _, total)) => (first, time, total + value),
            None => (time, time, value),
        });
    }

    fn held_modifiers(&self) -> Vec<Key> {
        MODIFIERS.iter().copied().filter(|m| self.held.contains(m)).collect()
    }

    fn super_held(&self) -> bool {
        self.held.iter().any(|k| is_super(*k))
    }

    fn flush(&mut self) {
        self.flush_text();
        self.flush_scroll();
    }

    fn flush_text(&mut self) {
        if let Some((first, last)) = self.text_span.take() {
            let text = std::mem::take(&mut self.text);
            if !text.is_empty() {
                self.push(first, last, ActionKind::Text, Step::Type(text));
            }
        }
    }

    fn flush_scroll(&mut self) {
        if let Some((first, last, total)) = self.scroll.take()
            && total != 0
        {
            self.push(first, last, ActionKind::Scroll, Step::Scroll(total));
        }
    }

    fn push(&mut self, start: Instant, end: Instant, kind: ActionKind, step: Step) {
        let _ = self.sender.send(RecordEvent::Step(step.to_string()));
        self.actions.push(Action { start, end, kind, step });
    }
}
