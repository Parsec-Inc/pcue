use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::constants::{CONFIG_FILE, HISTORY_FILE, LOG_FILE};
use crate::platform;

/// Roda `work` numa thread; o resultado, se houver, chega pelo canal.
pub fn in_background<T: Send + 'static>(work: impl FnOnce() -> Option<T> + Send + 'static) -> std::sync::mpsc::Receiver<T> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Some(value) = work() {
            let _ = sender.send(value);
        }
    });
    receiver
}

pub use crate::platform::{notify, runtime_dir};

/// Diretório home do usuário.
pub fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("variável HOME não definida")
}

/// Grava via arquivo temporário + rename, sem leitor ver arquivo pela metade.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// Lê um JSON; ausente ou inválido vira `None`.
pub fn read_json<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Troca o `~` inicial pelo home.
pub fn expand_home(path: &str) -> Result<PathBuf> {
    match path.strip_prefix("~/") {
        Some(rest) => Ok(home_dir()?.join(rest)),
        None if path == "~" => home_dir(),
        None => Ok(PathBuf::from(path)),
    }
}

/// Arquivo de configuração da tool.
pub fn config_path() -> Result<PathBuf> {
    Ok(platform::config_dir()?.join(CONFIG_FILE))
}

/// Arquivo de log das execuções no login.
pub fn log_path() -> Result<PathBuf> {
    Ok(platform::state_dir()?.join(LOG_FILE))
}

/// Histórico de execuções das rotinas.
pub fn history_path() -> Result<PathBuf> {
    Ok(platform::state_dir()?.join(HISTORY_FILE))
}

/// Data e hora local no formato do log e do histórico.
pub fn timestamp() -> String {
    jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S").to_string()
}

/// Escreve no stderr e anexa ao arquivo de log.
pub fn log(msg: &str) {
    let stamp = timestamp();
    eprintln!("{msg}");
    let Ok(path) = log_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(format!("[{stamp}] {msg}\n").as_bytes());
    }
}
