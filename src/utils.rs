use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};

use crate::constants::{APP_NAME, CONFIG_FILE, DAEMON_SOCKET, HISTORY_FILE, LOG_FILE};

/// Diretório home do usuário.
pub fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("variável HOME não definida")
}

/// Caminho XDG com fallback relativo ao home.
pub fn xdg_dir(var: &str, fallback: &str) -> Result<PathBuf> {
    match std::env::var_os(var) {
        Some(dir) if !dir.is_empty() => Ok(PathBuf::from(dir)),
        _ => Ok(home_dir()?.join(fallback)),
    }
}

/// Move config e estado do nome antigo, se ainda não migrados.
pub fn migrate_legacy() {
    for (var, fallback) in [("XDG_CONFIG_HOME", ".config"), ("XDG_STATE_HOME", ".local/state")] {
        let Ok(base) = xdg_dir(var, fallback) else { continue };
        let (old, new) = (base.join(crate::constants::LEGACY_NAME), base.join(APP_NAME));
        if old.is_dir() && !new.exists() {
            let _ = std::fs::rename(&old, &new);
            let legacy_log = new.join(format!("{}.log", crate::constants::LEGACY_NAME));
            let _ = std::fs::rename(legacy_log, new.join(LOG_FILE));
        }
    }
}

/// Arquivo de configuração da tool.
pub fn config_path() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join(APP_NAME).join(CONFIG_FILE))
}

/// Diretório de configuração do Hyprland.
pub fn hypr_config_dir() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join("hypr"))
}

/// Entrada de autostart XDG para desktops sem Hyprland.
pub fn xdg_autostart_file() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?
        .join("autostart")
        .join(format!("{APP_NAME}.desktop")))
}

/// Arquivo de log das execuções no login.
pub fn log_path() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_STATE_HOME", ".local/state")?.join(APP_NAME).join(LOG_FILE))
}

/// Histórico de execuções das rotinas.
pub fn history_path() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_STATE_HOME", ".local/state")?.join(APP_NAME).join(HISTORY_FILE))
}

/// `XDG_RUNTIME_DIR` da sessão.
pub fn xdg_runtime_base() -> Result<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .context("XDG_RUNTIME_DIR não definida")
}

/// Diretório de arquivos temporários do pcue.
pub fn runtime_dir() -> Result<PathBuf> {
    Ok(xdg_runtime_base()?.join(APP_NAME))
}

/// Socket do daemon.
pub fn daemon_socket() -> Result<PathBuf> {
    Ok(runtime_dir()?.join(DAEMON_SOCKET))
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
        let _ = writeln!(file, "[{stamp}] {msg}");
    }
}

/// Mostra notificação na área de trabalho, ignorando falhas.
pub fn notify(summary: &str, body: &str) {
    let _ = Command::new("notify-send")
        .args(["--app-name", APP_NAME, summary, body])
        .status();
}

/// Literal de string Lua com escapes.
pub fn lua_str(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Junta argumentos numa linha de shell com aspas quando necessário.
pub fn shell_join<S: AsRef<str>>(args: &[S]) -> String {
    args.iter()
        .map(|arg| shell_quote(arg.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Aspas simples POSIX para um argumento.
fn shell_quote(arg: &str) -> String {
    let safe = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c));
    if safe {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lua_str_escapes_quotes_and_backslashes() {
        assert_eq!(lua_str(r#"sh -c "a\b""#), r#""sh -c \"a\\b\"""#);
    }

    #[test]
    fn shell_join_quotes_only_unsafe_args() {
        assert_eq!(
            shell_join(&["code", "--new-window", "my dir", "it's"]),
            r"code --new-window 'my dir' 'it'\''s'"
        );
    }
}
