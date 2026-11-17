use std::fs::{File, TryLockError};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::config::Config;
use crate::history::{self, Run};
use crate::hypr::Hypr;
use crate::player::play;
use crate::script::parse_steps;
use crate::utils::{log, notify, runtime_dir, timestamp};

/// Executa uma rotina neste processo e registra no histórico.
pub fn run_routine(config: &Config, name: &str, trigger: &str) -> Result<()> {
    let routine = config
        .find(name)
        .with_context(|| format!("rotina '{name}' não existe"))?;
    let lines = parse_steps(&routine.steps)?;
    let _guard = RunGuard::acquire(name)?;
    let hypr = Hypr::detect();
    let timeout = Duration::from_secs(config.settings.timeout_secs);

    log(&format!("[{name}] início ({trigger})"));
    let started = timestamp();
    let clock = Instant::now();
    let result = play(name, &lines, hypr.as_ref(), timeout);
    let error = result.as_ref().err().map(|e| format!("{e:#}"));

    let _ = history::append(&Run {
        routine: name.to_string(),
        trigger: trigger.to_string(),
        started,
        duration_ms: clock.elapsed().as_millis() as u64,
        ok: error.is_none(),
        error: error.clone(),
    });
    match error {
        None => log(&format!("[{name}] concluída")),
        Some(err) => {
            log(&format!("[{name}] falhou: {err}"));
            notify("pcue", &format!("Rotina {name} falhou: {err}"));
        }
    }
    result
}

/// Executa a rotina num processo filho do próprio binário.
pub fn spawn_run(routine: &str, trigger: &str, delay_ms: u64) -> std::io::Result<Child> {
    Command::new(std::env::current_exe()?)
        .args(["run", routine, "--trigger", trigger, "--delay-ms", &delay_ms.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// Trava por rotina; o kernel solta quando o processo termina.
struct RunGuard {
    _file: File,
}

impl RunGuard {
    fn acquire(name: &str) -> Result<Self> {
        let dir = runtime_dir()?.join("running");
        std::fs::create_dir_all(&dir)?;
        let file = File::create(dir.join(format!("{name}.lock")))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => bail!("rotina '{name}' já está em execução"),
            Err(TryLockError::Error(err)) => Err(err.into()),
        }
    }
}
