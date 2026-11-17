use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::constants::APP_NAME;

/// Chave de inicialização do usuário.
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// Roda `reg.exe` sem janela; `true` se deu certo.
fn reg(args: &[&str]) -> Result<bool> {
    Ok(Command::new("reg")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("falha ao rodar reg.exe")?
        .success())
}

/// Registra o daemon para iniciar no login do usuário.
pub fn install() -> Result<String> {
    let exe = std::env::current_exe().context("não foi possível achar o próprio executável")?;
    let command = format!("\"{}\" daemon", exe.display());
    let ok = reg(&["add", RUN_KEY, "/v", APP_NAME, "/t", "REG_SZ", "/d", &command, "/f"])?;
    anyhow::ensure!(ok, "o registro recusou a entrada de inicialização");
    Ok(format!(r"{RUN_KEY}\{APP_NAME}"))
}

/// Remove o daemon da inicialização.
pub fn uninstall() -> Result<Option<String>> {
    if !is_installed() {
        return Ok(None);
    }
    let ok = reg(&["delete", RUN_KEY, "/v", APP_NAME, "/f"])?;
    anyhow::ensure!(ok, "o registro recusou a remoção");
    Ok(Some(format!(r"{RUN_KEY}\{APP_NAME}")))
}

/// Está na inicialização?
pub fn is_installed() -> bool {
    reg(&["query", RUN_KEY, "/v", APP_NAME]).unwrap_or(false)
}
