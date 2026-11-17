use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use regex::{Regex, RegexBuilder};

use crate::config::Workspace;
use crate::constants::POLL_INTERVAL;
use crate::hypr::{Client, Hypr};
use crate::utils::log;

/// O que abrir e onde colocar.
pub struct LaunchSpec<'a> {
    pub command: &'a str,
    pub workspace: Option<&'a Workspace>,
    pub class: Option<&'a str>,
    pub focus: bool,
    pub always: bool,
}

/// Abre o app e, havendo class, espera a janela e corrige o workspace.
pub fn launch(hypr: Option<&Hypr>, spec: &LaunchSpec, timeout: Duration) -> Result<()> {
    let class = spec.class.map(class_regex).transpose()?;
    let Some(hypr) = hypr else {
        if spec.workspace.is_some() {
            log("sem Hyprland: workspace ignorado, abrindo no atual");
        }
        return spawn_detached(spec.command);
    };

    let existing = hypr.clients()?;
    if !spec.always
        && let Some(re) = &class
        && existing.iter().any(|c| matches_class(re, c))
    {
        log(&format!("`{}` já está aberto, pulando", spec.command));
        return Ok(());
    }

    hypr.exec(spec.command, spec.workspace.map(|w| w.0.as_str()), !spec.focus)?;
    let Some(re) = class else { return Ok(()) };
    let seen: HashSet<String> = existing.into_iter().map(|c| c.address).collect();
    let window = wait_new_window(hypr, &re, &seen, timeout)?
        .with_context(|| format!("a janela de `{}` não apareceu em {}s", spec.command, timeout.as_secs()))?;

    if let Some(target) = spec.workspace
        && window.workspace.name != target.reported_name()
    {
        hypr.move_window(&window.address, &target.0)?;
        log(&format!("janela abriu no {}, movida para {target}", window.workspace.name));
    }
    Ok(())
}

/// Janelas da class em ordem de abertura.
pub fn windows_of(clients: Vec<Client>, re: &Regex) -> Vec<Client> {
    let mut windows: Vec<Client> = clients.into_iter().filter(|c| matches_class(re, c)).collect();
    windows.sort_by_key(Client::opened_order);
    windows
}

/// A n-ésima janela da class (1 = a mais antiga), esperando ela existir.
pub fn nth_window(hypr: &Hypr, class: &str, nth: usize, timeout: Duration) -> Result<Client> {
    let re = class_regex(class)?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(window) = windows_of(hypr.clients()?, &re).into_iter().nth(nth - 1) {
            return Ok(window);
        }
        if Instant::now() >= deadline {
            bail!("não há {nth}ª janela {class} após {}s", timeout.as_secs());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Faz polling até surgir uma janela fora de `seen` com a class.
fn wait_new_window(
    hypr: &Hypr,
    re: &Regex,
    seen: &HashSet<String>,
    timeout: Duration,
) -> Result<Option<Client>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(client) = hypr
            .clients()?
            .into_iter()
            .find(|c| !seen.contains(&c.address) && matches_class(re, c))
        {
            return Ok(Some(client));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Abre o comando fora da sessão do terminal, sem depender do compositor.
fn spawn_detached(command: &str) -> Result<()> {
    Command::new("setsid")
        .args(["-f", "sh", "-c", command])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("falha ao abrir `{command}`"))?
        .success()
        .then_some(())
        .ok_or_else(|| anyhow::anyhow!("setsid falhou ao abrir `{command}`"))
}

/// Regex da class, case-insensitive e casando a string inteira.
pub fn class_regex(pattern: &str) -> Result<Regex> {
    RegexBuilder::new(&format!("^(?:{pattern})$"))
        .case_insensitive(true)
        .build()
        .with_context(|| format!("class inválida: {pattern}"))
}

/// Confere a class atual ou a inicial da janela.
fn matches_class(re: &Regex, client: &Client) -> bool {
    re.is_match(&client.class) || re.is_match(&client.initial_class)
}
