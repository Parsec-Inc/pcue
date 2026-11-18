use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::constants::{
    CURSOR_HISTORY, CURSOR_SAMPLE, DESKTOP_POLL, DRAG_PATH_TOLERANCE, DRAG_THRESHOLD, RECORD_COUNTDOWN_SECS,
    RECORDED_DRAG_MIN,
};
use crate::draft::{self, Action, ActionKind, Change, DesktopEvent, Draft, Seen};
use crate::keys::{Key, MODIFIERS, is_modifier, is_shift, is_super, key_to_char};
use crate::launcher::class_and_nth;
use crate::platform::{Desktop, RawInput, Window, listen_input};
use crate::script::{Button, ClickTarget, Step, Waypoint};

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

    let desktop_events = desktop.manager().map(|_| watch_desktop());
    let mut recorder = Recorder::new(&desktop, sender, Track::start());
    while !stop.load(Ordering::Relaxed) {
        for event in desktop_events.iter().flat_map(|feed| feed.events.try_iter()) {
            if !recorder.paused {
                let _ = sender.send(RecordEvent::Desktop(describe_change(&event.change)));
                recorder.changes.push(event);
            }
        }
        recorder.sync_track();
        let (time, input) = match receiver.recv_timeout(STOP_POLL) {
            Ok(received) => received,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if time >= start && recorder.handle(time, input) == Flow::Stop {
            break;
        }
    }
    if !recorder.paused {
        recorder.changes.extend(desktop_events.iter().flat_map(|feed| feed.events.try_iter()));
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

/// Mudanças do desktop vindas de uma thread; ela para quando isto é descartado.
struct DesktopFeed {
    events: mpsc::Receiver<DesktopEvent>,
    done: Arc<AtomicBool>,
}

impl Drop for DesktopFeed {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Relaxed);
    }
}

/// Consulta o desktop numa thread própria a cada aviso de mudança, ou de tempos em tempos.
fn watch_desktop() -> DesktopFeed {
    let (sender, events) = mpsc::channel();
    let done = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&done);
    std::thread::spawn(move || {
        let desktop = Desktop::connect();
        let changes = desktop.watch_changes();
        let mut watcher = Watcher::new(&desktop);
        while !flag.load(Ordering::Relaxed) {
            let at = match changes.as_ref().map(|c| c.recv_timeout(DESKTOP_POLL)) {
                Some(Ok(at)) => at,
                Some(Err(mpsc::RecvTimeoutError::Timeout)) => Instant::now(),
                Some(Err(mpsc::RecvTimeoutError::Disconnected)) | None => {
                    std::thread::sleep(DESKTOP_POLL);
                    Instant::now()
                }
            };
            for event in watcher.poll(at) {
                if sender.send(event).is_err() {
                    return;
                }
            }
        }
    });
    DesktopFeed { events, done }
}

/// Acompanha janelas, foco e workspace consultando o desktop.
struct Watcher<'a> {
    desktop: &'a Desktop,
    known: HashMap<String, Window>,
    active: Option<String>,
    workspace: String,
}

impl<'a> Watcher<'a> {
    fn new(desktop: &'a Desktop) -> Self {
        let mut watcher = Self {
            desktop,
            known: HashMap::new(),
            active: None,
            workspace: String::new(),
        };
        watcher.poll(Instant::now());
        watcher
    }

    /// Mudanças desde a última consulta (na primeira, só tira a foto).
    fn poll(&mut self, at: Instant) -> Vec<DesktopEvent> {
        let first = self.workspace.is_empty();
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

/// Posições do cursor lidas numa thread, com o instante de cada leitura.
struct Track {
    samples: mpsc::Receiver<(Instant, (i32, i32))>,
    points: VecDeque<(Instant, (i32, i32))>,
    latest: Option<Instant>,
}

impl Track {
    fn start() -> Self {
        let (sender, samples) = mpsc::channel();
        std::thread::spawn(move || {
            let desktop = Desktop::connect();
            while let Ok(point) = desktop.cursor_pos() {
                if sender.send((Instant::now(), point)).is_err() {
                    return;
                }
                std::thread::sleep(CURSOR_SAMPLE);
            }
        });
        Self { samples, points: VecDeque::new(), latest: None }
    }

    fn push(&mut self, (time, point): (Instant, (i32, i32))) {
        self.latest = Some(time);
        if self.points.back().is_none_or(|(_, last)| *last != point) {
            self.points.push_back((time, point));
        }
    }

    fn drain(&mut self) {
        while let Ok(sample) = self.samples.try_recv() {
            self.push(sample);
        }
    }

    /// Espera a leitura que cobre o instante, se a thread ainda estiver lendo.
    fn catch_up(&mut self, time: Instant) {
        self.drain();
        while self.latest.is_none_or(|latest| latest < time) {
            match self.samples.recv_timeout(CURSOR_SAMPLE * 4) {
                Ok(sample) => self.push(sample),
                Err(_) => break,
            }
        }
    }

    /// Onde o cursor estava no instante.
    fn at(&self, time: Instant) -> Option<(i32, i32)> {
        self.points.iter().rev().find(|(t, _)| *t <= time).or(self.points.front()).map(|(_, point)| *point)
    }

    /// Posições lidas estritamente entre os dois instantes.
    fn between(&self, from: Instant, to: Instant) -> impl Iterator<Item = (Instant, (i32, i32))> + '_ {
        self.points.iter().copied().filter(move |(t, _)| *t > from && *t < to)
    }

    /// Esquece o que veio antes do instante, mantendo a última posição anterior a ele.
    fn forget_before(&mut self, cutoff: Instant) {
        while self.points.get(1).is_some_and(|(t, _)| *t <= cutoff) {
            self.points.pop_front();
        }
    }
}

/// Acumula eventos crus em ações legíveis.
struct Recorder<'a> {
    desktop: &'a Desktop,
    sender: &'a mpsc::Sender<RecordEvent>,
    track: Track,
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
    fn new(desktop: &'a Desktop, sender: &'a mpsc::Sender<RecordEvent>, track: Track) -> Self {
        Self {
            desktop,
            sender,
            track,
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

    /// Lê as posições novas e descarta as antigas que nenhum clique vai precisar.
    fn sync_track(&mut self) {
        self.track.drain();
        let cutoff = match &self.press {
            Some(press) => Some(press.time),
            None => Instant::now().checked_sub(CURSOR_HISTORY),
        };
        if let Some(cutoff) = cutoff {
            self.track.forget_before(cutoff);
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
        self.track.catch_up(time);
        let Some((x, y)) = self.track.at(time).or_else(|| desktop.cursor_pos().ok()) else { return };
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

    /// Botão solto: arrasto com o trajeto inteiro se o cursor saiu do lugar, senão clique.
    fn on_button_up(&mut self, time: Instant, button: Button) {
        let Some(press) = self.press.take_if(|press| press.button == button) else { return };
        self.track.catch_up(time);
        let end = self.track.at(time).unwrap_or(press.screen);
        let since = |t: Instant| whole_ms(t.saturating_duration_since(press.time));
        let trail: Vec<Waypoint> = std::iter::once((Duration::ZERO, press.screen))
            .chain(self.track.between(press.time, time).map(|(t, point)| (since(t), point)))
            .chain(std::iter::once((since(time), end)))
            .collect();
        let (sx, sy) = press.screen;
        let moved = trail.iter().any(|(_, (x, y))| (x - sx).abs().max((y - sy).abs()) > DRAG_THRESHOLD);
        let hold = press.modifiers;
        if !moved {
            self.push(press.time, time, ActionKind::Click, Step::Click { button, hold, target: press.target });
            return;
        }
        let ClickTarget { window, x: from_x, y: from_y } = press.target;
        let shift = |(x, y): (i32, i32)| (x - sx + from_x, y - sy + from_y);
        let kept = simplify(&trail, DRAG_PATH_TOLERANCE);
        let via = kept[1..kept.len() - 1].iter().map(|&i| (trail[i].0, shift(trail[i].1))).collect();
        let duration = since(time).max(RECORDED_DRAG_MIN);
        let step = Step::Drag { button, hold, window, from: (from_x, from_y), via, to: shift(end), duration };
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

fn whole_ms(duration: Duration) -> Duration {
    Duration::from_millis(duration.as_millis() as u64)
}

/// Pontos que mantêm o trajeto, no tempo, a até `tolerance` pixels do gravado.
fn simplify(trail: &[Waypoint], tolerance: f64) -> Vec<usize> {
    let mut keep = vec![false; trail.len()];
    let mut spans = vec![(0, trail.len().saturating_sub(1))];
    while let Some((a, b)) = spans.pop() {
        keep[a] = true;
        keep[b] = true;
        let worst = (a + 1..b)
            .map(|i| (i, drift(trail[i], trail[a], trail[b])))
            .max_by(|x, y| x.1.total_cmp(&y.1));
        if let Some((i, _)) = worst.filter(|(_, d)| *d > tolerance) {
            spans.extend([(a, i), (i, b)]);
        }
    }
    keep.iter().enumerate().filter(|(_, k)| **k).map(|(i, _)| i).collect()
}

/// Distância entre o ponto e onde o trecho reto `a`→`b` estaria no mesmo instante.
fn drift((t, (x, y)): Waypoint, (ta, (xa, ya)): Waypoint, (tb, (xb, yb)): Waypoint) -> f64 {
    let span = (tb - ta).as_secs_f64();
    let k = if span > 0.0 { (t - ta).as_secs_f64() / span } else { 0.0 };
    let ex = f64::from(xa) + f64::from(xb - xa) * k;
    let ey = f64::from(ya) + f64::from(yb - ya) * k;
    (f64::from(x) - ex).hypot(f64::from(y) - ey)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplify_keeps_overshoot_and_pauses() {
        let ms = Duration::from_millis;
        let trail: Vec<Waypoint> = [
            (0, (0, 0)),
            (50, (50, 0)),
            (100, (100, 0)),
            (150, (80, 0)),
            (200, (60, 0)),
            (700, (60, 0)),
            (750, (60, 50)),
        ]
        .into_iter()
        .map(|(t, p)| (ms(t), p))
        .collect();
        let kept: Vec<_> = simplify(&trail, 2.0).into_iter().map(|i| trail[i]).collect();
        assert_eq!(kept, vec![trail[0], trail[2], trail[4], trail[5], trail[6]]);
    }
}
