use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::constants::PROGRESS_DIR;
use crate::utils::{read_json, runtime_dir, write_atomic};

/// Estado ao vivo de uma execução, lido pela TUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    pub routine: String,
    pub pid: u32,
    /// Número da linha do script em execução.
    pub line: usize,
    /// Posição do passo entre os executados (1 = primeiro).
    pub position: usize,
    pub total: usize,
}

fn dir() -> Result<PathBuf> {
    Ok(runtime_dir()?.join(PROGRESS_DIR))
}

/// Grava o progresso de forma atômica.
pub fn write(progress: &Progress) -> Result<()> {
    write_atomic(&dir()?.join(format!("{}.json", progress.routine)), &serde_json::to_vec(progress)?)
}

/// Remove o progresso da rotina.
pub fn clear(routine: &str) {
    if let Ok(dir) = dir() {
        let _ = std::fs::remove_file(dir.join(format!("{routine}.json")));
    }
}

/// Execuções em andamento, ignorando processos que já morreram.
pub fn running() -> HashMap<String, Progress> {
    let Ok(entries) = dir().and_then(|d| Ok(std::fs::read_dir(d)?)) else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter_map(|entry| read_json::<Progress>(&entry.path()))
        .filter(|p| PathBuf::from(format!("/proc/{}", p.pid)).exists())
        .map(|p| (p.routine.clone(), p))
        .collect()
}

/// Interrompe a execução da rotina.
pub fn stop(progress: &Progress) -> Result<()> {
    let status = std::process::Command::new("kill").arg(progress.pid.to_string()).status()?;
    anyhow::ensure!(status.success(), "não foi possível parar {}", progress.routine);
    clear(&progress.routine);
    Ok(())
}
