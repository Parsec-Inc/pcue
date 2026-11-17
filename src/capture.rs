use std::collections::BTreeMap;

use anyhow::Result;

use crate::config::Workspace;
use crate::desktop::DesktopIndex;
use crate::hypr::{Client, Hypr};
use crate::script::Step;
use crate::utils::shell_join;

/// Passos `launch` para as janelas abertas agora.
pub fn capture(only: &[String]) -> Result<Vec<String>> {
    let hypr = Hypr::connect()?;
    let mut groups: BTreeMap<(String, String), Vec<Client>> = BTreeMap::new();
    for client in hypr.clients()? {
        if client.class.is_empty() || !wanted(&client.workspace.name, only) {
            continue;
        }
        groups
            .entry((client.workspace.name.clone(), client.class.clone()))
            .or_default()
            .push(client);
    }

    let desktop = DesktopIndex::load();
    let mut steps = Vec::new();
    for ((workspace, class), clients) in groups {
        let command = desktop
            .find_exec(&class)
            .or_else(|| proc_cmdline(clients[0].pid))
            .unwrap_or_else(|| class.to_lowercase());
        for index in 0..clients.len() {
            let step = Step::Launch {
                command: command.clone(),
                workspace: Some(to_workspace(&workspace)),
                class: Some(regex::escape(&class)),
                focus: false,
                always: index > 0,
            };
            steps.push(step.to_string());
        }
    }
    Ok(steps)
}

/// Filtro opcional por workspace.
fn wanted(workspace: &str, only: &[String]) -> bool {
    only.is_empty() || only.iter().any(|w| w == workspace)
}

/// Converte o nome reportado no formato aceito pelo dispatch.
fn to_workspace(reported: &str) -> Workspace {
    if reported.parse::<i64>().is_ok() || reported.starts_with("special:") {
        Workspace(reported.to_string())
    } else {
        Workspace(format!("name:{reported}"))
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
