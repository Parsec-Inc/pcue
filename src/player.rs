use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::constants::CLICK_MOVE_SETTLE;
use crate::hypr::Hypr;
use crate::input::VirtualInput;
use crate::launcher::{LaunchSpec, find_window, launch, wait_any_window};
use crate::script::{ClickTarget, Line, Step};
use crate::utils::log;

/// Executa os passos em ordem; qualquer falha interrompe a rotina.
pub fn play(routine: &str, lines: &[Line], hypr: Option<&Hypr>, timeout: Duration) -> Result<()> {
    let mut input: Option<VirtualInput> = None;
    for line in lines {
        log(&format!("[{routine}] {}: {}", line.number, describe(&line.step)));
        run_step(&line.step, hypr, timeout, &mut input)
            .with_context(|| format!("linha {}", line.number))?;
    }
    Ok(())
}

/// Texto para o log, sem expor o que é digitado.
fn describe(step: &Step) -> String {
    match step {
        Step::Type(text) => format!("type ({} caracteres)", text.chars().count()),
        other => other.to_string(),
    }
}

fn run_step(
    step: &Step,
    hypr: Option<&Hypr>,
    timeout: Duration,
    input: &mut Option<VirtualInput>,
) -> Result<()> {
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
        Step::WaitWindow { class, timeout } => match hypr {
            Some(hypr) => wait_any_window(hypr, class, *timeout).map(|_| ()),
            None => {
                log("sem Hyprland: wait_window vira espera fixa do timeout");
                std::thread::sleep(*timeout);
                Ok(())
            }
        },
        Step::Sleep(duration) => {
            std::thread::sleep(*duration);
            Ok(())
        }
        Step::Key(keys) => device(input)?.combo(keys),
        Step::Type(text) => device(input)?.type_text(text),
        Step::Scroll(amount) => device(input)?.scroll(*amount),
        Step::Click { button, target } => {
            let Some(hypr) = hypr else {
                bail!("click precisa do Hyprland para posicionar o cursor");
            };
            let (x, y) = resolve_click(hypr, target)?;
            hypr.move_cursor(x, y)?;
            std::thread::sleep(CLICK_MOVE_SETTLE);
            device(input)?.click(*button)
        }
    }
}

/// Posição global do clique; janela-alvo é focada antes.
fn resolve_click(hypr: &Hypr, target: &ClickTarget) -> Result<(i32, i32)> {
    match target {
        ClickTarget::Absolute { x, y } => Ok((*x, *y)),
        ClickTarget::Window { class, x, y } => {
            let window = find_window(hypr, class)?
                .with_context(|| format!("nenhuma janela {class} para clicar"))?;
            hypr.focus_window(&window.address)?;
            std::thread::sleep(CLICK_MOVE_SETTLE);
            let window = find_window(hypr, class)?.unwrap_or(window);
            Ok((window.at[0] + x, window.at[1] + y))
        }
    }
}

/// Cria o dispositivo virtual só quando algum passo precisar.
fn device(input: &mut Option<VirtualInput>) -> Result<&mut VirtualInput> {
    if input.is_none() {
        *input = Some(VirtualInput::new()?);
    }
    Ok(input.as_mut().expect("dispositivo acabou de ser criado"))
}
