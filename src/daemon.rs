use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, anyhow, bail};
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{Listener, ListenerOptions, Stream};
use jiff::Zoned;

use crate::config::{Config, Trigger, validate_name};
use crate::constants::{DAEMON_HANDOFF, DAEMON_TICK, EVENT_DEDUP_WINDOW, NEXT_RUNS_FILE};
use crate::runner::{Selection, spawn_run};
use crate::schedule::{Weekday, next_daily};
use crate::script::parse_duration;
use crate::platform::{self, daemon_endpoint};
use crate::utils::{config_path, log, read_json, runtime_dir, write_atomic};

/// Processo de fundo: dispara rotinas por evento e horário.
pub fn run_daemon(restarted: bool) -> Result<()> {
    let binary = Binary::current()?;
    let args: &[&str] = if restarted { &["daemon", "--restarted"] } else { &["daemon"] };
    if platform::detach_daemon(args)? {
        return Ok(());
    }
    wait_previous(restarted);
    if is_running() {
        bail!("o daemon já está rodando");
    }
    platform::clear_endpoint();
    let listener = ListenerOptions::new()
        .name(daemon_endpoint()?)
        .create_sync()
        .context("falha ao abrir o canal do daemon")?;

    let (sender, receiver) = mpsc::channel();
    spawn_watcher("ipc", sender.clone(), move |tx| serve_ipc(listener, tx));
    for (name, source) in platform::event_sources() {
        spawn_watcher(name, sender.clone(), source);
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

    /// Troca o processo pelo binário novo; só retorna em caso de erro.
    fn restart(&self) -> anyhow::Error {
        platform::restart_self(&self.path, &["daemon", "--restarted"])
    }
}

/// Depois de uma atualização, dá tempo do daemon antigo sair.
fn wait_previous(restarted: bool) {
    let deadline = Instant::now() + DAEMON_HANDOFF;
    while restarted && is_running() && Instant::now() < deadline {
        std::thread::sleep(DAEMON_TICK / 4);
    }
}

/// O daemon responde no socket?
pub fn is_running() -> bool {
    daemon_endpoint().is_ok_and(|name| Stream::connect(name).is_ok())
}

/// Envia um evento ao daemon.
pub fn emit(event: &str) -> Result<()> {
    validate_name(event)?;
    let stream = Stream::connect(daemon_endpoint()?)
        .map_err(|_| anyhow!("daemon não está rodando (pcue daemon)"))?;
    let mut reader = BufReader::new(stream);
    writeln!(reader.get_mut(), "emit {event}")?;
    let mut reply = String::new();
    reader.read_line(&mut reply)?;
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

    /// Momento do próximo disparo, em segundos Unix.
    fn next_unix(&self, now: &Zoned, instant: Instant) -> i64 {
        match self {
            Due::At { next, .. } => next.timestamp().as_second(),
            Due::Every { next, .. } => {
                now.timestamp().as_second() + next.saturating_duration_since(instant).as_secs() as i64
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

/// Próximo disparo agendado de cada rotina, em segundos Unix.
pub fn next_runs() -> HashMap<String, i64> {
    runtime_dir()
        .ok()
        .and_then(|dir| read_json(&dir.join(NEXT_RUNS_FILE)))
        .unwrap_or_default()
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
        self.publish_next();
    }

    /// Grava o próximo disparo de cada rotina para a TUI.
    fn publish_next(&self) {
        let (now, instant) = (Zoned::now(), Instant::now());
        let mut next: HashMap<&str, i64> = HashMap::new();
        for (name, due) in &self.schedule {
            let at = due.next_unix(&now, instant);
            next.entry(name).and_modify(|t| *t = (*t).min(at)).or_insert(at);
        }
        let result = serde_json::to_vec(&next)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| write_atomic(&runtime_dir()?.join(NEXT_RUNS_FILE), &bytes));
        if let Err(err) = result {
            log(&format!("falha ao publicar próximos disparos: {err:#}"));
        }
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
        if !fired.is_empty() {
            self.publish_next();
        }
        for (name, label) in fired {
            self.spawn(&name, label);
        }
    }

    /// Roda a rotina num processo filho.
    fn spawn(&mut self, routine: &str, trigger: &str) {
        match spawn_run(routine, trigger, 0, Selection::All) {
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
fn spawn_watcher<F>(name: &str, sender: Sender<String>, watcher: F)
where
    F: FnOnce(Sender<String>) -> Result<()> + Send + 'static,
{
    let name = name.to_string();
    std::thread::spawn(move || {
        if let Err(err) = watcher(sender) {
            log(&format!("observador {name} parou: {err:#}"));
        }
    });
}

/// Atende `emit <evento>` e `ping` no socket.
fn serve_ipc(listener: Listener, sender: Sender<String>) -> Result<()> {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
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
        let _ = writeln!(reader.get_mut(), "{reply}");
    }
    Ok(())
}
