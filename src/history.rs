use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::utils::history_path;

/// Uma execução de rotina.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub routine: String,
    pub trigger: String,
    pub started: String,
    pub duration_ms: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Linha do script que falhou.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_line: Option<usize>,
}

/// Acrescenta a execução ao histórico.
pub fn append(run: &Run) -> Result<()> {
    let path = history_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(run)?)?;
    Ok(())
}

/// Data de modificação do histórico, para evitar releituras.
pub fn modified() -> Option<std::time::SystemTime> {
    std::fs::metadata(history_path().ok()?).and_then(|m| m.modified()).ok()
}

/// Todas as execuções, da mais antiga para a mais recente.
pub fn all() -> Vec<Run> {
    let Ok(path) = history_path() else { return Vec::new() };
    let Ok(file) = std::fs::File::open(path) else { return Vec::new() };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<Run>(&line).ok())
        .collect()
}
