//! Celular Android pelo adb: os passos `phone` e a gravação dos toques.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::constants::{
    DEFAULT_WAIT_TIMEOUT, PHONE_FOCUS_POLL, PHONE_LONG_PRESS, PHONE_MOTION_COST, PHONE_PATH_TOLERANCE, PHONE_TAP_SLOP,
    POLL_INTERVAL, RECORD_MIN_SLEEP,
};
use crate::draft::{Draft, round_recorded};
use crate::keys::is_super;
use crate::platform::{self, RawInput, listen_input};
use crate::recorder::{PAUSE_KEY, RecordEvent, STOP_KEY, STOP_POLL, countdown, simplify};
use crate::config::filter_regex;
use crate::script::{PhoneAction, Step, UiTarget, Waypoint};
use crate::utils::{Feed, feed, shell_quote};

/// Tipos e códigos do Linux que o `getevent` repete.
const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const BTN_TOUCH: u16 = 0x14a;

/// Botões físicos que viram `phone key`: código do Linux e nome no Android.
const KEYS: &[(u16, &str)] = &[
    (114, "VOLUME_DOWN"),
    (115, "VOLUME_UP"),
    (116, "POWER"),
    (139, "MENU"),
    (158, "BACK"),
    (172, "HOME"),
    (580, "APP_SWITCH"),
];

const ADB_MISSING: &str = "adb não encontrado (instale android-tools)";

/// `adb` sem janela e sem entrada.
fn adb() -> Command {
    let mut command = Command::new("adb");
    platform::background(&mut command);
    command.stdin(Stdio::null());
    command
}

/// `adb shell <comando>`; falha com a saída de erro.
fn adb_shell(command: &str) -> Result<String> {
    let output = adb().args(["shell", command]).output().context(ADB_MISSING)?;
    if !output.status.success() {
        bail!("adb: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Espera o celular aparecer no adb, até `timeout`.
fn wait_ready(timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let state = adb().arg("get-state").output().context(ADB_MISSING)?;
        if state.status.success() && String::from_utf8_lossy(&state.stdout).trim() == "device" {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("celular fora do adb: confira o cabo, a depuração USB e `adb devices`");
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Espera o celular no adb e acende a tela (sem efeito se já estiver acesa).
pub fn prepare(timeout: Duration) -> Result<()> {
    wait_ready(timeout)?;
    run(&PhoneAction::Key("WAKEUP".into()))
}

/// Executa um passo `phone`.
pub fn run(action: &PhoneAction) -> Result<()> {
    let command = match action {
        PhoneAction::Tap { at: (x, y), hold } if hold.is_zero() => format!("input tap {x} {y}"),
        PhoneAction::Tap { at: (x, y), hold } => format!("input swipe {x} {y} {x} {y} {}", hold.as_millis()),
        PhoneAction::Swipe { from, via, to, duration } if via.is_empty() => {
            format!("input swipe {} {} {} {} {}", from.0, from.1, to.0, to.1, duration.as_millis())
        }
        PhoneAction::Swipe { from, via, to, duration } => trace(*from, via, *to, *duration),
        PhoneAction::Key(name) => format!("input keyevent KEYCODE_{name}"),
        PhoneAction::Text(text) => format!("input text {}", shell_quote(&text.replace(' ', "%s"))),
        PhoneAction::Open { package, timeout } => return open(package, *timeout),
        PhoneAction::Wait { target, timeout } => return find(target, *timeout).map(|_| ()),
        PhoneAction::TapOn { target, hold, timeout } => {
            let at = find(target, *timeout)?;
            return run(&PhoneAction::Tap { at, hold: *hold });
        }
    };
    adb_shell(&command).map(|_| ())
}

/// Arrasto pelo caminho todo, um `input motionevent` por ponto, nos tempos gravados.
fn trace(from: (i32, i32), via: &[Waypoint], to: (i32, i32), duration: Duration) -> String {
    let mut commands = vec![format!("input motionevent DOWN {} {}", from.0, from.1)];
    let mut previous = Duration::ZERO;
    for &(at, (x, y)) in via.iter().chain([&(duration, to)]) {
        let wait = at.saturating_sub(previous).saturating_sub(PHONE_MOTION_COST);
        if !wait.is_zero() {
            commands.push(format!("sleep {:.3}", wait.as_secs_f64()));
        }
        commands.push(format!("input motionevent MOVE {x} {y}"));
        previous = at;
    }
    commands.push(format!("input motionevent UP {} {}", to.0, to.1));
    commands.join("; ")
}

/// Abre o app (se não estiver na frente) e espera ele aparecer, até `timeout`.
fn open(package: &str, timeout: Duration) -> Result<()> {
    if foreground()? == package {
        return Ok(());
    }
    let output = adb_shell(&format!("monkey -p {package} -c android.intent.category.LAUNCHER 1 2>&1"))?;
    if output.contains("No activities found") || output.contains("aborted") {
        bail!("app {package} não encontrado no celular");
    }
    let deadline = Instant::now() + timeout;
    while foreground()? != package {
        if Instant::now() >= deadline {
            bail!("{package} não abriu em {}s (celular bloqueado?)", timeout.as_secs());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    Ok(())
}

/// Centro do primeiro elemento da tela que casa com o alvo, lendo a tela até `timeout`.
fn find(target: &UiTarget, timeout: Duration) -> Result<(i32, i32)> {
    let matcher = Matcher::new(target)?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(screen) = dump_screen()
            && let Some(node) = ui_nodes(&screen).find(|node| matcher.matches(node))
        {
            return Ok(node.center());
        }
        if Instant::now() >= deadline {
            bail!("{target} não apareceu na tela do celular em {}s", timeout.as_secs());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Regex do alvo, compiladas uma vez.
struct Matcher {
    text: Option<regex::Regex>,
    id: Option<regex::Regex>,
}

impl Matcher {
    fn new(target: &UiTarget) -> Result<Self> {
        Ok(Self {
            text: target.text.as_deref().map(filter_regex).transpose()?,
            id: target.id.as_deref().map(filter_regex).transpose()?,
        })
    }

    /// Texto ou descrição casam com `text`, e o id com `id`.
    fn matches(&self, node: &UiNode) -> bool {
        self.text.as_ref().is_none_or(|re| re.is_match(&node.text) || re.is_match(&node.description))
            && self.id.as_ref().is_none_or(|re| re.is_match(&node.id))
    }
}

/// Alvo que acha, nesta tela, o menor elemento com texto, descrição ou id sob o ponto.
fn suggest_target(nodes: &[UiNode], point: (i32, i32)) -> Option<UiTarget> {
    let node = nodes
        .iter()
        .filter(|n| n.contains(point) && !(n.text.is_empty() && n.description.is_empty() && n.id.is_empty()))
        .min_by_key(|n| n.area())?;
    let candidates = [&node.text, &node.description]
        .into_iter()
        .filter(|label| !label.trim().is_empty())
        .flat_map(|label| {
            let escaped = regex::escape(label.trim());
            [UiTarget { text: Some(escaped.clone()), id: None }, UiTarget { text: Some(format!("^{escaped}$")), id: None }]
        })
        .chain((!node.id.is_empty()).then(|| UiTarget { text: None, id: Some(format!("^{}$", regex::escape(&node.id))) }));
    candidates.into_iter().find(|target| {
        let matcher = Matcher::new(target).ok();
        let first = nodes.iter().find(|n| matcher.as_ref().is_some_and(|m| m.matches(n)));
        first.is_some_and(|n| std::ptr::eq(n, node))
    })
}

/// Árvore da tela em XML, pelo uiautomator.
fn dump_screen() -> Result<String> {
    let output = adb().args(["exec-out", "uiautomator", "dump", "/dev/stdout"]).output().context(ADB_MISSING)?;
    let xml = String::from_utf8_lossy(&output.stdout);
    match xml.find("</hierarchy>") {
        Some(end) => Ok(xml[..end].to_string()),
        None => bail!("uiautomator não leu a tela"),
    }
}

/// Elemento da tela: texto, descrição, id e retângulo (x1, y1, x2, y2).
struct UiNode {
    text: String,
    description: String,
    id: String,
    bounds: (i32, i32, i32, i32),
}

impl UiNode {
    fn center(&self) -> (i32, i32) {
        ((self.bounds.0 + self.bounds.2) / 2, (self.bounds.1 + self.bounds.3) / 2)
    }

    fn contains(&self, (x, y): (i32, i32)) -> bool {
        x >= self.bounds.0 && x < self.bounds.2 && y >= self.bounds.1 && y < self.bounds.3
    }

    fn area(&self) -> i64 {
        i64::from(self.bounds.2 - self.bounds.0) * i64::from(self.bounds.3 - self.bounds.1)
    }
}

/// Elementos com área visível, na ordem do XML.
fn ui_nodes(xml: &str) -> impl Iterator<Item = UiNode> + '_ {
    xml.split("<node ").skip(1).map(|node| format!(" {node}")).filter_map(|node| {
        let attr = |name: &str| {
            let rest = &node[node.find(&format!(" {name}=\""))?..];
            let value = rest.split_once('"')?.1.split_once('"')?.0;
            Some(unescape_xml(value))
        };
        let bounds = attr("bounds")?;
        let numbers: Vec<i32> =
            bounds.split(|c: char| !c.is_ascii_digit() && c != '-').filter_map(|n| n.parse().ok()).collect();
        let [x1, y1, x2, y2] = numbers[..] else { return None };
        if x2 <= x1 || y2 <= y1 {
            return None;
        }
        Some(UiNode {
            text: attr("text").unwrap_or_default(),
            description: attr("content-desc").unwrap_or_default(),
            id: attr("resource-id").unwrap_or_default(),
            bounds: (x1, y1, x2, y2),
        })
    })
}

fn unescape_xml(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#10;", "\n")
        .replace("&amp;", "&")
}

/// Pacote do app em foco, ignorando camadas do sistema (bloqueio, painel); vazio se não houver.
fn foreground() -> Result<String> {
    let output = adb_shell("dumpsys window displays 2>/dev/null | grep -m1 mFocusedApp")?;
    Ok(focused_package(&output).unwrap_or_default())
}

/// `mFocusedApp=ActivityRecord{abc u0 com.pkg/.Activity t12}` vira `com.pkg`.
fn focused_package(line: &str) -> Option<String> {
    let activity = line.split_whitespace().find(|token| token.contains('/'))?;
    Some(activity.split('/').next()?.to_string()).filter(|p| p.contains('.'))
}

/// Pacote da tela inicial (launcher).
fn home_package() -> Option<String> {
    let output =
        adb_shell("cmd package resolve-activity --brief -c android.intent.category.HOME -a android.intent.action.MAIN").ok()?;
    output.lines().last()?.split('/').next().map(str::to_string).filter(|p| p.contains('.'))
}

/// Tela de toque: arquivo do dispositivo, faixa do sensor e pixels.
struct Screen {
    device: String,
    max: (i32, i32),
    size: (i32, i32),
}

impl Screen {
    /// Acha a tela de toque no `getevent -p` e o tamanho no `wm size`.
    fn query() -> Result<Self> {
        let (device, max) = touch_device(&adb_shell("getevent -p")?).context("tela de toque não encontrada")?;
        let size = screen_size(&adb_shell("wm size")?).context("tamanho da tela não encontrado")?;
        Ok(Self { device, max, size })
    }

    /// Posição do sensor em pixels da tela.
    fn to_pixels(&self, (x, y): (i32, i32)) -> (i32, i32) {
        let scale = |value: i32, max: i32, size: i32| (value as i64 * (size - 1) as i64 / max.max(1) as i64) as i32;
        (scale(x, self.max.0, self.size.0), scale(y, self.max.1, self.size.1))
    }
}

/// Primeiro dispositivo direto (tela) com posição multitoque, e o máximo de X e Y.
fn touch_device(listing: &str) -> Option<(String, (i32, i32))> {
    listing.split("add device").skip(1).find_map(|block| {
        let device = block.split_once(':')?.1.lines().next()?.trim().to_string();
        let max_of = |code: &str| {
            block.lines().find_map(|line| {
                let line = line.trim().trim_start_matches("ABS (0003):").trim();
                let rest = line.strip_prefix(code)?.trim_start().strip_prefix(':')?;
                rest.split("max ").nth(1)?.split(',').next()?.trim().parse::<i32>().ok()
            })
        };
        let max = (max_of("0035")?, max_of("0036")?);
        block.contains("INPUT_PROP_DIRECT").then_some((device, max))
    })
}

/// `Override size` se houver, senão `Physical size`.
fn screen_size(output: &str) -> Option<(i32, i32)> {
    let size_of = |label: &str| {
        let (w, h) = output.lines().find_map(|l| l.strip_prefix(label))?.trim().split_once('x')?;
        Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
    };
    size_of("Override size:").or_else(|| size_of("Physical size:"))
}

/// Linha do `getevent -t`: instante, dispositivo, tipo, código e valor.
struct Raw {
    at: f64,
    device: String,
    kind: u16,
    code: u16,
    value: i64,
}

fn parse_raw(line: &str) -> Option<Raw> {
    let (time, rest) = line.trim().strip_prefix('[')?.split_once(']')?;
    let (device, fields) = rest.trim().split_once(": ")?;
    let mut fields = fields.split_whitespace();
    let mut hex = || u32::from_str_radix(fields.next()?, 16).ok();
    let (kind, code, value) = (hex()?, hex()?, hex()?);
    Some(Raw {
        at: time.trim().parse().ok()?,
        device: device.to_string(),
        kind: kind as u16,
        code: code as u16,
        value: value as i32 as i64,
    })
}

/// Grava toques e botões do celular até SUPER+F10 ou `stop`.
pub fn record(sender: &mpsc::Sender<RecordEvent>, stop: &AtomicBool) -> Result<Draft> {
    prepare(Duration::ZERO)?;
    let screen = Screen::query()?;
    let hotkeys = listen_input()?;
    if !countdown(sender, stop) {
        return Ok(Draft::default());
    }
    let mut child = adb()
        .args(["shell", "getevent", "-t"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context(ADB_MISSING)?;
    let stdout = child.stdout.take().context("sem saída do getevent")?;
    let (raw_sender, raws) = mpsc::channel();
    std::thread::spawn(move || {
        for raw in BufReader::new(stdout).lines().map_while(Result::ok).filter_map(|l| parse_raw(&l)) {
            if raw_sender.send(raw).is_err() {
                break;
            }
        }
    });
    let focus = watch_focus();
    let screen_reads = watch_screen();
    let _ = sender.send(RecordEvent::Started);

    let mut gestures = Gestures::new(&screen, sender);
    gestures.home = home_package();
    let mut super_held = false;
    'recording: while !stop.load(Ordering::Relaxed) {
        for (_, input) in hotkeys.try_iter() {
            let RawInput::Key { key, down } = input else { continue };
            if is_super(key) {
                super_held = down;
            } else if down && super_held && key == STOP_KEY {
                break 'recording;
            } else if down && super_held && key == PAUSE_KEY {
                gestures.toggle_pause();
            }
        }
        for (at, package) in focus.items.try_iter() {
            gestures.on_focus(at, &package);
        }
        if let Some(snapshot) = screen_reads.items.try_iter().last() {
            gestures.snapshot = Some(snapshot);
        }
        match raws.recv_timeout(STOP_POLL) {
            Ok(raw) => gestures.feed(&raw),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => bail!("o celular saiu do adb durante a gravação"),
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(gestures.draft)
}

/// Chama `read` até a feed ser descartada, esperando `interval` entre as chamadas.
fn poll_in_background<T: Send + 'static>(interval: Duration, mut read: impl FnMut() -> Option<T> + Send + 'static) -> Feed<T> {
    feed(move |sender, flag| {
        while !flag.load(Ordering::Relaxed) {
            if let Some(item) = read()
                && sender.send(item).is_err()
            {
                break;
            }
            std::thread::sleep(interval);
        }
    })
}

/// Avisa quando o app em primeiro plano muda.
fn watch_focus() -> Feed<(Instant, String)> {
    let mut last = String::new();
    poll_in_background(PHONE_FOCUS_POLL, move || {
        let package = foreground().ok().filter(|p| *p != last)?;
        last = package.clone();
        Some((Instant::now(), package))
    })
}

/// Tela lida pelo uiautomator, com o intervalo em que a leitura aconteceu.
struct Snapshot {
    start: Instant,
    end: Instant,
    nodes: Vec<UiNode>,
}

/// Lê a tela sem parar, para sugerir toques pelo texto.
fn watch_screen() -> Feed<Snapshot> {
    poll_in_background(Duration::ZERO, || {
        let start = Instant::now();
        let xml = dump_screen().ok()?;
        Some(Snapshot { start, end: Instant::now(), nodes: ui_nodes(&xml).collect() })
    })
}

/// Toque em andamento: início, ponto inicial, último ponto e se saiu do lugar.
struct Touching {
    since: f64,
    from: (i32, i32),
    /// Pontos do caminho, com o instante de cada um.
    path: Vec<(f64, (i32, i32))>,
    moved: bool,
}

impl Touching {
    fn reach(&mut self, at: f64, point: (i32, i32)) {
        let (dx, dy) = (point.0 - self.from.0, point.1 - self.from.1);
        self.moved |= dx * dx + dy * dy > PHONE_TAP_SLOP * PHONE_TAP_SLOP;
        if self.path.last().is_none_or(|(_, last)| *last != point) {
            self.path.push((at, point));
        }
    }

    /// Passo do toque terminado em `at`: toque, toque longo ou arrasto pelo caminho simplificado.
    fn finish(self, at: f64) -> PhoneAction {
        let duration = round_recorded(Duration::from_secs_f64((at - self.since).max(0.0)));
        if !self.moved {
            let hold = if duration >= PHONE_LONG_PRESS { duration } else { Duration::ZERO };
            return PhoneAction::Tap { at: self.from, hold };
        }
        let since = |t: f64| Duration::from_secs_f64((t - self.since).max(0.0));
        let trail: Vec<Waypoint> = self.path.iter().map(|(t, point)| (since(*t), *point)).collect();
        let kept = simplify(&trail, PHONE_PATH_TOLERANCE);
        let via = kept[1..kept.len() - 1].iter().map(|&i| (round_ms(trail[i].0), trail[i].1)).collect();
        let to = trail.last().map_or(self.from, |(_, point)| *point);
        PhoneAction::Swipe { from: self.from, via, to, duration }
    }
}

/// Corta abaixo do milissegundo, para o `via` ficar legível.
fn round_ms(duration: Duration) -> Duration {
    Duration::from_millis(duration.as_millis() as u64)
}

/// Junta os eventos crus em passos `phone`, com a pausa antes de cada um.
struct Gestures<'a> {
    screen: &'a Screen,
    sender: &'a mpsc::Sender<RecordEvent>,
    slot: i64,
    position: (Option<i32>, Option<i32>),
    pressed: bool,
    touching: Option<Touching>,
    last_end: Option<f64>,
    paused: bool,
    draft: Draft,
    /// Fim de cada trecho no relógio do PC, para casar com as trocas de app.
    ends: Vec<Instant>,
    /// Pacote da tela inicial e o primeiro trecho feito nela.
    home: Option<String>,
    home_since: Option<usize>,
    /// Última leitura da tela, para sugerir o toque pelo texto.
    snapshot: Option<Snapshot>,
}

impl<'a> Gestures<'a> {
    fn new(screen: &'a Screen, sender: &'a mpsc::Sender<RecordEvent>) -> Self {
        Self {
            screen,
            sender,
            slot: 0,
            position: (None, None),
            pressed: false,
            touching: None,
            last_end: None,
            paused: false,
            draft: Draft::default(),
            ends: Vec::new(),
            home: None,
            home_since: None,
            snapshot: None,
        }
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        self.touching = None;
        self.last_end = None;
        let _ = self.sender.send(RecordEvent::Paused(self.paused));
    }

    fn feed(&mut self, raw: &Raw) {
        if raw.device != self.screen.device {
            if raw.kind == EV_KEY && raw.value == 1
                && let Some((_, name)) = KEYS.iter().find(|(code, _)| *code == raw.code)
            {
                self.push(raw.at, raw.at, PhoneAction::Key((*name).to_string()));
            }
            return;
        }
        match (raw.kind, raw.code) {
            (EV_ABS, ABS_MT_SLOT) => self.slot = raw.value,
            (EV_ABS, ABS_MT_POSITION_X) if self.slot == 0 => self.position.0 = Some(raw.value as i32),
            (EV_ABS, ABS_MT_POSITION_Y) if self.slot == 0 => self.position.1 = Some(raw.value as i32),
            (EV_KEY, BTN_TOUCH) => self.pressed = raw.value == 1,
            (EV_SYN, 0) => self.frame(raw.at),
            _ => {}
        }
    }

    /// Fim de um quadro do sensor: começa, acompanha ou termina o toque.
    fn frame(&mut self, at: f64) {
        let point = match self.position {
            (Some(x), Some(y)) => self.screen.to_pixels((x, y)),
            _ => return,
        };
        match (self.pressed, self.touching.take()) {
            (true, None) => {
                self.touching = Some(Touching { since: at, from: point, path: vec![(at, point)], moved: false });
            }
            (true, Some(mut touch)) => {
                touch.reach(at, point);
                self.touching = Some(touch);
            }
            (false, Some(mut touch)) => {
                touch.reach(at, point);
                let since = touch.since;
                self.push(since, at, touch.finish(at));
            }
            (false, None) => {}
        }
    }

    fn push(&mut self, start: f64, end: f64, action: PhoneAction) {
        if self.paused {
            return;
        }
        let pause = self
            .last_end
            .map(|last| Duration::from_secs_f64((start - last).max(0.0)))
            .filter(|gap| *gap >= RECORD_MIN_SLEEP)
            .map(round_recorded);
        self.last_end = Some(end);
        let now = Instant::now();
        let started = now.checked_sub(Duration::from_secs_f64((end - start).max(0.0))).unwrap_or(now);
        let suggestion = self.suggest(&action, started);
        let step = Step::Phone(action).to_string();
        let _ = self.sender.send(RecordEvent::Step(step.clone()));
        match suggestion {
            Some(suggested) => self.draft.push_choice(pause, "pelo texto", suggested, step),
            None => self.draft.push_recorded(pause, step),
        }
        self.ends.push(now);
    }

    /// `phone tap text=...` para um toque, se a tela foi lida inteira entre o passo anterior e este.
    fn suggest(&self, action: &PhoneAction, started: Instant) -> Option<String> {
        let PhoneAction::Tap { at, hold } = action else { return None };
        let snapshot = self.snapshot.as_ref()?;
        let fresh = self.ends.last().is_none_or(|previous| snapshot.start >= *previous) && snapshot.end <= started;
        let target = suggest_target(&snapshot.nodes, *at).filter(|_| fresh)?;
        Some(Step::Phone(PhoneAction::TapOn { target, hold: *hold, timeout: DEFAULT_WAIT_TIMEOUT }).to_string())
    }

    /// Troca de app: os toques feitos na tela inicial até abrir um app viram `phone open`.
    fn on_focus(&mut self, at: Instant, package: &str) {
        if self.paused || package.is_empty() {
            return;
        }
        if self.home.as_deref() == Some(package) {
            self.home_since = Some(self.draft.items.len());
            return;
        }
        let Some(from) = self.home_since.take() else { return };
        let to = self.ends.iter().take_while(|end| **end <= at).count();
        if to > from {
            let open = Step::Phone(PhoneAction::Open { package: package.to_string(), timeout: DEFAULT_WAIT_TIMEOUT });
            self.draft.group(from, to, "abrir app", open.to_string());
            self.ends.drain(from..to - 1);
            let _ = self.sender.send(RecordEvent::Desktop(format!("abriu {package}")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_touchscreen_and_size() {
        let listing = "add device 7: /dev/input/event6\n  name:     \"sec_touchpad\"\n  events:\n    ABS (0003): 0035  : value 0, min 0, max 4095\n                0036  : value 0, min 0, max 4095\n  input props:\n    INPUT_PROP_POINTER\nadd device 8: /dev/input/event5\n  name:     \"sec_touchscreen\"\n  events:\n    ABS (0003): 0000  : value 0, min 0, max 4095\n                0035  : value 0, min 0, max 4095\n                0036  : value 0, min 0, max 8191\n  input props:\n    INPUT_PROP_DIRECT\n";
        assert_eq!(touch_device(listing), Some(("/dev/input/event5".into(), (4095, 8191))));
        assert_eq!(screen_size("Physical size: 1080x2340\nOverride size: 720x1560\n"), Some((720, 1560)));
        assert_eq!(screen_size("Physical size: 1080x2340\n"), Some((1080, 2340)));
        let raw = parse_raw("[   12345.678901] /dev/input/event5: 0003 0039 ffffffff").unwrap();
        assert_eq!((raw.kind, raw.code, raw.value), (EV_ABS, 0x39, -1));
    }

    #[test]
    fn touches_become_taps_presses_and_swipes() {
        let screen = Screen { device: "/dev/input/event5".into(), max: (4095, 4095), size: (1080, 2340) };
        let (sender, _events) = mpsc::channel();
        let mut gestures = Gestures::new(&screen, &sender);
        let screen_event = |at: f64, kind: u16, code: u16, value: i64| Raw { at, device: "/dev/input/event5".into(), kind, code, value };
        let mut touch = |at: f64, x: i64, y: i64, down: bool| {
            for raw in [
                screen_event(at, EV_ABS, ABS_MT_POSITION_X, x),
                screen_event(at, EV_ABS, ABS_MT_POSITION_Y, y),
                screen_event(at, EV_KEY, BTN_TOUCH, down as i64),
                screen_event(at, EV_SYN, 0, 0),
            ] {
                gestures.feed(&raw);
            }
        };
        touch(1.0, 2048, 2048, true);
        touch(1.08, 2050, 2049, false);
        touch(2.0, 1000, 3000, true);
        touch(2.9, 1000, 3000, false);
        touch(4.0, 2048, 3500, true);
        touch(4.15, 2048, 2500, true);
        touch(4.3, 2048, 1000, false);
        gestures.feed(&Raw { at: 5.0, device: "/dev/input/event0".into(), kind: EV_KEY, code: 158, value: 1 });
        let steps: Vec<String> = gestures.draft.items.iter().map(|i| i.current().steps.join(" | ")).collect();
        assert_eq!(steps, vec![
            "phone tap 539,1169",
            "phone tap 263,1713 duration=900ms",
            "phone swipe from=539,1999 via=539,1427@150ms to=539,571 duration=300ms",
            "phone key BACK",
        ]);
        assert_eq!(gestures.draft.items[1].pause, Some(Duration::from_millis(1000)), "pausa entre os toques");
    }

    #[test]
    fn taps_on_home_until_an_app_opens_become_open() {
        let screen = Screen { device: "/dev/input/event5".into(), max: (1079, 2339), size: (1080, 2340) };
        let (sender, _events) = mpsc::channel();
        let mut gestures = Gestures::new(&screen, &sender);
        gestures.home = Some("com.sec.android.app.launcher".into());
        let tap = |gestures: &mut Gestures, at: f64, x: i64, y: i64| {
            for down in [true, false] {
                for (kind, code, value) in
                    [(EV_ABS, ABS_MT_POSITION_X, x), (EV_ABS, ABS_MT_POSITION_Y, y), (EV_KEY, BTN_TOUCH, down as i64), (EV_SYN, 0, 0)]
                {
                    gestures.feed(&Raw { at: at + if down { 0.0 } else { 0.05 }, device: "/dev/input/event5".into(), kind, code, value });
                }
            }
        };
        tap(&mut gestures, 1.0, 100, 100);
        gestures.on_focus(Instant::now(), "com.sec.android.app.launcher");
        tap(&mut gestures, 2.0, 540, 2000);
        tap(&mut gestures, 3.0, 300, 900);
        gestures.on_focus(Instant::now(), "br.com.brainweb.ifood");
        tap(&mut gestures, 5.0, 500, 500);

        let items = &gestures.draft.items;
        assert_eq!(items.len(), 3);
        assert_eq!(items[1].current().steps, vec!["phone open br.com.brainweb.ifood"]);
        assert_eq!(items[1].variants[1].steps, vec!["phone tap 540,2000", "sleep 1s", "phone tap 300,900"]);
        assert_eq!(items[2].current().steps, vec!["phone tap 500,500"]);
        assert_eq!(gestures.ends.len(), items.len());
    }

    #[test]
    fn finds_screen_elements() {
        let xml = r#"<hierarchy rotation="0"><node index="0" text="" resource-id="" content-desc="" bounds="[0,0][1080,2340]"><node index="1" text="Restaurantes &amp; mercados" resource-id="br.com.brainweb.ifood:id/tab" content-desc="" bounds="[100,200][300,260]" /><node text="" resource-id="cart" content-desc="Ver sacola" bounds="[0,2200][1080,2340]" /><node text="Escondido" resource-id="" content-desc="" bounds="[0,0][0,0]" />"#;
        let nodes: Vec<UiNode> = ui_nodes(xml).collect();
        assert_eq!(nodes.len(), 3, "o de área zero fica de fora");
        assert_eq!(nodes[1].text, "Restaurantes & mercados");
        assert_eq!(nodes[1].center(), (200, 230));
        assert_eq!(nodes[1].id, "br.com.brainweb.ifood:id/tab");
        assert_eq!(nodes[2].description, "Ver sacola");
    }

    #[test]
    fn suggests_the_smallest_labeled_element_that_is_unique() {
        let node = |text: &str, description: &str, id: &str, bounds| UiNode {
            text: text.into(),
            description: description.into(),
            id: id.into(),
            bounds,
        };
        let nodes = vec![
            node("", "", "", (0, 0, 1080, 2340)),
            node("Restaurantes", "", "", (0, 100, 540, 200)),
            node("Restaurantes", "", "tab", (0, 2200, 540, 2340)),
            node("", "Ver sacola (2)", "", (600, 2200, 1080, 2340)),
            node("", "", "br.com.x:id/banner", (0, 300, 1080, 900)),
        ];
        let text = |target: Option<UiTarget>| target.map(|t| t.to_string());
        assert_eq!(text(suggest_target(&nodes, (100, 150))), Some("text=Restaurantes".into()));
        assert_eq!(text(suggest_target(&nodes, (100, 2300))), Some("id=^tab$".into()), "o texto repete, então vai pelo id");
        assert_eq!(text(suggest_target(&nodes, (700, 2300))).as_deref(), Some(r#"text="Ver sacola \\(2\\)""#));
        assert_eq!(text(suggest_target(&nodes, (500, 500))).as_deref(), Some(r#"id="^br\\.com\\.x:id/banner$""#));
        assert_eq!(text(suggest_target(&nodes, (500, 1500))), None, "só o fundo sem rótulo");
    }

    #[test]
    fn trace_replays_the_path() {
        let via = vec![(Duration::from_millis(200), (540, 1500))];
        assert_eq!(
            trace((200, 1500), &via, (540, 1800), Duration::from_millis(500)),
            "input motionevent DOWN 200 1500; sleep 0.170; input motionevent MOVE 540 1500; sleep 0.270; \
             input motionevent MOVE 540 1800; input motionevent UP 540 1800"
        );
        let line = "  mFocusedApp=ActivityRecord{48808496 u0 br.com.brainweb.ifood/.AppIconClub t8386}";
        assert_eq!(focused_package(line), Some("br.com.brainweb.ifood".into()));
        assert_eq!(focused_package("  mFocusedApp=null"), None);
    }
}
