use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::config::Config;
use crate::constants::{CLICK_MOVE_SETTLE, MOVE_FRAME};
use crate::launcher::{LaunchSpec, anchored_regex, launch, nth_window, windows_of};
use crate::platform::{Desktop, VirtualInput};
use crate::progress::{self, Progress};
use crate::script::{Arg, ClickTarget, Guard, Input, Line, MAX_CALL_DEPTH, Step, Waypoint, WindowRef};
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
            Step::Call { routine, args } => self.call(routine, args),
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
            Step::Drag { button, hold, window, from, via, to, duration } => {
                let origin = window_origin(desktop, window.as_ref(), timeout)?;
                let shift = |(x, y): (i32, i32)| (origin.0 + x, origin.1 + y);
                let path: Vec<Waypoint> = std::iter::once((Duration::ZERO, *from))
                    .chain(via.iter().copied())
                    .chain(std::iter::once((*duration, *to)))
                    .map(|(at, point)| (at, shift(point)))
                    .collect();
                self.scoped(|player| {
                    player.press_keys(hold)?;
                    player.move_and_settle(path[0].1)?;
                    player.press(Input::Mouse(*button))?;
                    player.trace(&path)
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
        self.trace(&[(Duration::ZERO, from), (duration, to)])
    }

    /// Percorre o trajeto respeitando quando passar por cada ponto.
    fn trace(&self, path: &[Waypoint]) -> Result<()> {
        let Some(&(total, end)) = path.last() else { return Ok(()) };
        let start = Instant::now();
        let mut last = path[0].1;
        let mut elapsed = MOVE_FRAME;
        while elapsed < total {
            let point = position_on(path, elapsed);
            if point != last {
                self.desktop.move_cursor(point.0, point.1)?;
                last = point;
            }
            std::thread::sleep((start + elapsed).saturating_duration_since(Instant::now()));
            elapsed += MOVE_FRAME;
        }
        if last != end {
            self.desktop.move_cursor(end.0, end.1)?;
        }
        Ok(())
    }

    fn call(&mut self, name: &str, args: &[Arg]) -> Result<()> {
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
        let lines = routine.lines(args)?;
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

/// Posição no trajeto num instante, interpolando entre os pontos vizinhos.
fn position_on(path: &[Waypoint], at: Duration) -> (i32, i32) {
    let Some(next) = path.iter().position(|(t, _)| *t >= at) else { return path[path.len() - 1].1 };
    let Some(prev) = next.checked_sub(1) else { return path[next].1 };
    let ((t0, (x0, y0)), (t1, (x1, y1))) = (path[prev], path[next]);
    let span = (t1 - t0).as_secs_f64();
    let k = if span > 0.0 { (at - t0).as_secs_f64() / span } else { 1.0 };
    let lerp = |a: i32, b: i32| a + ((b - a) as f64 * k).round() as i32;
    (lerp(x0, x1), lerp(y0, y1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_goes_past_and_comes_back() {
        let ms = Duration::from_millis;
        let path = [(ms(0), (0, 0)), (ms(100), (100, 0)), (ms(300), (60, 20))];
        assert_eq!(position_on(&path, ms(50)), (50, 0));
        assert_eq!(position_on(&path, ms(100)), (100, 0));
        assert_eq!(position_on(&path, ms(200)), (80, 10));
        assert_eq!(position_on(&path, ms(999)), (60, 20));
    }
}
