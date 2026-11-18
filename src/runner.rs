use std::fs::{File, TryLockError};
use std::process::{Child, Command, Stdio};
use std::time::Instant;

use anyhow::{Context, Result, bail};

use crate::config::Config;
use crate::constants::RUNNING_DIR;
use crate::history::{self, Run};
use crate::platform::{self, Desktop};
use crate::player::Player;
use crate::progress;
use crate::script::{Arg, Line};
use crate::utils::{log, notify, runtime_dir, timestamp};

/// Quais linhas executar, para depuração.
#[derive(Debug, Clone, Copy, Default)]
pub enum Selection {
    #[default]
    All,
    From(usize),
    Only(usize),
}

impl Selection {
    /// Seleção a partir das flags `--from` e `--only`.
    pub fn from_flags(from: Option<usize>, only: Option<usize>) -> Self {
        match (from, only) {
            (_, Some(n)) => Self::Only(n),
            (Some(n), None) => Self::From(n),
            (None, None) => Self::All,
        }
    }

    fn apply(self, lines: Vec<Line>) -> Result<Vec<Line>> {
        let picked: Vec<Line> = match self {
            Self::All => return Ok(lines),
            Self::From(n) => lines.into_iter().filter(|l| l.number >= n).collect(),
            Self::Only(n) => lines.into_iter().filter(|l| l.number == n).collect(),
        };
        anyhow::ensure!(!picked.is_empty(), "nenhum passo a partir da seleção");
        Ok(picked)
    }

    /// Sufixo para o rótulo do gatilho no histórico.
    fn label(self) -> Option<String> {
        match self {
            Self::All => None,
            Self::From(n) => Some(format!("a partir do {n}")),
            Self::Only(n) => Some(format!("só o {n}")),
        }
    }

    fn args(self) -> Vec<String> {
        match self {
            Self::All => Vec::new(),
            Self::From(n) => vec!["--from".into(), n.to_string()],
            Self::Only(n) => vec!["--only".into(), n.to_string()],
        }
    }
}

/// Executa uma rotina neste processo e registra no histórico.
pub fn run_routine(config: &Config, name: &str, trigger: &str, selection: Selection, args: &[Arg]) -> Result<()> {
    let routine = config
        .find(name)
        .with_context(|| format!("rotina '{name}' não existe"))?;
    let lines = selection.apply(routine.lines(args)?)?;
    let _guard = RunGuard::acquire(name)?;
    let desktop = Desktop::connect();
    let trigger = match selection.label() {
        Some(label) => format!("{trigger} ({label})"),
        None => trigger.to_string(),
    };

    log(&format!("[{name}] início ({trigger})"));
    let started = timestamp();
    let clock = Instant::now();
    let failure = Player::new(config, &desktop).play(name, &lines).err();
    progress::clear(name);
    let error = failure.as_ref().map(|f| format!("{:#}", f.error));

    let _ = history::append(&Run {
        routine: name.to_string(),
        trigger: trigger.to_string(),
        started,
        duration_ms: clock.elapsed().as_millis() as u64,
        ok: error.is_none(),
        error: error.clone(),
        failed_line: failure.as_ref().map(|f| f.line),
    });
    match failure {
        Some(failure) => {
            let err = error.unwrap_or_default();
            log(&format!("[{name}] falhou: {err}"));
            notify("pcue", &format!("Rotina {name} falhou: {err}"));
            Err(failure.error.context(format!("linha {}", failure.line)))
        }
        None => {
            log(&format!("[{name}] concluída"));
            Ok(())
        }
    }
}

/// Executa a rotina num processo filho do próprio binário.
pub fn spawn_run(
    routine: &str,
    trigger: &str,
    delay_ms: u64,
    selection: Selection,
    args: &[Arg],
) -> std::io::Result<Child> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["run", routine, "--trigger", trigger, "--delay-ms", &delay_ms.to_string()])
        .args(selection.args())
        .arg("--")
        .args(args.iter().map(|(name, value)| format!("{name}={value}")))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    platform::background(&mut command);
    command.spawn()
}

/// Trava por rotina; o kernel solta quando o processo termina.
struct RunGuard {
    _file: File,
}

impl RunGuard {
    fn acquire(name: &str) -> Result<Self> {
        let dir = runtime_dir()?.join(RUNNING_DIR);
        std::fs::create_dir_all(&dir)?;
        let file = File::create(dir.join(format!("{name}.lock")))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => bail!("rotina '{name}' já está em execução"),
            Err(TryLockError::Error(err)) => Err(err.into()),
        }
    }
}
