use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, anyhow, bail};
use jiff::Zoned;

use crate::config::{Config, Trigger, validate_name};
use crate::constants::{DAEMON_TICK, EVENT_DEDUP_WINDOW};
use crate::runner::spawn_run;
use crate::schedule::{Weekday, next_daily};
use crate::script::parse_duration;
use crate::utils::{config_path, daemon_socket, log};

/// Processo de fundo: dispara rotinas por evento e horário.
pub fn run_daemon(restarted: bool) -> Result<()> {
    let binary = Binary::current()?;
    let socket = daemon_socket()?;
    if is_running() {
        bail!("o daemon já está rodando");
    }
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("falha ao abrir {}", socket.display()))?;

    let (sender, receiver) = mpsc::channel();
    spawn_watcher("ipc", sender.clone(), move |tx| serve_ipc(listener, tx));
    spawn_watcher("logind", sender.clone(), watch_logind);
    if crate::hypr::in_session() {
        spawn_watcher("hyprland", sender.clone(), watch_hyprland);
        spawn_watcher("bloqueio", sender.clone(), watch_lock);
    }
    drop(sender);

    let path = config_path()?;
    let mut daemon = Daemon::new(path)?;
    if restarted {
        log("daemon reiniciado com o binário novo");
    } else {
        log("daemon iniciado");
        std::thread::sleep(Duration::from_millis(daemon.config.settings.startup_delay_ms));
        daemon.fire_event("login");
    }

    loop {
        match receiver.recv_timeout(DAEMON_TICK) {
            Ok(name) => daemon.fire_event(&name),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => bail!("todas as fontes de evento pararam"),
        }
        daemon.reload_if_changed();
        daemon.tick_schedules();
        daemon.reap();
        if binary.changed() {
            log("binário atualizado, reiniciando o daemon");
            return Err(binary.restart());
        }
    }
}

/// Executável do daemon, para se reiniciar quando for atualizado.
struct Binary {
    path: PathBuf,
    modified: Option<SystemTime>,
}

impl Binary {
    fn current() -> Result<Self> {
        let path = std::env::current_exe()?;
        let modified = mtime(&path);
        Ok(Self { path, modified })
    }

    fn changed(&self) -> bool {
        let now = mtime(&self.path);
        now.is_some() && now != self.modified
    }

    /// Substitui o processo pelo binário novo; só retorna em caso de erro.
    fn restart(&self) -> anyhow::Error {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&self.path)
            .args(["daemon", "--restarted"])
            .exec();
        anyhow!("falha ao reiniciar o daemon: {err}")
    }
}

/// O daemon responde no socket?
pub fn is_running() -> bool {
    daemon_socket().is_ok_and(|s| UnixStream::connect(s).is_ok())
}

/// Envia um evento ao daemon.
pub fn emit(event: &str) -> Result<()> {
    validate_name(event)?;
    let mut stream = UnixStream::connect(daemon_socket()?)
        .map_err(|_| anyhow!("daemon não está rodando (pwboot daemon)"))?;
    writeln!(stream, "emit {event}")?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    if reply.trim() != "ok" {
        bail!("daemon recusou: {}", reply.trim());
    }
    Ok(())
}

/// Próximo disparo de um gatilho de tempo.
enum Due {
    At { next: Zoned, at: String, days: Vec<Weekday> },
    Every { next: Instant, every: Duration },
}

impl Due {
    /// Disparo inicial; eventos não têm horário.
    fn first(trigger: &Trigger, now: &Zoned) -> Option<Self> {
        match trigger {
            Trigger::Daily { at, days } => Some(Due::At {
                next: next_daily(at, days, now).ok()?,
                at: at.clone(),
                days: days.clone(),
            }),
            Trigger::Interval { every } => {
                let every = parse_duration(every).ok()?;
                Some(Due::Every {
                    next: Instant::now() + every,
                    every,
                })
            }
            Trigger::Event { .. } => None,
        }
    }

    fn is_due(&self, now: &Zoned, instant: Instant) -> bool {
        match self {
            Due::At { next, .. } => next <= now,
            Due::Every { next, .. } => *next <= instant,
        }
    }

    /// Reagenda a partir de agora; `false` se não houver próximo.
    fn advance(&mut self, now: &Zoned, instant: Instant) -> bool {
        match self {
            Due::At { next, at, days } => match next_daily(at, days, now) {
                Ok(following) => {
                    *next = following;
                    true
                }
                Err(_) => false,
            },
            Due::Every { next, every } => {
                *next = instant + *every;
                true
            }
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Due::At { .. } => "horário",
            Due::Every { .. } => "intervalo",
        }
    }
}

/// Estado do agendador.
struct Daemon {
    path: PathBuf,
    config: Config,
    modified: Option<SystemTime>,
    schedule: Vec<(String, Due)>,
    last_event: HashMap<String, Instant>,
    children: Vec<Child>,
}

impl Daemon {
    fn new(path: PathBuf) -> Result<Self> {
        let config = Config::load(&path)?;
        let mut daemon = Self {
            modified: mtime(&path),
            path,
            config,
            schedule: Vec::new(),
            last_event: HashMap::new(),
            children: Vec::new(),
        };
        daemon.plan();
        Ok(daemon)
    }

    /// Recalcula os próximos disparos a partir de agora.
    fn plan(&mut self) {
        let now = Zoned::now();
        self.schedule = self
            .config
            .routines
            .iter()
            .filter(|r| r.enabled)
            .flat_map(|r| {
                r.triggers
                    .iter()
                    .filter_map(|t| Due::first(t, &now))
                    .map(|due| (r.name.clone(), due))
            })
            .collect();
    }

    /// Relê a config quando o arquivo muda; config inválida mantém a anterior.
    fn reload_if_changed(&mut self) {
        let modified = mtime(&self.path);
        if modified == self.modified {
            return;
        }
        self.modified = modified;
        match Config::load(&self.path) {
            Ok(config) => {
                self.config = config;
                self.plan();
                log("config recarregada");
            }
            Err(err) => log(&format!("config inválida, mantendo a anterior: {err:#}")),
        }
    }

    /// Dispara rotinas do evento, ignorando repetições próximas.
    fn fire_event(&mut self, event: &str) {
        let now = Instant::now();
        if self
            .last_event
            .get(event)
            .is_some_and(|t| now.duration_since(*t) < EVENT_DEDUP_WINDOW)
        {
            return;
        }
        self.last_event.insert(event.to_string(), now);
        log(&format!("evento {event}"));
        let names: Vec<String> = self
            .config
            .routines
            .iter()
            .filter(|r| r.listens_to(event))
            .map(|r| r.name.clone())
            .collect();
        for name in names {
            self.spawn(&name, &format!("evento {event}"));
        }
    }

    /// Dispara horários e intervalos vencidos e reagenda.
    fn tick_schedules(&mut self) {
        let now = Zoned::now();
        let instant = Instant::now();
        let mut fired = Vec::new();
        self.schedule.retain_mut(|(name, due)| {
            if !due.is_due(&now, instant) {
                return true;
            }
            fired.push((name.clone(), due.label()));
            due.advance(&now, instant)
        });
        for (name, label) in fired {
            self.spawn(&name, label);
        }
    }

    /// Roda a rotina num processo filho.
    fn spawn(&mut self, routine: &str, trigger: &str) {
        match spawn_run(routine, trigger, 0) {
            Ok(child) => self.children.push(child),
            Err(err) => log(&format!("[{routine}] falha ao iniciar: {err}")),
        }
    }

    /// Recolhe filhos que terminaram.
    fn reap(&mut self) {
        self.children
            .retain_mut(|child| matches!(child.try_wait(), Ok(None)));
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Roda um observador em thread própria, registrando a falha no log.
fn spawn_watcher<F>(name: &'static str, sender: Sender<String>, watcher: F)
where
    F: FnOnce(Sender<String>) -> Result<()> + Send + 'static,
{
    std::thread::spawn(move || {
        if let Err(err) = watcher(sender) {
            log(&format!("observador {name} parou: {err:#}"));
        }
    });
}

/// Atende `emit <evento>` e `ping` no socket.
fn serve_ipc(listener: UnixListener, sender: Sender<String>) -> Result<()> {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let mut line = String::new();
        if BufReader::new(&stream).read_line(&mut line).is_err() {
            continue;
        }
        let reply = match line.trim().split_once(' ') {
            Some(("emit", event)) if validate_name(event).is_ok() => {
                sender.send(event.to_string())?;
                "ok".to_string()
            }
            _ if line.trim() == "ping" => "ok".to_string(),
            _ => format!("comando inválido: {}", line.trim()),
        };
        let _ = writeln!(&stream, "{reply}");
    }
    Ok(())
}

/// Bloqueio, desbloqueio e retorno da suspensão pelo logind.
fn watch_logind(sender: Sender<String>) -> Result<()> {
    use zbus::blocking::{Connection, MessageIterator};
    use zbus::zvariant::OwnedValue;

    let connection = Connection::system()?;
    let session = current_session(&connection);
    if session.is_none() {
        log("sessão do logind não encontrada: lock/unlock indisponíveis, resume segue ativo");
    }
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.login1")?
        .build();

    let mut locked_hint = false;
    for message in MessageIterator::for_match_rule(rule, &connection, None)? {
        let message = message?;
        let header = message.header();
        let member = header.member().map(|m| m.as_str()).unwrap_or_default();
        let ours = header
            .path()
            .zip(session.as_ref())
            .is_some_and(|(path, session)| path.as_str() == session.as_str());
        let event = match member {
            "PrepareForSleep" => {
                let sleeping: bool = message.body().deserialize()?;
                (!sleeping).then_some("resume")
            }
            "Lock" if ours => Some("lock"),
            "Unlock" if ours => Some("unlock"),
            "PropertiesChanged" if ours => {
                let (_, changed, _): (String, HashMap<String, OwnedValue>, Vec<String>) =
                    message.body().deserialize()?;
                changed
                    .get("LockedHint")
                    .and_then(|v| bool::try_from(v).ok())
                    .filter(|hint| *hint != locked_hint)
                    .map(|hint| {
                        locked_hint = hint;
                        if hint { "lock" } else { "unlock" }
                    })
            }
            _ => None,
        };
        if let Some(event) = event {
            sender.send(event.to_string())?;
        }
    }
    Ok(())
}

/// Caminho D-Bus da sessão: pelo PID e, se falhar, por `XDG_SESSION_ID`.
fn current_session(
    connection: &zbus::blocking::Connection,
) -> Option<zbus::zvariant::OwnedObjectPath> {
    const DEST: Option<&str> = Some("org.freedesktop.login1");
    const PATH: &str = "/org/freedesktop/login1";
    const IFACE: Option<&str> = Some("org.freedesktop.login1.Manager");
    let by_pid = connection.call_method(DEST, PATH, IFACE, "GetSessionByPID", &std::process::id());
    let reply = match by_pid {
        Ok(reply) => reply,
        Err(_) => {
            let id = std::env::var("XDG_SESSION_ID").ok()?;
            connection.call_method(DEST, PATH, IFACE, "GetSession", &id).ok()?
        }
    };
    reply.body().deserialize().ok()
}

/// Bloqueio e desbloqueio pelo estado de session lock do Hyprland.
fn watch_lock(sender: Sender<String>) -> Result<()> {
    let hypr = crate::hypr::Hypr::connect()?;
    let mut locked = hypr.is_locked()?;
    loop {
        std::thread::sleep(DAEMON_TICK);
        let now = hypr.is_locked()?;
        if now != locked {
            locked = now;
            sender.send(if now { "lock" } else { "unlock" }.to_string())?;
        }
    }
}

/// Monitores conectados e desconectados pelo socket de eventos do Hyprland.
fn watch_hyprland(sender: Sender<String>) -> Result<()> {
    let stream = UnixStream::connect(crate::hypr::event_socket()?)?;
    for line in BufReader::new(stream).lines() {
        let line = line?;
        let event = match line.split_once(">>").map(|(name, _)| name) {
            Some("monitoradded" | "monitoraddedv2") => "monitor_added",
            Some("monitorremoved" | "monitorremovedv2") => "monitor_removed",
            _ => continue,
        };
        sender.send(event.to_string())?;
    }
    Ok(())
}
