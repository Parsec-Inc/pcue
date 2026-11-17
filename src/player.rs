use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::config::Config;
use crate::constants::CLICK_MOVE_SETTLE;
use crate::hypr::Hypr;
use crate::input::VirtualInput;
use crate::launcher::{LaunchSpec, anchored_regex, launch, nth_window, wait_layer, windows_of};
use crate::progress::{self, Progress};
use crate::script::{ClickTarget, Guard, Line, MAX_CALL_DEPTH, Step, parse_steps};
use crate::utils::log;

/// Passo que interrompeu a rotina.
pub struct Failure {
    pub line: usize,
    pub error: anyhow::Error,
}

/// Executa passos, inclusive rotinas chamadas por `call`.
pub struct Player<'a> {
    config: &'a Config,
    hypr: Option<&'a Hypr>,
    timeout: Duration,
    input: Option<VirtualInput>,
    stack: Vec<String>,
}

impl<'a> Player<'a> {
    pub fn new(config: &'a Config, hypr: Option<&'a Hypr>) -> Self {
        Self {
            config,
            hypr,
            timeout: Duration::from_secs(config.settings.timeout_secs),
            input: None,
            stack: Vec::new(),
        }
    }

    /// Executa os passos em ordem; só a rotina de topo publica progresso.
    pub fn play(&mut self, routine: &str, lines: &[Line]) -> Result<(), Failure> {
        let top = self.stack.is_empty();
        self.stack.push(routine.to_string());
        let result = lines.iter().enumerate().try_for_each(|(index, line)| {
            if top {
                let _ = progress::write(&Progress {
                    routine: routine.to_string(),
                    pid: std::process::id(),
                    line: line.number,
                    position: index + 1,
                    total: lines.len(),
                });
            }
            self.run_line(routine, line)
                .map_err(|error| Failure { line: line.number, error })
        });
        self.stack.pop();
        result
    }

    fn run_line(&mut self, routine: &str, line: &Line) -> Result<()> {
        let prefix = format!("[{routine}] {}", line.number);
        if !self.guard_allows(&line.guard)? {
            log(&format!("{prefix}: pulado ({})", line.guard.to_string().trim()));
            return Ok(());
        }
        log(&format!("{prefix}: {}", describe(&line.step)));
        let result = (0..line.guard.repeat).try_for_each(|_| self.run_step(&line.step));
        match result {
            Err(err) if line.guard.optional => {
                log(&format!("{prefix}: falhou, mas é opcional: {err:#}"));
                Ok(())
            }
            other => other,
        }
    }

    fn guard_allows(&self, guard: &Guard) -> Result<bool> {
        if let Some(class) = &guard.if_window
            && !self.window_exists(class)?
        {
            return Ok(false);
        }
        if let Some(class) = &guard.unless_window
            && self.window_exists(class)?
        {
            return Ok(false);
        }
        Ok(true)
    }

    fn window_exists(&self, class: &str) -> Result<bool> {
        let hypr = self.hypr.context("if_window/unless_window precisam do Hyprland")?;
        Ok(!windows_of(hypr.clients()?, &anchored_regex(class)?).is_empty())
    }

    fn run_step(&mut self, step: &Step) -> Result<()> {
        let (hypr, timeout) = (self.hypr, self.timeout);
        match step {
            Step::Launch {
                command,
                workspace,
                class,
                focus,
                always,
            } => {
                let spec = LaunchSpec {
                    command,
                    workspace: workspace.as_ref(),
                    class: class.as_deref(),
                    focus: *focus,
                    always: *always,
                };
                launch(hypr, &spec, timeout)
            }
            Step::WaitWindow { class, nth, timeout } => match hypr {
                Some(hypr) => nth_window(hypr, class, *nth, *timeout).map(|_| ()),
                None => {
                    log("sem Hyprland: wait_window vira espera fixa do timeout");
                    std::thread::sleep(*timeout);
                    Ok(())
                }
            },
            Step::WaitLayer { namespace, timeout } => {
                wait_layer(hypr.context("wait_layer precisa do Hyprland")?, namespace, *timeout)
            }
            Step::Sleep(duration) => {
                std::thread::sleep(*duration);
                Ok(())
            }
            Step::Key(keys) => self.device()?.combo(keys),
            Step::Type(text) => self.device()?.type_text(text),
            Step::Scroll(amount) => self.device()?.scroll(*amount),
            Step::Click { button, target } => {
                let hypr = hypr.context("click precisa do Hyprland para posicionar o cursor")?;
                let (x, y) = resolve_click(hypr, target, timeout)?;
                hypr.move_cursor(x, y)?;
                std::thread::sleep(CLICK_MOVE_SETTLE);
                self.device()?.click(*button)
            }
            Step::Call(name) => self.call(name),
        }
    }

    fn call(&mut self, name: &str) -> Result<()> {
        if self.stack.len() >= MAX_CALL_DEPTH {
            bail!("call aninhado demais (máximo {MAX_CALL_DEPTH})");
        }
        if self.stack.iter().any(|routine| routine == name) {
            bail!("chamada circular: {} → {name}", self.stack.join(" → "));
        }
        let config = self.config;
        let routine = config
            .find(name)
            .with_context(|| format!("rotina '{name}' não existe"))?;
        let lines = parse_steps(&routine.steps)?;
        self.play(name, &lines)
            .map_err(|f| f.error.context(format!("{name}, linha {}", f.line)))
    }

    /// Cria o dispositivo virtual só quando algum passo precisar.
    fn device(&mut self) -> Result<&mut VirtualInput> {
        if self.input.is_none() {
            self.input = Some(VirtualInput::new()?);
        }
        Ok(self.input.as_mut().expect("dispositivo acabou de ser criado"))
    }
}

/// Texto para o log, sem expor o que é digitado.
fn describe(step: &Step) -> String {
    match step {
        Step::Type(text) => format!("type ({} caracteres)", text.chars().count()),
        other => other.to_string(),
    }
}

/// Posição global do clique; janela-alvo é focada antes.
fn resolve_click(hypr: &Hypr, target: &ClickTarget, timeout: Duration) -> Result<(i32, i32)> {
    match target {
        ClickTarget::Absolute { x, y } => Ok((*x, *y)),
        ClickTarget::Window { class, nth, x, y } => {
            let window = nth_window(hypr, class, *nth, timeout)?;
            hypr.focus_window(&window.address)?;
            std::thread::sleep(CLICK_MOVE_SETTLE);
            let window = nth_window(hypr, class, *nth, timeout)?;
            Ok((window.at[0] + x, window.at[1] + y))
        }
    }
}
