use std::path::{Path, PathBuf};

use crate::constants::DESKTOP_FIELD_CODES;
use super::xdg_dir;
use crate::utils::home_dir;

/// Entrada `.desktop` com o que interessa para abrir o app.
struct Entry {
    stem: String,
    wm_class: Option<String>,
    exec: String,
}

/// Entradas `.desktop` instaladas, lidas uma vez.
pub struct DesktopIndex {
    entries: Vec<Entry>,
}

impl DesktopIndex {
    /// Lê todos os diretórios `applications`.
    pub fn load() -> Self {
        Self {
            entries: application_dirs().iter().flat_map(|d| read_entries(d)).collect(),
        }
    }

    /// Comando de abertura do app cuja janela tem essa class.
    pub fn find_exec(&self, class: &str) -> Option<String> {
        find_in(&self.entries, class)
    }
}

/// Busca por StartupWMClass, nome do arquivo e sufixo de ID reverso.
fn find_in(entries: &[Entry], class: &str) -> Option<String> {
    let by_wm_class = entries
        .iter()
        .find(|e| e.wm_class.as_deref().is_some_and(|w| w.eq_ignore_ascii_case(class)));
    let by_stem = || entries.iter().find(|e| e.stem.eq_ignore_ascii_case(class));
    let by_reverse_dns = || {
        entries.iter().find(|e| {
            e.stem
                .rsplit('.')
                .next()
                .is_some_and(|last| last.eq_ignore_ascii_case(class))
        })
    };
    by_wm_class
        .or_else(by_stem)
        .or_else(by_reverse_dns)
        .map(|e| strip_field_codes(&e.exec))
}

/// Diretórios `applications` em ordem de prioridade XDG.
fn application_dirs() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = xdg_dir("XDG_DATA_HOME", ".local/share").into_iter().collect();
    roots.extend(home_dir().ok().map(|h| h.join(".local/share/flatpak/exports/share")));
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    roots.extend(system.split(':').map(PathBuf::from));
    roots.push(PathBuf::from("/var/lib/flatpak/exports/share"));
    roots.into_iter().map(|r| r.join("applications")).collect()
}

/// Lê as entradas `.desktop` de um diretório.
fn read_entries(dir: &Path) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    read.flatten()
        .map(|f| f.path())
        .filter(|p| p.extension().is_some_and(|e| e == "desktop"))
        .filter_map(|p| parse_entry(&p))
        .collect()
}

/// Extrai Exec e StartupWMClass da seção principal.
fn parse_entry(path: &Path) -> Option<Entry> {
    let raw = std::fs::read_to_string(path).ok()?;
    let mut in_main = false;
    let mut exec = None;
    let mut wm_class = None;
    for line in raw.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        if let Some(value) = line.strip_prefix("Exec=") {
            exec = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("StartupWMClass=") {
            wm_class = Some(value.to_string());
        }
    }
    Some(Entry {
        stem: path.file_stem()?.to_string_lossy().into_owned(),
        wm_class,
        exec: exec?,
    })
}

/// Remove os códigos `%f`, `%U` etc. do Exec.
fn strip_field_codes(exec: &str) -> String {
    exec.split_whitespace()
        .filter(|token| !DESKTOP_FIELD_CODES.contains(token))
        .collect::<Vec<_>>()
        .join(" ")
        .replace("%%", "%")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_field_codes() {
        assert_eq!(strip_field_codes("/usr/bin/code --new-window %F"), "/usr/bin/code --new-window");
        assert_eq!(strip_field_codes("app --pct 50%%"), "app --pct 50%");
    }
}
