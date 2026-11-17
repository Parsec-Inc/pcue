use std::collections::BTreeMap;

use anyhow::{Result, bail};

use crate::platform::{Desktop, Window};
use crate::script::Step;

/// Passos `launch` para as janelas abertas agora.
pub fn capture(only: &[String]) -> Result<Vec<String>> {
    let desktop = Desktop::connect();
    if desktop.manager().is_none() {
        bail!("capturar janelas precisa de um gerenciador de janelas suportado");
    }
    let mut groups: BTreeMap<(String, String), Vec<Window>> = BTreeMap::new();
    for window in desktop.windows()? {
        if window.class.is_empty() || !wanted(&window.workspace, only) {
            continue;
        }
        groups
            .entry((window.workspace.clone(), window.class.clone()))
            .or_default()
            .push(window);
    }

    let mut steps = Vec::new();
    for ((workspace, class), windows) in groups {
        let command = desktop
            .launch_command(&windows[0])
            .unwrap_or_else(|| class.to_lowercase());
        for index in 0..windows.len() {
            let step = Step::Launch {
                command: command.clone(),
                workspace: Some(desktop.workspace_for(&workspace)),
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
