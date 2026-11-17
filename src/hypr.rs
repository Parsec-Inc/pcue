use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::constants::{POLL_INTERVAL, SOCKET_READY_TIMEOUT};
use crate::utils::{lua_str, xdg_runtime_base};

/// Sintaxe de dispatch aceita pelo Hyprland em execução.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Lua,
    Legacy,
}

/// Cliente do socket IPC do Hyprland.
pub struct Hypr {
    socket: PathBuf,
    dialect: Dialect,
}

/// Janela como reportada por `j/clients`.
#[derive(Debug, Clone, Deserialize)]
pub struct Client {
    pub address: String,
    pub class: String,
    #[serde(rename = "initialClass")]
    pub initial_class: String,
    pub workspace: ClientWorkspace,
    pub pid: i64,
    #[serde(default)]
    pub at: [i32; 2],
    #[serde(default)]
    pub size: [i32; 2],
    #[serde(default)]
    pub hidden: bool,
    #[serde(default = "visible_default")]
    pub visible: bool,
    #[serde(rename = "focusHistoryID", default)]
    pub focus_history_id: i32,
    #[serde(rename = "stableId", default)]
    pub stable_id: String,
}

/// Posição do cursor em `j/cursorpos`.
#[derive(Debug, Deserialize)]
struct CursorPos {
    x: i32,
    y: i32,
}

fn visible_default() -> bool {
    true
}

impl Client {
    /// O ponto global está dentro da janela?
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.at[0] && y >= self.at[1] && x < self.at[0] + self.size[0] && y < self.at[1] + self.size[1]
    }

    /// Ordem de abertura (o `stableId` cresce a cada janela nova).
    pub fn opened_order(&self) -> u64 {
        u64::from_str_radix(&self.stable_id, 16).unwrap_or(u64::MAX)
    }

    /// Janela visível na tela agora.
    pub fn on_screen(&self) -> bool {
        self.visible && !self.hidden
    }
}

/// Monitor conectado.
#[derive(Debug, Clone, Deserialize)]
pub struct Monitor {
    pub name: String,
    pub description: String,
}

impl Monitor {
    /// Aceita o nome (`DP-1`) ou `desc:` como nas regras do Hyprland.
    pub fn matches(&self, spec: &str) -> bool {
        match spec.strip_prefix("desc:") {
            Some(desc) => self.description.starts_with(desc.trim()),
            None => self.name == spec,
        }
    }
}

/// Workspace existente e o monitor onde está.
#[derive(Debug, Clone, Deserialize)]
pub struct Workspace {
    pub name: String,
    pub monitor: String,
}

/// Regra de workspace com monitor fixo.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceRule {
    #[serde(rename = "workspaceString")]
    pub workspace: String,
    pub monitor: Option<String>,
}

/// Workspace de uma janela.
#[derive(Debug, Clone, Deserialize)]
pub struct ClientWorkspace {
    pub name: String,
}

impl Hypr {
    /// Conecta só se houver uma sessão do Hyprland; falhas viram aviso.
    pub fn detect() -> Option<Self> {
        if !in_session() {
            return None;
        }
        match Self::connect() {
            Ok(hypr) => Some(hypr),
            Err(err) => {
                crate::utils::log(&format!("Hyprland indisponível: {err:#}"));
                None
            }
        }
    }

    /// Conecta à instância atual, esperando o socket ficar pronto.
    pub fn connect() -> Result<Self> {
        let socket = socket_dir()?.join(".socket.sock");
        wait_ready(&socket)?;
        let dialect = detect_dialect(&socket)?;
        Ok(Self { socket, dialect })
    }

    /// Janelas abertas.
    pub fn clients(&self) -> Result<Vec<Client>> {
        let raw = self.request("j/clients")?;
        serde_json::from_str(&raw).context("resposta inesperada de j/clients")
    }

    /// Executa um comando, aplicando a regra de workspace se houver.
    pub fn exec(&self, command: &str, workspace: Option<&str>, silent: bool) -> Result<()> {
        let rule = workspace.map(|ws| if silent { format!("{ws} silent") } else { ws.to_string() });
        let dispatch = match (self.dialect, rule) {
            (Dialect::Lua, Some(rule)) => format!(
                "hl.dsp.exec_cmd({}, {{ workspace = {} }})",
                lua_str(command),
                lua_str(&rule)
            ),
            (Dialect::Lua, None) => format!("hl.dsp.exec_cmd({})", lua_str(command)),
            (Dialect::Legacy, Some(rule)) => format!("exec [workspace {rule}] {command}"),
            (Dialect::Legacy, None) => format!("exec {command}"),
        };
        self.dispatch(&dispatch)
    }

    /// A sessão está bloqueada (qualquer tela de bloqueio ext-session-lock)?
    /// Namespaces das superfícies de shell (barras, painéis, fundo).
    pub fn layers(&self) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Monitor {
            levels: std::collections::HashMap<String, Vec<Layer>>,
        }
        #[derive(Deserialize)]
        struct Layer {
            namespace: String,
        }
        let monitors: std::collections::HashMap<String, Monitor> =
            serde_json::from_str(&self.request("j/layers")?).context("resposta inesperada de j/layers")?;
        Ok(monitors
            .into_values()
            .flat_map(|m| m.levels.into_values().flatten())
            .map(|layer| layer.namespace)
            .collect())
    }

    pub fn is_locked(&self) -> Result<bool> {
        #[derive(Deserialize)]
        struct Locked {
            locked: bool,
        }
        let reply: Locked = serde_json::from_str(&self.request("j/locked")?)
            .context("resposta inesperada de j/locked")?;
        Ok(reply.locked)
    }

    /// Posição global do cursor.
    pub fn cursor_pos(&self) -> Result<(i32, i32)> {
        let pos: CursorPos = serde_json::from_str(&self.request("j/cursorpos")?)
            .context("resposta inesperada de j/cursorpos")?;
        Ok((pos.x, pos.y))
    }

    /// Leva o cursor a uma posição global.
    pub fn move_cursor(&self, x: i32, y: i32) -> Result<()> {
        let dispatch = match self.dialect {
            Dialect::Lua => format!("hl.dsp.cursor.move({{ x = {x}, y = {y} }})"),
            Dialect::Legacy => format!("movecursor {x} {y}"),
        };
        self.dispatch(&dispatch)
    }

    /// Foca a janela, trocando para o workspace dela se preciso.
    pub fn focus_window(&self, address: &str) -> Result<()> {
        let dispatch = match self.dialect {
            Dialect::Lua => format!(
                "hl.dsp.focus({{ window = {} }})",
                lua_str(&format!("address:{address}"))
            ),
            Dialect::Legacy => format!("focuswindow address:{address}"),
        };
        self.dispatch(&dispatch)
    }

    /// Janela visível sob o ponto, a mais recente em foco primeiro.
    pub fn window_at(&self, x: i32, y: i32) -> Result<Option<Client>> {
        Ok(self
            .clients()?
            .into_iter()
            .filter(|c| c.on_screen() && c.contains(x, y))
            .min_by_key(|c| c.focus_history_id))
    }

    /// Move uma janela para o workspace sem trocar o foco.
    /// Monitores conectados.
    pub fn monitors(&self) -> Result<Vec<Monitor>> {
        serde_json::from_str(&self.request("j/monitors")?).context("resposta inesperada de j/monitors")
    }

    /// Workspaces existentes.
    pub fn workspaces(&self) -> Result<Vec<Workspace>> {
        serde_json::from_str(&self.request("j/workspaces")?).context("resposta inesperada de j/workspaces")
    }

    /// Regras de workspace que fixam um monitor.
    pub fn workspace_rules(&self) -> Result<Vec<WorkspaceRule>> {
        let rules: Vec<WorkspaceRule> = serde_json::from_str(&self.request("j/workspacerules")?)
            .context("resposta inesperada de j/workspacerules")?;
        Ok(rules.into_iter().filter(|r| r.monitor.is_some()).collect())
    }

    /// Vai para o workspace exato, sem depender dos atalhos.
    pub fn focus_workspace(&self, workspace: &str) -> Result<()> {
        let dispatch = match self.dialect {
            Dialect::Lua => format!("hl.dsp.focus({{ workspace = {} }})", lua_str(workspace)),
            Dialect::Legacy => format!("workspace {workspace}"),
        };
        self.dispatch(&dispatch)
    }

    /// Leva o workspace para o monitor.
    pub fn move_workspace(&self, workspace: &str, monitor: &str) -> Result<()> {
        let dispatch = match self.dialect {
            Dialect::Lua => format!(
                "hl.dsp.workspace.move({{ workspace = {}, monitor = {} }})",
                lua_str(workspace),
                lua_str(monitor)
            ),
            Dialect::Legacy => format!("moveworkspacetomonitor {workspace} {monitor}"),
        };
        self.dispatch(&dispatch)
    }

    pub fn move_window(&self, address: &str, workspace: &str) -> Result<()> {
        let dispatch = match self.dialect {
            Dialect::Lua => format!(
                "hl.dsp.window.move({{ workspace = {}, follow = false, window = {} }})",
                lua_str(workspace),
                lua_str(&format!("address:{address}"))
            ),
            Dialect::Legacy => format!("movetoworkspacesilent {workspace},address:{address}"),
        };
        self.dispatch(&dispatch)
    }

    /// Envia um dispatch e falha se a resposta não for `ok`.
    fn dispatch(&self, body: &str) -> Result<()> {
        let reply = self.request(&format!("dispatch {body}"))?;
        if reply.trim() != "ok" {
            bail!("Hyprland recusou o dispatch: {}", reply.trim());
        }
        Ok(())
    }

    /// Uma requisição/resposta no socket de comandos.
    fn request(&self, message: &str) -> Result<String> {
        request(&self.socket, message)
    }
}

/// Diretório dos sockets da instância atual.
fn socket_dir() -> Result<PathBuf> {
    let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .context("HYPRLAND_INSTANCE_SIGNATURE não definida (rodando fora do Hyprland?)")?;
    Ok(xdg_runtime_base()?.join("hypr").join(signature))
}

/// Socket de eventos (`socket2`) da instância atual.
pub fn event_socket() -> Result<PathBuf> {
    Ok(socket_dir()?.join(".socket2.sock"))
}

/// Há uma sessão do Hyprland neste ambiente?
pub fn in_session() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

/// Repete a conexão até o compositor responder.
fn wait_ready(socket: &Path) -> Result<()> {
    let deadline = Instant::now() + SOCKET_READY_TIMEOUT;
    loop {
        match request(socket, "j/version") {
            Ok(_) => return Ok(()),
            Err(err) if Instant::now() >= deadline => {
                return Err(err.context("socket do Hyprland não respondeu"));
            }
            Err(_) => std::thread::sleep(POLL_INTERVAL),
        }
    }
}

/// Descobre se o dispatch usa Lua ou a sintaxe antiga.
fn detect_dialect(socket: &Path) -> Result<Dialect> {
    let reply = request(socket, "dispatch hl.dsp.no_op()")?;
    Ok(if reply.trim() == "ok" {
        Dialect::Lua
    } else {
        Dialect::Legacy
    })
}

/// Uma requisição/resposta no socket de comandos.
fn request(socket: &Path, message: &str) -> Result<String> {
    let mut stream = UnixStream::connect(socket)
        .with_context(|| format!("falha ao conectar em {}", socket.display()))?;
    stream.write_all(message.as_bytes())?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::Monitor;

    #[test]
    fn monitor_matches_name_or_description() {
        let monitor = Monitor {
            name: "DP-7".into(),
            description: "Dell Inc. DELL P2425H C8T8BJ4 (DP-7)".into(),
        };
        assert!(monitor.matches("DP-7"));
        assert!(monitor.matches("desc:Dell Inc. DELL P2425H C8T8BJ4"));
        assert!(!monitor.matches("desc:Dell Inc. DELL P2425H 11T8BJ4"));
        assert!(!monitor.matches("DP-5"));
    }
}
