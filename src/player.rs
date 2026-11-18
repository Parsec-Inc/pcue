use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::config::Config;
use crate::constants::{CLICK_MOVE_SETTLE, MOVE_FRAME};
use crate::launcher::{LaunchSpec, anchored_regex, launch, nth_window, windows_of};
use crate::platform::{Desktop, VirtualInput};
use crate::progress::{self, Progress};
use crate::script::{ClickTarget, Guard, Input, Line, MAX_CALL_DEPTH, Step, WindowRef, parse_steps};
use crate::keys::Key;
use crate::utils::log;

/// Passo que interrompeu a rotina.
pub struct Failure {
    pub line: usize,
    pub error: anyhow::Error,
}

/// Executa passos, inclusive rotinas chamadas por `call`.
pub struct Player<'a> {
    config: &'a Config,
    desktop: &'a Desktop,
    timeout: Duration,
    input: Option<VirtualInput>,
    stack: Vec<String>,
    /// Teclas e botões apertados por `hold`, soltos no fim da rotina.
    held: Vec<Input>,
}

impl<'a> Player<'a> {
    pub fn new(config: &'a Config, desktop: &'a Desktop) -> Self {
        Self {
            config,
            desktop,
            timeout: Duration::from_secs(config.settings.timeout_secs),
            input: None,
            stack: Vec::new(),
            held: Vec::new(),
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
        if top {
            self.release_to(0);
        }
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
        Ok(!windows_of(self.desktop.windows()?, &anchored_regex(class)?).is_empty())
    }

    fn run_step(&mut self, step: &Step) -> Result<()> {
        let (desktop, timeout) = (self.desktop, self.timeout);
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
                launch(desktop, &spec, timeout)
            }
            Step::WaitWindow { class, nth, timeout } => {
                if desktop.manager().is_none() {
                    log("sem gerenciador de janelas: wait_window vira espera fixa do timeout");
                    std::thread::sleep(*timeout);
                    return Ok(());
                }
                nth_window(desktop, class, *nth, *timeout).map(|_| ())
            }
            Step::Focus { class, nth, timeout } => {
                let window = nth_window(desktop, class, *nth, *timeout)?;
                desktop.focus_window(&window)
            }
            Step::WaitLayer { namespace, timeout } => desktop.wait_layer(namespace, *timeout),
            Step::Workspace(workspace) => desktop.focus_workspace(workspace),
            Step::WaitMonitors(timeout) => desktop.wait_monitors(*timeout),
            Step::Sleep(duration) => {
                std::thread::sleep(*duration);
                Ok(())
            }
            Step::Key(keys) => self.device()?.combo(keys),
            Step::Type(text) => self.device()?.type_text(text),
            Step::Scroll(amount) => self.device()?.scroll(*amount),
            Step::Click { button, hold, target } => {
                self.point_at(target)?;
                self.scoped(|player| {
                    player.press_keys(hold)?;
                    player.device()?.click(*button)
                })
            }
            Step::Call(name) => self.call(name),
            Step::Hold { input, at } => {
                if let Some(target) = at {
                    self.point_at(target)?;
                }
                self.press(*input)
            }
            Step::Release(input) => self.release(*input),
            Step::Move { target, duration } => {
                let to = resolve_click(desktop, target, timeout)?;
                let from = if duration.is_zero() { to } else { desktop.cursor_pos()? };
                self.glide(from, to, *duration)
            }
            Step::Drag { button, hold, window, from, to, duration } => {
                let origin = window_origin(desktop, window.as_ref(), timeout)?;
                let start = (origin.0 + from.0, origin.1 + from.1);
                let end = (origin.0 + to.0, origin.1 + to.1);
                self.scoped(|player| {
                    player.press_keys(hold)?;
                    player.move_and_settle(start)?;
                    player.press(Input::Mouse(*button))?;
                    player.glide(start, end, *duration)
                })
            }
        }
    }

    fn point_at(&self, target: &ClickTarget) -> Result<()> {
        self.move_and_settle(resolve_click(self.desktop, target, self.timeout)?)
    }

    fn move_and_settle(&self, (x, y): (i32, i32)) -> Result<()> {
        self.desktop.move_cursor(x, y)?;
        std::thread::sleep(CLICK_MOVE_SETTLE);
        Ok(())
    }

    fn press(&mut self, input: Input) -> Result<()> {
        self.device()?.press(input)?;
        if !self.held.contains(&input) {
            self.held.push(input);
        }
        Ok(())
    }

    fn press_keys(&mut self, keys: &[Key]) -> Result<()> {
        keys.iter().try_for_each(|key| self.press(Input::Key(*key)))
    }

    fn release(&mut self, input: Input) -> Result<()> {
        self.held.retain(|held| *held != input);
        self.device()?.release(input)
    }

    /// Roda `action` e solta o que ela apertou, mesmo se falhar; o que já estava seguro continua.
    fn scoped(&mut self, action: impl FnOnce(&mut Self) -> Result<()>) -> Result<()> {
        let mark = self.held.len();
        let result = action(self);
        self.release_to(mark);
        result
    }

    /// Solta, na ordem inversa, o que foi apertado depois de `mark`.
    fn release_to(&mut self, mark: usize) {
        let pressed = self.held.split_off(mark.min(self.held.len()));
        if let Some(device) = self.input.as_mut() {
            for input in pressed.into_iter().rev() {
                let _ = device.release(input);
            }
        }
    }

    /// Leva o cursor de `from` a `to` ao longo de `duration` (direto, sem duração).
    fn glide(&self, from: (i32, i32), to: (i32, i32), duration: Duration) -> Result<()> {
        let frames = (duration.as_millis() / MOVE_FRAME.as_millis()).max(1) as i32;
        if frames == 1 {
            return self.desktop.move_cursor(to.0, to.1);
        }
        let start = Instant::now();
        let mut last = from;
        for frame in 1..=frames {
            let point = (from.0 + (to.0 - from.0) * frame / frames, from.1 + (to.1 - from.1) * frame / frames);
            if point != last {
                self.desktop.move_cursor(point.0, point.1)?;
                last = point;
            }
            if frame < frames {
                let deadline = start + MOVE_FRAME * frame as u32;
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            }
        }
        Ok(())
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
fn resolve_click(desktop: &Desktop, target: &ClickTarget, timeout: Duration) -> Result<(i32, i32)> {
    let (left, top) = window_origin(desktop, target.window.as_ref(), timeout)?;
    Ok((left + target.x, top + target.y))
}

/// Canto da janela (focada antes), ou 0,0 para coordenadas absolutas.
fn window_origin(desktop: &Desktop, window: Option<&WindowRef>, timeout: Duration) -> Result<(i32, i32)> {
    let Some((class, nth)) = window else { return Ok((0, 0)) };
    let found = nth_window(desktop, class, *nth, timeout)?;
    desktop.focus_window(&found)?;
    std::thread::sleep(CLICK_MOVE_SETTLE);
    let found = nth_window(desktop, class, *nth, timeout)?;
    Ok((found.at[0], found.at[1]))
}
