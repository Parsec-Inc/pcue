use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use regex::{Regex, RegexBuilder};

use crate::config::Workspace;
use crate::constants::POLL_INTERVAL;
use crate::platform::{Desktop, Window};
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
pub fn launch(desktop: &Desktop, spec: &LaunchSpec, timeout: Duration) -> Result<()> {
    let class = spec.class.map(anchored_regex).transpose()?;
    if desktop.manager().is_none() {
        return desktop.spawn(spec.command, spec.workspace, spec.focus);
    }

    let existing = desktop.windows()?;
    if !spec.always
        && let Some(re) = &class
        && existing.iter().any(|w| w.matches(re))
    {
        log(&format!("`{}` já está aberto, pulando", spec.command));
        return Ok(());
    }

    desktop.spawn(spec.command, spec.workspace, spec.focus)?;
    let Some(re) = class else { return Ok(()) };
    let seen: HashSet<String> = existing.into_iter().map(|w| w.id).collect();
    let window = wait_new_window(desktop, &re, &seen, timeout)?
        .with_context(|| format!("a janela de `{}` não apareceu em {}s", spec.command, timeout.as_secs()))?;
    if let Some(target) = spec.workspace {
        desktop.place(&window, target)?;
    }
    if spec.focus {
        desktop.focus_window(&window)?;
    }
    Ok(())
}

/// Janelas da class em ordem de abertura.
pub fn windows_of(windows: Vec<Window>, re: &Regex) -> Vec<Window> {
    let mut matching: Vec<Window> = windows.into_iter().filter(|w| w.matches(re)).collect();
    matching.sort_by_key(|w| w.opened);
    matching
}

/// A n-ésima janela da class (1 = a mais antiga), esperando ela existir.
pub fn nth_window(desktop: &Desktop, class: &str, nth: usize, timeout: Duration) -> Result<Window> {
    let re = anchored_regex(class)?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(window) = windows_of(desktop.windows()?, &re).into_iter().nth(nth - 1) {
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
    desktop: &Desktop,
    re: &Regex,
    seen: &HashSet<String>,
    timeout: Duration,
) -> Result<Option<Window>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(window) = desktop
            .windows()?
            .into_iter()
            .find(|w| !seen.contains(&w.id) && w.matches(re))
        {
            return Ok(Some(window));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Class como regex literal, sem escapar o `-` que não precisa.
pub fn literal_class(class: &str) -> String {
    regex::escape(class).replace("\\-", "-")
}

/// Class literal da janela e sua posição entre as da mesma class (1 = a mais antiga).
pub fn class_and_nth(window: &Window, all: &[Window]) -> (String, usize) {
    let class = literal_class(&window.class);
    let nth = anchored_regex(&class).map_or(1, |re| {
        1 + all.iter().filter(|w| w.matches(&re) && w.opened < window.opened).count()
    });
    (class, nth)
}

/// Regex da class, case-insensitive e casando a string inteira.
pub fn anchored_regex(pattern: &str) -> Result<Regex> {
    RegexBuilder::new(&format!("^(?:{pattern})$"))
        .case_insensitive(true)
        .build()
        .with_context(|| format!("class inválida: {pattern}"))
}
