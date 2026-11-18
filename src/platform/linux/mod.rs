//! Linux: Hyprland para janelas, uinput/evdev para entrada, logind para eventos.

mod apps;
mod events;
mod hypr;
mod input;
mod install;
mod listen;

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{OnceLock, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use interprocess::local_socket::{GenericFilePath, Name, ToFsName};

pub use events::{event_sources, watch_devices, watch_notifications};
pub use input::VirtualInput;
pub use install::{install, is_installed, uninstall};
pub use listen::listen_input;

use apps::DesktopIndex;
use hypr::Hypr;

use crate::config::Workspace;
use crate::constants::{APP_NAME, BOOT_WAIT_TIMEOUT, DAEMON_SOCKET, DESKTOP_CHANGE_EVENTS, KNOWN_BARS, POLL_INTERVAL};
use crate::launcher::anchored_regex;
use crate::platform::Window;
use crate::script::quote;
use crate::utils::{home_dir, log};

/// Gerenciador de janelas da sessão (Hyprland), quando disponível.
pub struct Desktop {
    hypr: Option<Hypr>,
    apps: OnceLock<DesktopIndex>,
}

impl Desktop {
    /// Conecta ao Hyprland se houver; sem ele, só abre apps e envia teclas.
    pub fn connect() -> Self {
        Self {
            hypr: Hypr::detect(),
            apps: OnceLock::new(),
        }
    }

    /// Avisa o instante de cada mudança de janela ou workspace.
    pub fn watch_changes(&self) -> Option<mpsc::Receiver<Instant>> {
        self.hypr.as_ref()?;
        let stream = UnixStream::connect(hypr::event_socket().ok()?).ok()?;
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let name = line.split_once(">>").map_or("", |(name, _)| name);
                if DESKTOP_CHANGE_EVENTS.contains(&name) && sender.send(Instant::now()).is_err() {
                    return;
                }
            }
        });
        Some(receiver)
    }

    /// Nome do gerenciador de janelas controlado, se houver.
    pub fn manager(&self) -> Option<&'static str> {
        self.hypr.as_ref().map(|_| "Hyprland")
    }

    fn hypr(&self, what: &str) -> Result<&Hypr> {
        self.hypr
            .as_ref()
            .ok_or_else(|| anyhow!("{what} precisa do Hyprland"))
    }

    /// Janelas abertas.
    pub fn windows(&self) -> Result<Vec<Window>> {
        Ok(self
            .hypr("listar janelas")?
            .clients()?
            .into_iter()
            .map(hypr::Client::into_window)
            .collect())
    }

    /// Janela em foco.
    pub fn active_window(&self) -> Result<Option<Window>> {
        Ok(self.hypr("janela ativa")?.active_window()?.map(hypr::Client::into_window))
    }

    /// Workspace em foco.
    pub fn active_workspace(&self) -> Result<String> {
        Ok(self.hypr("workspace ativo")?.active_workspace()?.name)
    }

    /// Abre o comando, já no workspace pedido quando der.
    pub fn spawn(&self, command: &str, workspace: Option<&Workspace>, focus: bool) -> Result<()> {
        match &self.hypr {
            Some(hypr) => hypr.exec(command, workspace.map(|w| w.0.as_str()), !focus),
            None => {
                if workspace.is_some() {
                    log("sem Hyprland: workspace ignorado, abrindo no atual");
                }
                spawn_detached(command)
            }
        }
    }

    /// Leva a janela ao workspace, se ela abriu em outro.
    pub fn place(&self, window: &Window, workspace: &Workspace) -> Result<()> {
        if window.workspace == workspace.reported_name() {
            return Ok(());
        }
        self.hypr("mover janela")?.move_window(&window.id, &workspace.0)?;
        log(&format!("janela abriu no {}, movida para {workspace}", window.workspace));
        Ok(())
    }

    /// Foca a janela, trocando de workspace se preciso.
    pub fn focus_window(&self, window: &Window) -> Result<()> {
        self.hypr("focar janela")?.focus_window(&window.id)
    }

    /// Vai para o workspace exato.
    pub fn focus_workspace(&self, workspace: &Workspace) -> Result<()> {
        self.hypr("workspace")?.focus_workspace(&workspace.0)
    }

    /// Posição global do cursor.
    pub fn cursor_pos(&self) -> Result<(i32, i32)> {
        self.hypr("posição do cursor")?.cursor_pos()
    }

    /// Leva o cursor a uma posição global.
    pub fn move_cursor(&self, x: i32, y: i32) -> Result<()> {
        self.hypr("click")?.move_cursor(x, y)
    }

    /// Espera existir uma superfície de shell com o namespace.
    pub fn wait_layer(&self, namespace: &str, timeout: Duration) -> Result<()> {
        let hypr = self.hypr("wait_layer")?;
        let re = anchored_regex(namespace)?;
        let deadline = Instant::now() + timeout;
        while !hypr.layers()?.iter().any(|layer| re.is_match(layer)) {
            if Instant::now() >= deadline {
                bail!("nenhuma camada {namespace} após {}s", timeout.as_secs());
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        Ok(())
    }

    /// Espera os monitores das regras de workspace e devolve cada workspace ao seu.
    pub fn wait_monitors(&self, timeout: Duration) -> Result<()> {
        let hypr = self.hypr("wait_monitors")?;
        let rules = hypr.workspace_rules()?;
        let deadline = Instant::now() + timeout;
        let monitors = loop {
            let monitors = hypr.monitors()?;
            let missing: Vec<&str> = rules
                .iter()
                .filter_map(|r| r.monitor.as_deref())
                .filter(|spec| !monitors.iter().any(|m| m.matches(spec)))
                .collect();
            if missing.is_empty() {
                break monitors;
            }
            if Instant::now() >= deadline {
                bail!("monitores não conectaram após {}s: {}", timeout.as_secs(), missing.join(", "));
            }
            std::thread::sleep(POLL_INTERVAL);
        };
        for workspace in hypr.workspaces()? {
            let target = rules
                .iter()
                .find(|r| r.workspace == workspace.name)
                .and_then(|r| r.monitor.as_deref())
                .and_then(|spec| monitors.iter().find(|m| m.matches(spec)));
            if let Some(monitor) = target.filter(|m| m.name != workspace.monitor) {
                hypr.move_workspace(&workspace.name, &monitor.name)?;
                log(&format!("workspace {} movido de {} para {}", workspace.name, workspace.monitor, monitor.name));
            }
        }
        Ok(())
    }

    /// Esperas do boot conforme o ambiente: barra conhecida e monitores.
    pub fn boot_waits(&self) -> Vec<String> {
        let Some(hypr) = &self.hypr else { return Vec::new() };
        let layers = hypr.layers().unwrap_or_default();
        let bar = KNOWN_BARS.iter().find(|bar| layers.iter().any(|l| l == *bar));
        let mut steps: Vec<String> = bar
            .map(|bar| format!("wait_layer namespace={} timeout={BOOT_WAIT_TIMEOUT}", quote(bar)))
            .into_iter()
            .collect();
        steps.push(format!("wait_monitors timeout={BOOT_WAIT_TIMEOUT}"));
        steps
    }

    /// Comando que abre o app da janela: entrada `.desktop` ou linha do processo.
    pub fn launch_command(&self, window: &Window) -> Option<String> {
        self.apps
            .get_or_init(DesktopIndex::load)
            .find_exec(&window.class)
            .or_else(|| proc_cmdline(window.pid))
    }

    /// Workspace no formato aceito pelos passos, a partir do nome reportado.
    pub fn workspace_for(&self, reported: &str) -> Workspace {
        if reported.parse::<i64>().is_ok() || reported.starts_with("special:") {
            Workspace(reported.to_string())
        } else {
            Workspace(format!("name:{reported}"))
        }
    }
}

/// Linha de comando do processo dono da janela.
fn proc_cmdline(pid: i64) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args: Vec<String> = raw
        .split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    (!args.is_empty()).then(|| shell_join(&args))
}

/// Comando do passo `shell`; em background, num grupo próprio para sobreviver à rotina.
pub fn shell(command: &str, background: bool) -> Command {
    use std::os::unix::process::CommandExt;
    let mut shell = Command::new("sh");
    shell.args(["-c", command]);
    if background {
        shell.process_group(0);
    }
    shell
}

/// Abre o comando fora da sessão do terminal, sem depender do compositor.
pub fn spawn_detached(command: &str) -> Result<()> {
    Command::new("setsid")
        .args(["-f", "sh", "-c", command])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("falha ao abrir `{command}`"))?
        .success()
        .then_some(())
        .ok_or_else(|| anyhow!("setsid falhou ao abrir `{command}`"))
}

/// Caminho XDG com fallback relativo ao home.
fn xdg_dir(var: &str, fallback: &str) -> Result<PathBuf> {
    match std::env::var_os(var) {
        Some(dir) if !dir.is_empty() => Ok(PathBuf::from(dir)),
        _ => Ok(home_dir()?.join(fallback)),
    }
}

/// Base XDG de configuração (`~/.config`).
fn config_base() -> Result<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

/// Configuração do pcue.
pub fn config_dir() -> Result<PathBuf> {
    Ok(config_base()?.join(APP_NAME))
}

/// Log e histórico.
pub fn state_dir() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_STATE_HOME", ".local/state")?.join(APP_NAME))
}

/// Arquivos que só valem enquanto a sessão existir.
pub fn runtime_dir() -> Result<PathBuf> {
    Ok(runtime_base()?.join(APP_NAME))
}

/// `XDG_RUNTIME_DIR` da sessão.
fn runtime_base() -> Result<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .context("XDG_RUNTIME_DIR não definida")
}

/// Diretório de configuração do Hyprland.
fn hypr_config_dir() -> Result<PathBuf> {
    Ok(config_base()?.join("hypr"))
}

/// Entrada de autostart XDG para desktops sem Hyprland.
fn xdg_autostart_file() -> Result<PathBuf> {
    Ok(config_base()?
        .join("autostart")
        .join(format!("{APP_NAME}.desktop")))
}

/// Move config e estado do nome antigo, se ainda não migrados.
pub fn migrate_legacy() {
    use crate::constants::{LEGACY_NAME, LOG_FILE};
    for (var, fallback) in [("XDG_CONFIG_HOME", ".config"), ("XDG_STATE_HOME", ".local/state")] {
        let Ok(base) = xdg_dir(var, fallback) else { continue };
        let (old, new) = (base.join(LEGACY_NAME), base.join(APP_NAME));
        if old.is_dir() && !new.exists() {
            let _ = std::fs::rename(&old, &new);
            let _ = std::fs::rename(new.join(format!("{LEGACY_NAME}.log")), new.join(LOG_FILE));
        }
    }
}

/// Nome do gerenciador de janelas da sessão, sem conectar a ele.
pub fn manager_name() -> Option<&'static str> {
    hypr::in_session().then_some("Hyprland")
}

/// Endereço do socket do daemon.
pub fn daemon_endpoint() -> Result<Name<'static>> {
    let path = runtime_dir()?.join(DAEMON_SOCKET);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    Ok(path.to_fs_name::<GenericFilePath>()?.into_owned())
}

/// Prepara o endereço antes de escutar (remove socket órfão).
pub fn clear_endpoint() {
    if let Ok(dir) = runtime_dir() {
        let _ = std::fs::remove_file(dir.join(DAEMON_SOCKET));
    }
}

/// O processo ainda existe?
pub fn process_alive(pid: u32) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
}

/// Encerra o processo.
pub fn kill(pid: u32) -> Result<()> {
    let status = Command::new("kill").arg(pid.to_string()).status()?;
    anyhow::ensure!(status.success(), "kill {pid} falhou");
    Ok(())
}

/// Sufixo do binário desta plataforma nas releases.
pub const ASSET_SUFFIX: &str = "linux-x86_64";

/// Troca o executável pelo novo; o rename funciona com ele em execução.
pub fn replace_exe(exe: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let tmp = exe.with_extension("new");
    match write_executable(&tmp, bytes) {
        Ok(()) => std::fs::rename(&tmp, exe).with_context(|| format!("falha ao substituir {}", exe.display())),
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => replace_as_root(exe, bytes),
        Err(err) => Err(err.into()),
    }
}

/// Instalado por pacote: grava fora e pede a senha pelo pkexec para trocar.
fn replace_as_root(exe: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let tmp = std::env::temp_dir().join(format!("pcue-update-{}", std::process::id()));
    write_executable(&tmp, bytes)?;
    let status = Command::new("pkexec")
        .args(["/bin/sh", "-c", r#"install -m755 "$0" "$1.new" && mv -f "$1.new" "$1""#])
        .arg(&tmp)
        .arg(exe)
        .status();
    let _ = std::fs::remove_file(&tmp);
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => bail!("senha recusada ou cancelada"),
        Err(_) => bail!("pkexec não encontrado; atualize pelo gerenciador de pacotes"),
    }
}

fn write_executable(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, bytes)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

/// Processo filho sem terminal (no Linux não há janela a esconder).
pub fn background(_command: &mut Command) {}

/// Substitui o processo pelo binário novo; só retorna em caso de erro.
pub fn restart_self(exe: &std::path::Path, args: &[&str]) -> anyhow::Error {
    use std::os::unix::process::CommandExt;
    let err = Command::new(exe).args(args).exec();
    anyhow!("falha ao reiniciar: {err}")
}

/// Abre o editor no arquivo, aceitando argumentos em `$EDITOR`.
pub fn editor_command(editor: &str, file: &std::path::Path) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(format!("{editor} \"$1\"")).arg("sh").arg(file);
    command
}

/// Cria (ou trunca) um arquivo legível só pelo dono.
pub fn private_file(path: &std::path::Path) -> Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

/// No Linux o autostart já roda sem terminal; nada a relançar.
pub fn detach_daemon(_args: &[&str]) -> Result<bool> {
    Ok(false)
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
        .map(|arg| crate::utils::shell_quote(arg.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
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
