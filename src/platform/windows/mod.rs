//! Windows: Win32 para janelas, SendInput para entrada, hooks para gravar.
//! Aqui `workspace=N` é o monitor N, contado da esquerda para a direita.

mod events;
mod input;
mod install;
mod listen;
mod win;

use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use interprocess::local_socket::{GenericNamespaced, Name, ToNsName};
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, DETACHED_PROCESS};

pub use events::event_sources;
pub use input::VirtualInput;
pub use install::{install, is_installed, uninstall};
pub use listen::listen_input;

use crate::config::Workspace;
use crate::constants::{APP_NAME, BOOT_WAIT_TIMEOUT, MONITOR_SETTLE, POLL_INTERVAL};
use crate::platform::Window;
use crate::utils::log;

/// Janelas e monitores da sessão do Windows.
pub struct Desktop;

impl Desktop {
    /// Sempre disponível no Windows.
    pub fn connect() -> Self {
        Self
    }

    /// Sem aviso de mudanças no Windows.
    pub fn watch_changes(&self) -> Option<std::sync::mpsc::Receiver<Instant>> {
        None
    }

    /// Nome do gerenciador de janelas controlado.
    pub fn manager(&self) -> Option<&'static str> {
        Some("Windows")
    }

    /// Janelas de aplicativo abertas (as do Alt+Tab).
    pub fn windows(&self) -> Result<Vec<Window>> {
        let monitors = win::monitors();
        let mut processes = HashMap::new();
        Ok(win::top_level_windows()
            .into_iter()
            .enumerate()
            .filter(|(_, hwnd)| win::is_app_window(*hwnd))
            .map(|(rank, hwnd)| win::describe(hwnd, rank as i32, &monitors, &mut processes))
            .collect())
    }

    /// Janela em foco.
    pub fn active_window(&self) -> Result<Option<Window>> {
        let active = win::foreground();
        let window = win::is_app_window(active)
            .then(|| win::describe(active, 0, &win::monitors(), &mut HashMap::new()));
        Ok(window)
    }

    /// Monitor da janela em foco.
    pub fn active_workspace(&self) -> Result<String> {
        let monitors = win::monitors();
        Ok(win::monitor_number(win::foreground(), &monitors).to_string())
    }

    /// Abre o comando como o Executar do Windows (aceita caminho, nome registrado ou URL).
    pub fn spawn(&self, command: &str, _workspace: Option<&Workspace>, _focus: bool) -> Result<()> {
        spawn_detached(command)
    }

    /// Leva a janela ao monitor do workspace, mantendo maximizada se estava.
    pub fn place(&self, window: &Window, workspace: &Workspace) -> Result<()> {
        if window.workspace == workspace.0 {
            return Ok(());
        }
        let monitors = win::monitors();
        let target = monitor_index(workspace, monitors.len())?;
        win::move_to_monitor(win::parse_id(&window.id)?, &monitors[target]);
        log(&format!("janela abriu no monitor {}, movida para o {workspace}", window.workspace));
        Ok(())
    }

    /// Traz a janela para a frente, restaurando se minimizada.
    pub fn focus_window(&self, window: &Window) -> Result<()> {
        win::force_foreground(win::parse_id(&window.id)?)
    }

    /// Foca a janela mais recente do monitor e leva o cursor até ele.
    pub fn focus_workspace(&self, workspace: &Workspace) -> Result<()> {
        let monitors = win::monitors();
        let index = monitor_index(workspace, monitors.len())?;
        let (x, y) = monitors[index].center();
        win::set_cursor(x, y)?;
        let top = self
            .windows()?
            .into_iter()
            .filter(|w| w.workspace == workspace.0 && w.on_screen)
            .min_by_key(|w| w.focus_rank);
        top.map_or(Ok(()), |window| self.focus_window(&window))
    }

    /// Posição global do cursor.
    pub fn cursor_pos(&self) -> Result<(i32, i32)> {
        win::cursor()
    }

    /// Leva o cursor a uma posição global.
    pub fn move_cursor(&self, x: i32, y: i32) -> Result<()> {
        win::set_cursor(x, y)
    }

    /// Camadas de shell são do Hyprland.
    pub fn wait_layer(&self, _namespace: &str, _timeout: Duration) -> Result<()> {
        bail!("wait_layer só existe no Hyprland")
    }

    /// Espera a barra de tarefas e a quantidade de monitores parar de mudar.
    pub fn wait_monitors(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        let mut count = win::monitors().len();
        let mut stable_since = Instant::now();
        loop {
            let now = win::monitors().len();
            if now != count {
                count = now;
                stable_since = Instant::now();
            }
            if win::taskbar_ready() && stable_since.elapsed() >= MONITOR_SETTLE {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!("a área de trabalho não estabilizou após {}s", timeout.as_secs());
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// Esperas do boot: barra de tarefas e monitores.
    pub fn boot_waits(&self) -> Vec<String> {
        vec![format!("wait_monitors timeout={BOOT_WAIT_TIMEOUT}")]
    }

    /// Caminho do executável da janela, entre aspas se tiver espaço.
    pub fn launch_command(&self, window: &Window) -> Option<String> {
        let path = win::process_path(u32::try_from(window.pid).ok()?)?;
        Some(if path.contains(' ') { format!("\"{path}\"") } else { path })
    }

    /// No Windows o workspace já é o número do monitor.
    pub fn workspace_for(&self, reported: &str) -> Workspace {
        Workspace(reported.to_string())
    }
}

/// Índice do monitor pedido em `workspace=N`.
fn monitor_index(workspace: &Workspace, count: usize) -> Result<usize> {
    let number: usize = workspace
        .0
        .parse()
        .ok()
        .filter(|n| *n >= 1)
        .with_context(|| format!("no Windows o workspace é o número do monitor (1, 2...): {workspace}"))?;
    anyhow::ensure!(number <= count, "monitor {number} não existe (há {count})");
    Ok(number - 1)
}

/// Abre o comando sem janela de console.
pub fn spawn_detached(command: &str) -> Result<()> {
    Command::new("cmd")
        .raw_arg(format!("/C start \"\" {command}"))
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("falha ao abrir `{command}`"))?;
    Ok(())
}

fn known_dir(var: &str) -> Result<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .with_context(|| format!("{var} não definida"))
}

/// Configuração do pcue (`%APPDATA%\pcue`).
pub fn config_dir() -> Result<PathBuf> {
    Ok(known_dir("APPDATA")?.join(APP_NAME))
}

/// Log e histórico (`%LOCALAPPDATA%\pcue`).
pub fn state_dir() -> Result<PathBuf> {
    Ok(known_dir("LOCALAPPDATA")?.join(APP_NAME))
}

/// Progresso e próximos disparos.
pub fn runtime_dir() -> Result<PathBuf> {
    Ok(state_dir()?.join("run"))
}

/// Nome do gerenciador de janelas da sessão.
pub fn manager_name() -> Option<&'static str> {
    Some("Windows")
}

/// Named pipe do daemon, um por usuário.
pub fn daemon_endpoint() -> Result<Name<'static>> {
    let user = std::env::var("USERNAME").unwrap_or_default();
    Ok(format!("{APP_NAME}-{user}").to_ns_name::<GenericNamespaced>()?.into_owned())
}

/// Named pipes somem sozinhos; nada a limpar.
pub fn clear_endpoint() {}

/// O processo ainda existe?
pub fn process_alive(pid: u32) -> bool {
    win::process_alive(pid)
}

/// Encerra o processo.
pub fn kill(pid: u32) -> Result<()> {
    win::terminate(pid)
}

/// Sufixo do binário desta plataforma nas releases.
pub const ASSET_SUFFIX: &str = "windows-x86_64.exe";

/// Troca o executável: o Windows deixa renomear o que está em uso, mas não sobrescrever.
pub fn replace_exe(exe: &Path, bytes: &[u8]) -> Result<()> {
    let old = exe.with_extension("old.exe");
    let _ = std::fs::remove_file(&old);
    match std::fs::rename(exe, &old) {
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => return replace_as_admin(exe, &old, bytes),
        result => result.context("não foi possível afastar o executável atual")?,
    }
    if let Err(err) = std::fs::write(exe, bytes) {
        let _ = std::fs::rename(&old, exe);
        return Err(err).context("não foi possível gravar o executável novo");
    }
    Ok(())
}

/// Pasta protegida (ex.: Program Files): pede elevação pelo UAC para trocar.
fn replace_as_admin(exe: &Path, old: &Path, bytes: &[u8]) -> Result<()> {
    let dir = std::env::temp_dir();
    let new = dir.join("pcue-update.exe");
    let script = dir.join("pcue-update.cmd");
    std::fs::write(&new, bytes)?;
    let (exe, old, new_path) = (exe.display(), old.display(), new.display());
    std::fs::write(
        &script,
        format!(
            "@echo off\r\nmove /y \"{exe}\" \"{old}\" >nul || exit /b 1\r\n\
             move /y \"{new_path}\" \"{exe}\" >nul || (move /y \"{old}\" \"{exe}\" >nul & exit /b 1)\r\n"
        ),
    )?;
    let mut command = Command::new("powershell");
    command
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg("$p = Start-Process -FilePath $env:PCUE_UPDATE_SCRIPT -Verb RunAs -Wait -PassThru -WindowStyle Hidden; exit $p.ExitCode")
        .env("PCUE_UPDATE_SCRIPT", &script);
    background(&mut command);
    let status = command.status();
    let _ = std::fs::remove_file(&script);
    let _ = std::fs::remove_file(&new);
    match status {
        Ok(status) if status.success() => Ok(()),
        _ => bail!("o Windows não autorizou a troca do executável (UAC recusado?)"),
    }
}

/// Processo filho sem janela de console.
pub fn background(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

/// Inicia o binário novo e encerra este.
pub fn restart_self(exe: &Path, args: &[&str]) -> anyhow::Error {
    match Command::new(exe)
        .args(args)
        .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
        .spawn()
    {
        Ok(_) => std::process::exit(0),
        Err(err) => anyhow!("falha ao reiniciar: {err}"),
    }
}

/// Se o daemon abriu com janela de console, relança sem ela; `true` se relançou.
pub fn detach_daemon(args: &[&str]) -> Result<bool> {
    if !win::has_console_window() {
        return Ok(false);
    }
    Command::new(std::env::current_exe()?)
        .args(args)
        .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
        .spawn()
        .context("falha ao iniciar o daemon em segundo plano")?;
    Ok(true)
}

/// Abre o editor no arquivo, aceitando argumentos em `%EDITOR%`.
pub fn editor_command(editor: &str, file: &Path) -> Command {
    let mut command = Command::new("cmd");
    command.raw_arg(format!("/C {editor} \"{}\"", file.display()));
    command
}

/// Cria (ou trunca) o arquivo; a pasta do perfil já é só do usuário.
pub fn private_file(path: &Path) -> Result<std::fs::File> {
    Ok(std::fs::File::create(path)?)
}

/// Sem notificações no Windows por enquanto; a falha fica no log e no histórico.
pub fn notify(_summary: &str, _body: &str) {}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::Foundation::HWND;

    use super::*;

    /// Pasta temporária apagada no fim do teste.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("pcue-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn replace_exe_swaps_a_running_program() {
        let dir = Scratch::new("running");
        let exe = dir.0.join("pcue.exe");
        let ping = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join(r"System32\PING.EXE");
        std::fs::copy(ping, &exe).unwrap();
        let mut child = Command::new(&exe).args(["-n", "30", "127.0.0.1"]).stdout(Stdio::null()).spawn().unwrap();
        assert!(std::fs::write(&exe, b"x").is_err(), "o Windows não sobrescreve um exe em uso");
        let result = replace_exe(&exe, b"novo");
        let _ = child.kill();
        let _ = child.wait();
        result.unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"novo");
        assert!(dir.0.join("pcue.old.exe").exists());
    }

    #[test]
    fn this_process_is_alive_and_has_a_path() {
        let pid = std::process::id();
        assert!(process_alive(pid));
        let path = win::process_path(pid).unwrap();
        let exe = std::env::current_exe().unwrap();
        assert!(path.ends_with(&*exe.file_name().unwrap().to_string_lossy()), "{path}");
        assert!(!process_alive(u32::MAX - 1));
    }

    #[test]
    fn window_ids_round_trip() {
        let hwnd = 0x1a2b3c as HWND;
        assert_eq!(win::parse_id(&win::window_id(hwnd)).unwrap(), hwnd);
        assert!(win::parse_id("janela").is_err());
    }

    #[test]
    fn workspace_is_a_monitor_number() {
        assert_eq!(monitor_index(&Workspace("2".into()), 2).unwrap(), 1);
        assert!(monitor_index(&Workspace("3".into()), 2).is_err());
        assert!(monitor_index(&Workspace("0".into()), 2).is_err());
        assert!(monitor_index(&Workspace("web".into()), 2).is_err());
    }

    #[test]
    fn folders_and_pipe_come_from_the_environment() {
        assert!(config_dir().unwrap().ends_with(APP_NAME));
        assert!(runtime_dir().unwrap().starts_with(state_dir().unwrap()));
        daemon_endpoint().unwrap();
    }
}
