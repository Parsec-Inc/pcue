use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::constants::{BLOCK_BEGIN, BLOCK_END};
use crate::utils::{hypr_config_dir, lua_str, xdg_autostart_file};

/// Onde e como o bloco de autostart é escrito.
struct Target {
    file: PathBuf,
    style: Style,
}

/// Formato do arquivo de autostart.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    HyprLua,
    HyprConf,
    XdgDesktop,
}

impl Style {
    fn comment(self) -> &'static str {
        match self {
            Style::HyprLua => "--",
            Style::HyprConf | Style::XdgDesktop => "#",
        }
    }
}

/// Registra o daemon para iniciar junto com a sessão gráfica.
pub fn install() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("não foi possível achar o próprio executável")?;
    let target = target()?;
    let command = format!("{} daemon", exe.display());
    let body = match target.style {
        Style::HyprLua => format!(
            "hl.on(\"hyprland.start\", function()\n    hl.exec_cmd({})\nend)",
            lua_str(&command)
        ),
        Style::HyprConf => format!("exec-once = {command}"),
        Style::XdgDesktop => format!(
            "[Desktop Entry]\nType=Application\nName=pwboot\nComment=Rotinas pós-login\nExec={command}\nX-GNOME-Autostart-enabled=true"
        ),
    };
    if let Some(dir) = target.file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let current = read_or_empty(&target.file)?;
    let comment = target.style.comment();
    let updated = upsert_block(&strip_block(&current, comment), comment, &body);
    write_with_backup(&target.file, &current, &updated)?;
    Ok(target.file)
}

/// Remove o bloco do pwboot da config do Hyprland.
pub fn uninstall() -> Result<Option<PathBuf>> {
    let target = target()?;
    let current = read_or_empty(&target.file)?;
    let updated = strip_block(&current, target.style.comment());
    if updated == current {
        return Ok(None);
    }
    write_with_backup(&target.file, &current, &updated)?;
    Ok(Some(target.file))
}

/// Hyprland Lua, Hyprland `.conf` ou autostart XDG nos demais desktops.
fn target() -> Result<Target> {
    let dir = hypr_config_dir()?;
    if dir.join("hyprland.lua").exists() {
        let custom = dir.join("custom");
        let file = if custom.is_dir() {
            custom.join("execs.lua")
        } else {
            dir.join("hyprland.lua")
        };
        return Ok(Target {
            file,
            style: Style::HyprLua,
        });
    }
    if dir.join("hyprland.conf").exists() {
        return Ok(Target {
            file: dir.join("hyprland.conf"),
            style: Style::HyprConf,
        });
    }
    Ok(Target {
        file: xdg_autostart_file()?,
        style: Style::XdgDesktop,
    })
}

/// Está registrado no autostart?
pub fn is_installed() -> bool {
    target()
        .ok()
        .and_then(|t| std::fs::read_to_string(&t.file).ok().map(|raw| (t, raw)))
        .is_some_and(|(t, raw)| raw.contains(&format!("{} {BLOCK_BEGIN}", t.style.comment())))
}

fn read_or_empty(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(raw),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err).with_context(|| format!("falha ao ler {}", path.display())),
    }
}

/// Grava guardando a versão anterior em `.pwboot.bak`.
fn write_with_backup(path: &Path, previous: &str, updated: &str) -> Result<()> {
    if !previous.is_empty() {
        let mut backup = path.as_os_str().to_owned();
        backup.push(".pwboot.bak");
        std::fs::write(&backup, previous)?;
    }
    std::fs::write(path, updated).with_context(|| format!("falha ao gravar {}", path.display()))
}

/// Anexa o bloco delimitado ao fim do conteúdo.
fn upsert_block(content: &str, comment: &str, body: &str) -> String {
    let mut out = content.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(&format!("{comment} {BLOCK_BEGIN}\n{body}\n{comment} {BLOCK_END}\n"));
    out
}

/// Tira o bloco delimitado, se existir.
fn strip_block(content: &str, comment: &str) -> String {
    let begin = format!("{comment} {BLOCK_BEGIN}");
    let end = format!("{comment} {BLOCK_END}");
    let (Some(start), Some(stop)) = (content.find(&begin), content.find(&end)) else {
        return content.to_string();
    };
    let head = content[..start].trim_end();
    let tail = content[stop + end.len()..].trim_start_matches('\n');
    match (head.is_empty(), tail.is_empty()) {
        (true, _) => tail.to_string(),
        (false, true) => format!("{head}\n"),
        (false, false) => format!("{head}\n\n{tail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_then_strip_restores_original() {
        let original = "hl.config({})\n";
        let with = upsert_block(original, "--", "hl.exec_cmd(\"x\")");
        assert!(with.contains("-- >>> pwboot >>>"));
        assert_eq!(strip_block(&with, "--"), original);
    }

    #[test]
    fn reinstall_does_not_duplicate() {
        let once = upsert_block("", "#", "exec-once = a");
        let twice = upsert_block(&strip_block(&once, "#"), "#", "exec-once = b");
        assert_eq!(twice.matches(BLOCK_BEGIN).count(), 1);
        assert!(twice.contains("exec-once = b"));
    }
}
