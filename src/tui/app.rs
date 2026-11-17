use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::text_input::TextInput;
use crate::config::{Config, Routine, Trigger, validate_name};
use crate::constants::{BUILTIN_EVENTS, TUI_RUN_DELAY_MS};
use crate::history::{self, Run};
use crate::hypr::Hypr;
use crate::schedule::parse_daily_input;
use crate::runner::spawn_run;
use crate::script::{SYNTAX, is_comment, parse_line, parse_steps};
use crate::utils::{log_path, runtime_dir};
use crate::{capture, daemon, install, recorder};

/// Intervalo entre atualizações de status do daemon e histórico.
const REFRESH_EVERY: Duration = Duration::from_secs(2);
/// Quanto tempo uma mensagem fica na barra.
const STATUS_TTL: Duration = Duration::from_secs(6);
/// Linhas do log mostradas no visualizador.
const LOG_TAIL: usize = 400;
/// Prefixo das linhas de ajuda no arquivo aberto no editor.
const EDITOR_HELP_PREFIX: &str = "#|";

/// Painel com foco.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Routines,
    Triggers,
    Steps,
}

/// Janela sobreposta ativa.
pub enum Modal {
    Input {
        title: String,
        hint: String,
        input: TextInput,
        purpose: InputPurpose,
        error: Option<String>,
    },
    Confirm {
        prompt: String,
        action: ConfirmAction,
    },
    Pick {
        title: String,
        options: Vec<(String, String)>,
        selected: usize,
        purpose: PickPurpose,
    },
    Help,
    Log {
        lines: Vec<String>,
        scroll: usize,
    },
}

/// O que fazer com o texto digitado.
#[derive(Clone, Copy)]
pub enum InputPurpose {
    NewRoutine,
    RenameRoutine,
    AddStep,
    EditStep(usize),
    Daily(Option<usize>),
    Interval(Option<usize>),
    CustomEvent(Option<usize>),
}

/// O que fazer com a opção escolhida.
#[derive(Clone, Copy)]
pub enum PickPurpose {
    TriggerKind(Option<usize>),
    Event(Option<usize>),
}

/// Ação destrutiva aguardando confirmação.
#[derive(Clone, Copy)]
pub enum ConfirmAction {
    DeleteRoutine,
    DeleteStep(usize),
    DeleteTrigger(usize),
    MakeFree,
}

/// Ações que precisam do terminal fora da TUI.
pub enum External {
    Record,
    Editor,
}

/// Mensagem na barra de status.
pub struct Status {
    pub text: String,
    pub error: bool,
    at: Instant,
}

/// Estado completo da interface.
pub struct App {
    path: PathBuf,
    pub config: Config,
    pub selected: usize,
    pub focus: Pane,
    pub step_selected: usize,
    pub trigger_selected: usize,
    pub modal: Option<Modal>,
    pub status: Option<Status>,
    pub history: HashMap<String, Run>,
    history_modified: Option<SystemTime>,
    pub daemon_up: bool,
    pub installed: bool,
    pub hyprland: bool,
    editor_draft: Option<String>,
    external: Option<External>,
    last_refresh: Instant,
    quit: bool,
}

impl App {
    pub fn new(path: PathBuf) -> Result<Self> {
        let config = Config::load(&path)?;
        Ok(Self {
            path,
            config,
            selected: 0,
            focus: Pane::Routines,
            step_selected: 0,
            trigger_selected: 0,
            modal: None,
            status: None,
            history: history::latest(),
            history_modified: history::modified(),
            daemon_up: daemon::is_running(),
            installed: install::is_installed(),
            hyprland: crate::hypr::in_session(),
            editor_draft: None,
            external: None,
            last_refresh: Instant::now(),
            quit: false,
        })
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    pub fn take_external(&mut self) -> Option<External> {
        self.external.take()
    }

    /// Rotina selecionada.
    pub fn routine(&self) -> Option<&Routine> {
        self.config.routines.get(self.selected)
    }

    /// Mensagem atual, se ainda válida.
    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref().filter(|s| s.at.elapsed() < STATUS_TTL)
    }

    /// Atualiza daemon e histórico periodicamente.
    pub fn refresh(&mut self) {
        if self.last_refresh.elapsed() < REFRESH_EVERY {
            return;
        }
        self.last_refresh = Instant::now();
        self.daemon_up = daemon::is_running();
        let modified = history::modified();
        if modified != self.history_modified {
            self.history_modified = modified;
            self.history = history::latest();
        }
    }

    fn info(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: false,
            at: Instant::now(),
        });
    }

    fn error(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: true,
            at: Instant::now(),
        });
    }

    /// Aplica a mudança numa cópia; só grava se continuar válida.
    fn mutate(&mut self, change: impl FnOnce(&mut Config) -> Result<()>) -> Result<()> {
        let mut draft = self.config.clone();
        change(&mut draft)?;
        draft.save(&self.path)?;
        self.config = draft;
        self.clamp();
        Ok(())
    }

    /// Mudança na rotina selecionada.
    fn mutate_routine(&mut self, change: impl FnOnce(&mut Routine)) -> Result<()> {
        let index = self.selected;
        self.mutate(|config| {
            change(config.routines.get_mut(index).context("nenhuma rotina selecionada")?);
            Ok(())
        })
    }

    /// Mostra o erro na barra, se houver.
    fn report(&mut self, result: Result<()>) {
        if let Err(err) = result {
            self.error(format!("{err:#}"));
        }
    }

    fn clamp(&mut self) {
        let last = |len: usize| len.saturating_sub(1);
        self.selected = self.selected.min(last(self.config.routines.len()));
        let (steps, triggers) = self
            .routine()
            .map_or((0, 0), |r| (r.steps.len(), r.triggers.len()));
        self.step_selected = self.step_selected.min(last(steps));
        self.trigger_selected = self.trigger_selected.min(last(triggers));
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.modal.is_some() {
            self.on_modal_key(key);
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.modal = Some(Modal::Help),
            KeyCode::Char('L') => self.open_log(),
            KeyCode::Char('I') => self.toggle_install(),
            KeyCode::Tab | KeyCode::Right => self.cycle_focus(true),
            KeyCode::BackTab | KeyCode::Left => self.cycle_focus(false),
            KeyCode::Esc => self.focus = Pane::Routines,
            _ => match self.focus {
                Pane::Routines => self.on_routines_key(key),
                Pane::Triggers => self.on_triggers_key(key),
                Pane::Steps => self.on_steps_key(key),
            },
        }
    }

    fn cycle_focus(&mut self, forward: bool) {
        if self.routine().is_none() {
            return;
        }
        let order = [Pane::Routines, Pane::Triggers, Pane::Steps];
        let index = order.iter().position(|p| *p == self.focus).unwrap_or(0);
        let next = if forward { index + 1 } else { index + order.len() - 1 };
        self.focus = order[next % order.len()];
    }

    fn on_routines_key(&mut self, key: KeyEvent) {
        if let Some(index) = moved(self.selected, self.config.routines.len(), key.code) {
            self.selected = index;
            self.clamp();
            return;
        }
        match key.code {
            KeyCode::Char('n') => self.open_input(
                "Nova rotina",
                "nome: minúsculas, dígitos, - ou _",
                "",
                InputPurpose::NewRoutine,
            ),
            _ if self.routine().is_none() => {}
            KeyCode::Enter | KeyCode::Char('x') => self.run_selected(),
            KeyCode::Char('r') => {
                let name = self.routine().map(|r| r.name.clone()).unwrap_or_default();
                self.open_input("Renomear rotina", "novo nome", &name, InputPurpose::RenameRoutine);
            }
            KeyCode::Char('d') => {
                let name = self.routine().map(|r| r.name.clone()).unwrap_or_default();
                self.confirm(format!("Apagar a rotina '{name}'?"), ConfirmAction::DeleteRoutine);
            }
            KeyCode::Char(' ') => self.toggle_enabled(),
            KeyCode::Char('t') => self.toggle_kind(),
            _ => {}
        }
        self.clamp();
    }

    fn on_triggers_key(&mut self, key: KeyEvent) {
        let count = self.routine().map_or(0, |r| r.triggers.len());
        if let Some(index) = moved(self.trigger_selected, count, key.code) {
            self.trigger_selected = index;
            return;
        }
        match key.code {
            KeyCode::Char('a') => self.pick_trigger_kind(None),
            KeyCode::Enter | KeyCode::Char('e') if count > 0 => self.edit_trigger(self.trigger_selected),
            KeyCode::Char('d') if count > 0 => {
                let index = self.trigger_selected;
                let prompt = if count == 1 {
                    "Apagar o último gatilho? A rotina vira livre.".to_string()
                } else {
                    "Apagar este gatilho?".to_string()
                };
                self.confirm(prompt, ConfirmAction::DeleteTrigger(index));
            }
            _ => {}
        }
    }

    fn on_steps_key(&mut self, key: KeyEvent) {
        let count = self.routine().map_or(0, |r| r.steps.len());
        let index = self.step_selected;
        if let Some(moved) = moved(index, count, key.code) {
            self.step_selected = moved;
            return;
        }
        match key.code {
            KeyCode::Char('a') => self.open_input("Novo passo", &step_hint(), "", InputPurpose::AddStep),
            KeyCode::Char('R') => self.external = Some(External::Record),
            KeyCode::Char('c') => self.capture_windows(),
            KeyCode::Char('E') => self.external = Some(External::Editor),
            _ if count == 0 => {}
            KeyCode::Enter | KeyCode::Char('e') => {
                let current = self.routine().map(|r| r.steps[index].clone()).unwrap_or_default();
                self.open_input("Editar passo", &step_hint(), &current, InputPurpose::EditStep(index));
            }
            KeyCode::Char('d') => self.confirm("Apagar este passo?".into(), ConfirmAction::DeleteStep(index)),
            KeyCode::Char('K') if index > 0 => self.move_step(index, index - 1),
            KeyCode::Char('J') if index + 1 < count => self.move_step(index, index + 1),
            _ => {}
        }
    }

    fn move_step(&mut self, from: usize, to: usize) {
        let result = self.mutate_routine(|r| r.steps.swap(from, to));
        if result.is_ok() {
            self.step_selected = to;
        }
        self.report(result);
    }

    fn open_input(&mut self, title: &str, hint: &str, value: &str, purpose: InputPurpose) {
        self.modal = Some(Modal::Input {
            title: title.to_string(),
            hint: hint.to_string(),
            input: TextInput::with_value(value),
            purpose,
            error: None,
        });
    }

    fn confirm(&mut self, prompt: String, action: ConfirmAction) {
        self.modal = Some(Modal::Confirm { prompt, action });
    }

    fn pick_trigger_kind(&mut self, editing: Option<usize>) {
        self.modal = Some(Modal::Pick {
            title: "Tipo de gatilho".into(),
            options: vec![
                ("evento".into(), "login, bloqueio, desbloqueio, suspensão, monitor...".into()),
                ("diário".into(), "num horário, todos os dias ou em dias escolhidos".into()),
                ("intervalo".into(), "a cada N minutos ou horas".into()),
            ],
            selected: 0,
            purpose: PickPurpose::TriggerKind(editing),
        });
    }

    fn pick_event(&mut self, editing: Option<usize>) {
        let mut options: Vec<(String, String)> = BUILTIN_EVENTS
            .iter()
            .map(|(name, desc)| ((*name).to_string(), (*desc).to_string()))
            .collect();
        options.push(("personalizado…".into(), "disparado por `pwboot emit <nome>`".into()));
        self.modal = Some(Modal::Pick {
            title: "Evento".into(),
            options,
            selected: 0,
            purpose: PickPurpose::Event(editing),
        });
    }

    fn edit_trigger(&mut self, index: usize) {
        let Some(trigger) = self.routine().and_then(|r| r.triggers.get(index)).cloned() else {
            return;
        };
        match trigger {
            Trigger::Event { event } if BUILTIN_EVENTS.iter().any(|(e, _)| *e == event) => {
                self.pick_event(Some(index));
            }
            Trigger::Event { event } => self.open_input(
                "Evento personalizado",
                "nome do evento (pwboot emit <nome>)",
                &event,
                InputPurpose::CustomEvent(Some(index)),
            ),
            Trigger::Daily { at, days } => {
                let days: Vec<&str> = days.iter().map(|d| d.label()).collect();
                let value = format!("{at} {}", days.join(",")).trim().to_string();
                self.open_input("Horário diário", DAILY_HINT, &value, InputPurpose::Daily(Some(index)));
            }
            Trigger::Interval { every } => self.open_input(
                "Intervalo",
                INTERVAL_HINT,
                &every,
                InputPurpose::Interval(Some(index)),
            ),
        }
    }

    fn on_modal_key(&mut self, key: KeyEvent) {
        let Some(modal) = self.modal.as_mut() else { return };
        let esc = key.code == KeyCode::Esc;
        match modal {
            Modal::Help => {
                if esc || matches!(key.code, KeyCode::Char('q' | '?')) {
                    self.modal = None;
                }
            }
            Modal::Log { lines, scroll } => {
                let max = lines.len().saturating_sub(1);
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q' | 'L') => self.modal = None,
                    KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => *scroll = (*scroll + 1).min(max),
                    KeyCode::PageUp => *scroll = scroll.saturating_sub(20),
                    KeyCode::PageDown => *scroll = (*scroll + 20).min(max),
                    KeyCode::Home | KeyCode::Char('g') => *scroll = 0,
                    KeyCode::End | KeyCode::Char('G') => *scroll = max,
                    _ => {}
                }
            }
            Modal::Confirm { action, .. } => match key.code {
                KeyCode::Char('s' | 'y') | KeyCode::Enter => {
                    let action = *action;
                    self.modal = None;
                    self.apply_confirm(action);
                }
                KeyCode::Char('n') | KeyCode::Esc => self.modal = None,
                _ => {}
            },
            Modal::Pick {
                options,
                selected,
                purpose,
                ..
            } => {
                if let Some(index) = moved(*selected, options.len(), key.code) {
                    *selected = index;
                } else if esc {
                    self.modal = None;
                } else if key.code == KeyCode::Enter {
                    let (purpose, index) = (*purpose, *selected);
                    let choice = options[index].0.clone();
                    self.modal = None;
                    self.apply_pick(purpose, index, &choice);
                }
            }
            Modal::Input {
                input,
                purpose,
                error,
                ..
            } => match key.code {
                KeyCode::Esc => self.modal = None,
                KeyCode::Enter => {
                    let (purpose, value) = (*purpose, input.value().trim().to_string());
                    match self.apply_input(purpose, &value) {
                        Ok(()) => self.modal = None,
                        Err(err) => {
                            if let Some(Modal::Input { error, .. }) = &mut self.modal {
                                *error = Some(format!("{err:#}"));
                            }
                        }
                    }
                }
                _ => {
                    if input.handle(key) {
                        *error = None;
                    }
                }
            },
        }
    }

    /// Aplica o texto do campo; erro mantém o campo aberto.
    fn apply_input(&mut self, purpose: InputPurpose, value: &str) -> Result<()> {
        match purpose {
            InputPurpose::NewRoutine => {
                validate_name(value)?;
                if self.config.find(value).is_some() {
                    bail!("já existe uma rotina '{value}'");
                }
                self.mutate(|c| c.routine_or_create(value).map(|_| ()))?;
                self.selected = self.config.routines.len() - 1;
                self.focus = Pane::Steps;
                self.info(format!("rotina '{value}' criada (livre); `a` adiciona passos, `R` grava"));
            }
            InputPurpose::RenameRoutine => {
                validate_name(value)?;
                let new = value.to_string();
                self.mutate_routine(|r| {
                    r.name = new;
                })?;
            }
            InputPurpose::AddStep => {
                let line = check_step(value)?;
                let at = self.routine().map_or(0, |r| (self.step_selected + 1).min(r.steps.len()));
                self.mutate_routine(|r| {
                    r.steps.insert(at, line);
                })?;
                self.step_selected = at;
            }
            InputPurpose::EditStep(index) => {
                let line = check_step(value)?;
                self.mutate_routine(|r| {
                    r.steps[index] = line;
                })?;
            }
            InputPurpose::Daily(editing) => self.save_trigger(editing, parse_daily_input(value)?)?,
            InputPurpose::Interval(editing) => self.save_trigger(
                editing,
                Trigger::Interval {
                    every: value.to_string(),
                },
            )?,
            InputPurpose::CustomEvent(editing) => {
                validate_name(value)?;
                self.save_trigger(
                    editing,
                    Trigger::Event {
                        event: value.to_string(),
                    },
                )?;
            }
        }
        Ok(())
    }

    fn apply_pick(&mut self, purpose: PickPurpose, index: usize, choice: &str) {
        match purpose {
            PickPurpose::TriggerKind(editing) => match index {
                0 => self.pick_event(editing),
                1 => self.open_input("Horário diário", DAILY_HINT, "", InputPurpose::Daily(editing)),
                _ => self.open_input("Intervalo", INTERVAL_HINT, "", InputPurpose::Interval(editing)),
            },
            PickPurpose::Event(editing) if index >= BUILTIN_EVENTS.len() => self.open_input(
                "Evento personalizado",
                "nome do evento (pwboot emit <nome>)",
                "",
                InputPurpose::CustomEvent(editing),
            ),
            PickPurpose::Event(editing) => {
                let trigger = Trigger::Event {
                    event: choice.to_string(),
                };
                let result = self.save_trigger(editing, trigger);
                self.report(result);
            }
        }
    }

    /// Adiciona ou troca um gatilho; com gatilho a rotina é agendada.
    fn save_trigger(&mut self, editing: Option<usize>, trigger: Trigger) -> Result<()> {
        trigger.validate()?;
        self.mutate_routine(|r| {
            match editing {
                Some(index) => r.triggers[index] = trigger,
                None => r.triggers.push(trigger),
            }
        })?;
        self.focus = Pane::Triggers;
        if editing.is_none() {
            self.trigger_selected = self.routine().map_or(0, |r| r.triggers.len() - 1);
        }
        if !self.daemon_up {
            self.info("gatilho salvo; o daemon não está rodando (I registra no autostart)");
        }
        Ok(())
    }

    fn apply_confirm(&mut self, action: ConfirmAction) {
        let result = match action {
            ConfirmAction::DeleteRoutine => {
                let index = self.selected;
                self.focus = Pane::Routines;
                self.mutate(|c| {
                    c.routines.remove(index);
                    Ok(())
                })
            }
            ConfirmAction::DeleteStep(index) => self.mutate_routine(|r| {
                r.steps.remove(index);
            }),
            ConfirmAction::DeleteTrigger(index) => self.mutate_routine(|r| {
                r.triggers.remove(index);
            }),
            ConfirmAction::MakeFree => self.mutate_routine(|r| {
                r.triggers.clear();
            }),
        };
        self.report(result);
    }

    fn toggle_enabled(&mut self) {
        let result = self.mutate_routine(|r| r.enabled = !r.enabled);
        self.report(result);
    }

    /// Livre vira agendada pedindo um gatilho; agendada vira livre com confirmação.
    fn toggle_kind(&mut self) {
        match self.routine().map(Routine::is_scheduled) {
            Some(false) => self.pick_trigger_kind(None),
            Some(true) => self.confirm(
                "Tornar livre? Os gatilhos serão apagados.".into(),
                ConfirmAction::MakeFree,
            ),
            None => {}
        }
    }

    /// Roda a rotina num processo separado, com tempo para trocar de janela.
    fn run_selected(&mut self) {
        let Some(name) = self.routine().map(|r| r.name.clone()) else { return };
        match spawn_run(&name, "manual", TUI_RUN_DELAY_MS) {
            Ok(_) => self.info(format!("'{name}' começa em {:.1}s (L mostra o log)", TUI_RUN_DELAY_MS as f64 / 1000.0)),
            Err(err) => self.error(format!("falha ao executar: {err}")),
        }
    }

    fn capture_windows(&mut self) {
        let steps = match capture::capture(&[]) {
            Ok(steps) => steps,
            Err(err) => return self.error(format!("captura falhou: {err:#}")),
        };
        let total = steps.len();
        match self.mutate_routine(|r| r.steps.extend(steps)) {
            Ok(()) => self.info(format!("{total} janela(s) capturada(s) como `launch`")),
            Err(err) => self.error(format!("{err:#}")),
        }
    }

    fn toggle_install(&mut self) {
        let result = if self.installed {
            install::uninstall().map(|f| f.map(|p| format!("removido do autostart ({})", p.display())))
        } else {
            install::install().map(|p| Some(format!("daemon registrado em {}", p.display())))
        };
        match result {
            Ok(message) => {
                self.installed = install::is_installed();
                self.info(message.unwrap_or_else(|| "nada a remover".into()));
            }
            Err(err) => self.error(format!("{err:#}")),
        }
    }

    fn open_log(&mut self) {
        let lines: Vec<String> = log_path()
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|raw| raw.lines().map(String::from).collect())
            .unwrap_or_default();
        let start = lines.len().saturating_sub(LOG_TAIL);
        let lines = lines[start..].to_vec();
        let scroll = lines.len().saturating_sub(1);
        self.modal = Some(Modal::Log { lines, scroll });
    }

    /// Executa gravação ou editor com o terminal liberado.
    pub fn run_external(&mut self, external: External) {
        let result = match external {
            External::Record => self.record(),
            External::Editor => self.edit_in_editor(),
        };
        if let Err(err) = result {
            self.error(format!("{}: {err:#}", external.label()));
        }
    }

    fn record(&mut self) -> Result<()> {
        let steps = recorder::record(Hypr::detect().as_ref())?;
        let total = steps.len();
        self.mutate_routine(|r| {
            r.steps.extend(steps);
        })?;
        self.step_selected = self.routine().map_or(0, |r| r.steps.len().saturating_sub(1));
        self.info(format!("{total} passo(s) gravados"));
        Ok(())
    }

    /// Edita todos os passos no $EDITOR; erro guarda o rascunho para tentar de novo.
    fn edit_in_editor(&mut self) -> Result<()> {
        let Some(routine) = self.routine() else { return Ok(()) };
        let name = routine.name.clone();
        let current = routine.steps.join("\n");
        let body = self.editor_draft.take().unwrap_or(current);
        let content = format!("{}\n{body}\n", editor_header(&name));
        let dir = runtime_dir()?;
        std::fs::create_dir_all(&dir)?;
        let file = dir.join(format!("{name}.pwb"));
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&file)?
            .write_all(content.as_bytes())?;

        let editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_else(|_| "nano".into());
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("{editor} \"$1\""))
            .arg("sh")
            .arg(&file)
            .status()
            .with_context(|| format!("falha ao abrir {editor}"))?;
        let edited = std::fs::read_to_string(&file)?;
        let _ = std::fs::remove_file(&file);
        if !status.success() {
            bail!("editor saiu com erro; nada mudou");
        }

        let steps: Vec<String> = edited
            .lines()
            .filter(|l| !l.starts_with(EDITOR_HELP_PREFIX))
            .map(|l| l.trim_end().to_string())
            .skip_while(|l| l.is_empty())
            .collect();
        let steps = trim_trailing_empty(steps);
        if let Err(err) = parse_steps(&steps) {
            self.editor_draft = Some(steps.join("\n"));
            bail!("{err:#} (E reabre o rascunho)");
        }
        let total = steps.iter().filter(|l| !is_comment(l)).count();
        self.mutate_routine(|r| {
            r.steps = steps;
        })?;
        self.info(format!("{total} passo(s) salvos"));
        Ok(())
    }
}

/// Valida um passo digitado e devolve a linha a guardar.
fn check_step(value: &str) -> Result<String> {
    if value.is_empty() {
        bail!("passo vazio");
    }
    if !is_comment(value) {
        parse_line(value)?;
    }
    Ok(value.to_string())
}

fn trim_trailing_empty(mut lines: Vec<String>) -> Vec<String> {
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

/// Cabeçalho de ajuda do arquivo aberto no editor.
fn editor_header(name: &str) -> String {
    std::iter::once(format!(
        "Passos da rotina '{name}'. Linhas com {EDITOR_HELP_PREFIX} são ignoradas."
    ))
    .chain(SYNTAX.iter().map(|line| (*line).to_string()))
    .map(|line| format!("{EDITOR_HELP_PREFIX} {line}"))
    .collect::<Vec<_>>()
    .join("\n")
}

/// Dica de sintaxe mostrada ao editar um passo.
fn step_hint() -> String {
    SYNTAX.join(" · ")
}

/// Novo índice se a tecla for de navegação vertical.
fn moved(index: usize, len: usize, key: KeyCode) -> Option<usize> {
    match key {
        KeyCode::Up | KeyCode::Char('k') => Some(index.saturating_sub(1)),
        KeyCode::Down | KeyCode::Char('j') => Some((index + 1).min(len.saturating_sub(1))),
        _ => None,
    }
}
const DAILY_HINT: &str = "HH:MM e dias opcionais, ex.: 09:00 seg,ter,qua,qui,sex";
const INTERVAL_HINT: &str = "ex.: 30m, 2h ou 1.5h (mínimo 1m)";
