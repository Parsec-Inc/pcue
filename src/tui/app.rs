use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::anim::Slide;
use super::text_input::TextInput;
use crate::config::{Config, Routine, Trigger, validate_name};
use crate::constants::{BOOT_EVENT, BUILTIN_EVENTS, DEFAULT_EDITOR, DESKTOP_MARK, TUI_RUN_DELAY_MS};
use crate::history::{self, Run};
use crate::progress::{self, Progress};
use crate::draft::Draft;
use crate::recorder::{self, RecordEvent};
use crate::runner::{Selection, spawn_run};
use crate::schedule::parse_daily_input;
use crate::script::{SYNTAX, is_comment, parse_line, parse_steps};
use crate::utils::{expand_home, log_path, runtime_dir};
use crate::platform::{self as install, Desktop};
use crate::update::{self, Release, UpdateEvent};
use crate::{boot, capture, daemon};

/// Intervalo entre leituras de daemon, histórico e agenda.
const REFRESH_EVERY: Duration = Duration::from_secs(2);
/// Intervalo entre leituras do progresso das execuções.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// Quanto tempo uma mensagem fica na barra.
const STATUS_TTL: Duration = Duration::from_secs(6);
/// Quanto tempo o resultado de uma execução fica em destaque.
pub const CELEBRATE_FOR: Duration = Duration::from_millis(2500);
/// Dois cliques dentro deste intervalo contam como duplo.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
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

/// Região clicável registrada no último desenho.
#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    Routine(usize),
    Trigger(usize),
    Step(usize),
    Pane(Pane),
    Key(KeyCode),
    Option(usize),
    HistoryRow(usize),
    ReviewRow(usize),
    Backdrop,
    StopRecording,
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
    History {
        runs: Vec<Run>,
        selected: usize,
        only: Option<String>,
    },
    /// Revisão da gravação: escolher a forma de cada trecho antes de salvar.
    Review {
        routine: String,
        draft: Draft,
        selected: usize,
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
    Export,
    Import,
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
    ClearSteps,
    DeleteTrigger(usize),
    MakeFree,
    Update,
    Discard,
    Quit,
    SaveAndRun(Selection),
}

/// Mensagem na barra de status.
pub struct Status {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

/// Filtro da lista de rotinas.
pub struct Filter {
    pub input: TextInput,
    pub editing: bool,
}

/// Fase da gravação.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecPhase {
    Countdown(u64),
    Recording,
    Paused,
}

/// Gravação acontecendo dentro da TUI.
pub struct Recording {
    session: recorder::Session,
    pub routine: String,
    pub phase: RecPhase,
    pub steps: Vec<String>,
    /// Ações gravadas (sem contar as mudanças do desktop).
    pub actions: usize,
    pub started: Option<Instant>,
    pub last_step: Instant,
}

/// Resultado recente de uma execução, para destacar.
pub struct Finished {
    pub routine: String,
    pub ok: bool,
    pub at: Instant,
}

/// Seleções animadas de cada lista.
#[derive(Default)]
pub struct Slides {
    pub routines: Slide,
    pub triggers: Slide,
    pub steps: Slide,
    pub modal: Slide,
}

/// Estado completo da interface.
pub struct App {
    path: PathBuf,
    pub config: Config,
    /// Cópia do que está gravado no disco.
    saved: Config,
    pub selected: usize,
    pub focus: Pane,
    pub step_selected: usize,
    pub trigger_selected: usize,
    pub modal: Option<Modal>,
    pub modal_at: Instant,
    pub status: Option<Status>,
    pub runs: Vec<Run>,
    history_modified: Option<SystemTime>,
    pub running: HashMap<String, Progress>,
    pub next_runs: HashMap<String, i64>,
    pub finished: Option<Finished>,
    pub daemon_up: bool,
    pub installed: bool,
    /// Gerenciador de janelas controlado (ex.: Hyprland).
    pub manager: Option<&'static str>,
    /// Release mais nova que a atual, se houver.
    pub update: Option<Release>,
    updates: Option<std::sync::mpsc::Receiver<UpdateEvent>>,
    pub filter: Option<Filter>,
    pub recording: Option<Recording>,
    pub slides: Slides,
    pub flash: Option<Instant>,
    pub hits: Vec<(Rect, Hit)>,
    /// Primeira linha visível de cada lista, mantida entre quadros.
    pub scroll: HashMap<&'static str, usize>,
    pub clock: Instant,
    last_click: Option<(Instant, Hit)>,
    editor_draft: Option<String>,
    children: Vec<std::process::Child>,
    open_editor: bool,
    last_refresh: Instant,
    last_progress: Instant,
    quit: bool,
}

impl App {
    pub fn new(path: PathBuf) -> Result<Self> {
        let config = Config::load(&path)?;
        let runs = history::all();
        Ok(Self {
            path,
            saved: config.clone(),
            config,
            selected: 0,
            focus: Pane::Routines,
            step_selected: 0,
            trigger_selected: 0,
            modal: None,
            modal_at: Instant::now(),
            status: None,
            runs,
            history_modified: history::modified(),
            running: progress::running(),
            next_runs: daemon::next_runs(),
            finished: None,
            daemon_up: daemon::is_running(),
            installed: install::is_installed(),
            manager: install::manager_name(),
            update: None,
            updates: Some(update::check_in_background()),
            filter: None,
            recording: None,
            slides: Slides::default(),
            flash: None,
            hits: Vec::new(),
            scroll: HashMap::new(),
            clock: Instant::now(),
            last_click: None,
            editor_draft: None,
            children: Vec::new(),
            open_editor: false,
            last_refresh: Instant::now(),
            last_progress: Instant::now(),
            quit: false,
        })
    }

    /// Há mudanças que ainda não foram gravadas.
    pub fn dirty(&self) -> bool {
        self.config != self.saved
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// Pedido de abrir o $EDITOR, que precisa do terminal fora da TUI.
    pub fn take_editor_request(&mut self) -> bool {
        std::mem::take(&mut self.open_editor)
    }

    /// Rotina selecionada.
    pub fn routine(&self) -> Option<&Routine> {
        self.config.routines.get(self.selected)
    }

    /// Última execução da rotina.
    pub fn last_run(&self, name: &str) -> Option<&Run> {
        self.runs.iter().rfind(|run| run.routine == name)
    }

    /// Mensagem atual, se ainda válida.
    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref().filter(|s| s.at.elapsed() < STATUS_TTL)
    }

    /// Índices das rotinas que passam no filtro.
    pub fn visible(&self) -> Vec<usize> {
        let needle = self
            .filter
            .as_ref()
            .map(|f| f.input.value().to_lowercase())
            .unwrap_or_default();
        self.config
            .routines
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                needle.is_empty() || r.name.contains(&needle) || r.kind_label().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Algo se mexendo na tela pede quadros mais frequentes.
    pub fn animating(&self) -> bool {
        let recent = |at: Option<Instant>, d: Duration| at.is_some_and(|t| t.elapsed() < d);
        self.recording.is_some()
            || !self.running.is_empty()
            || recent(self.flash, super::anim::FLASH)
            || recent(self.finished.as_ref().map(|f| f.at), CELEBRATE_FOR)
            || self.modal_at.elapsed() < super::anim::POP
            || [&self.slides.routines, &self.slides.triggers, &self.slides.steps, &self.slides.modal]
                .iter()
                .any(|s| s.moving())
    }

    /// Atualiza gravação, progresso, daemon e histórico.
    pub fn refresh(&mut self) {
        self.poll_recording();
        self.poll_update();
        if self.last_progress.elapsed() >= PROGRESS_EVERY {
            self.last_progress = Instant::now();
            self.running = progress::running();
        }
        if self.last_refresh.elapsed() < REFRESH_EVERY {
            return;
        }
        self.last_refresh = Instant::now();
        self.children.retain_mut(|child| matches!(child.try_wait(), Ok(None)));
        self.daemon_up = daemon::is_running();
        self.next_runs = daemon::next_runs();
        let modified = history::modified();
        if modified != self.history_modified {
            self.history_modified = modified;
            let before = self.runs.len();
            self.runs = history::all();
            if let Some(run) = self.runs.last().filter(|_| self.runs.len() > before) {
                self.finished = Some(Finished {
                    routine: run.routine.clone(),
                    ok: run.ok,
                    at: Instant::now(),
                });
            }
        }
    }

    fn info(&mut self, text: impl Into<String>) {
        self.set_status(text.into(), false);
    }

    fn error(&mut self, text: impl Into<String>) {
        self.set_status(text.into(), true);
    }

    fn set_status(&mut self, text: String, error: bool) {
        self.status = Some(Status {
            text,
            error,
            at: Instant::now(),
        });
    }

    fn open_modal(&mut self, modal: Modal) {
        self.modal = Some(modal);
        self.modal_at = Instant::now();
        self.slides.modal = Slide::default();
    }

    /// Aplica a mudança numa cópia; só fica se continuar válida. Não grava.
    fn mutate(&mut self, change: impl FnOnce(&mut Config) -> Result<()>) -> Result<()> {
        let mut draft = self.config.clone();
        change(&mut draft)?;
        draft.validate()?;
        self.config = draft;
        self.clamp();
        Ok(())
    }

    fn save(&mut self) -> Result<()> {
        self.config.save(&self.path)?;
        self.saved = self.config.clone();
        self.flash = Some(Instant::now());
        Ok(())
    }

    fn save_now(&mut self) {
        if !self.dirty() {
            return self.info("nada para salvar");
        }
        match self.save() {
            Ok(()) => self.info("alterações salvas"),
            Err(err) => self.error(format!("{err:#}")),
        }
    }

    fn ask_discard(&mut self) {
        if self.dirty() {
            self.confirm("Descartar as alterações e voltar ao que está salvo?".into(), ConfirmAction::Discard);
        } else {
            self.info("nada para descartar");
        }
    }

    fn ask_quit(&mut self) {
        if self.dirty() {
            self.confirm("Há alterações não salvas. Sair e perder essas alterações?".into(), ConfirmAction::Quit);
        } else {
            self.quit = true;
        }
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
        let visible = self.visible();
        if !visible.contains(&self.selected) {
            self.selected = visible.first().copied().unwrap_or(0);
        }
        self.selected = self.selected.min(last(self.config.routines.len()));
        let (steps, triggers) = self
            .routine()
            .map_or((0, 0), |r| (r.steps.len(), r.triggers.len()));
        self.step_selected = self.step_selected.min(last(steps));
        self.trigger_selected = self.trigger_selected.min(last(triggers));
    }

    fn select_routine(&mut self, index: usize) {
        if index != self.selected {
            self.step_selected = 0;
            self.trigger_selected = 0;
        }
        self.selected = index;
        self.clamp();
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if let Some(recording) = &self.recording {
                recording.session.stop();
            } else if matches!(self.modal, Some(Modal::Confirm { action: ConfirmAction::Quit, .. })) {
                self.quit = true;
            } else {
                self.modal = None;
                self.ask_quit();
            }
            return;
        }
        if self.recording.is_some() {
            return;
        }
        if self.modal.is_some() {
            self.on_modal_key(key);
            return;
        }
        if self.filter.as_ref().is_some_and(|f| f.editing) && self.on_filter_key(key) {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            return self.save_now();
        }
        match key.code {
            KeyCode::Char('q') => self.ask_quit(),
            KeyCode::Char('S') => self.save_now(),
            KeyCode::Char('u') => self.ask_discard(),
            KeyCode::Char('?') => self.open_modal(Modal::Help),
            KeyCode::Char('L') => self.open_log(),
            KeyCode::Char('H') => self.open_history(None),
            KeyCode::Char('I') => self.toggle_install(),
            KeyCode::Char('U') => self.ask_update(),
            KeyCode::Tab | KeyCode::Right => self.cycle_focus(true),
            KeyCode::BackTab | KeyCode::Left => self.cycle_focus(false),
            KeyCode::Esc if self.focus == Pane::Routines && self.filter.is_some() => {
                self.filter = None;
                self.clamp();
            }
            KeyCode::Esc => self.focus = Pane::Routines,
            _ => match self.focus {
                Pane::Routines => self.on_routines_key(key),
                Pane::Triggers => self.on_triggers_key(key),
                Pane::Steps => self.on_steps_key(key),
            },
        }
    }

    /// Teclas do campo de busca; `false` deixa a tecla seguir adiante.
    fn on_filter_key(&mut self, key: KeyEvent) -> bool {
        let Some(filter) = self.filter.as_mut() else { return false };
        match key.code {
            KeyCode::Esc => self.filter = None,
            KeyCode::Enter => {
                filter.editing = false;
                if filter.input.value().is_empty() {
                    self.filter = None;
                }
            }
            KeyCode::Up | KeyCode::Down => return false,
            _ => {
                if !filter.input.handle(key) {
                    return false;
                }
            }
        }
        self.focus = Pane::Routines;
        self.clamp();
        true
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
        let visible = self.visible();
        let position = visible.iter().position(|i| *i == self.selected).unwrap_or(0);
        if let Some(moved) = moved(position, visible.len(), key.code) {
            if let Some(index) = visible.get(moved) {
                self.select_routine(*index);
            }
            return;
        }
        match key.code {
            KeyCode::Char('n') => self.open_input(
                "Nova rotina",
                "nome: minúsculas, dígitos, - ou _",
                "",
                InputPurpose::NewRoutine,
            ),
            KeyCode::Char('/') => {
                let input = self.filter.take().map(|f| f.input).unwrap_or_default();
                self.filter = Some(Filter { input, editing: true });
            }
            KeyCode::Char('i') => self.open_input(
                "Importar rotinas",
                "caminho de um .toml exportado pelo pcue",
                "~/",
                InputPurpose::Import,
            ),
            _ if self.routine().is_none() => {}
            KeyCode::Enter | KeyCode::Char('x') => self.run_selected(Selection::All),
            KeyCode::Char('s') => self.stop_selected(),
            KeyCode::Char('r') => {
                let name = self.routine().map(|r| r.name.clone()).unwrap_or_default();
                self.open_input("Renomear rotina", "novo nome", &name, InputPurpose::RenameRoutine);
            }
            KeyCode::Char('D') => self.duplicate_selected(),
            KeyCode::Char('X') => {
                let name = self.routine().map(|r| r.name.clone()).unwrap_or_default();
                self.open_input(
                    "Exportar rotina",
                    "arquivo .toml de destino",
                    &format!("~/pcue-{name}.toml"),
                    InputPurpose::Export,
                );
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
            KeyCode::Char('R') => self.start_recording(),
            KeyCode::Char('c') => self.capture_windows(),
            KeyCode::Char('E') => self.open_editor = true,
            _ if count == 0 => {}
            KeyCode::Char('C') => {
                let name = self.routine().map(|r| r.name.clone()).unwrap_or_default();
                let prompt = format!("Apagar os {count} passos de '{name}'? Os gatilhos continuam.");
                self.confirm(prompt, ConfirmAction::ClearSteps);
            }
            KeyCode::Enter | KeyCode::Char('e') => self.edit_step(index),
            KeyCode::Char('x') => self.run_step(index, Selection::Only(index + 1)),
            KeyCode::Char('f') => self.run_step(index, Selection::From(index + 1)),
            KeyCode::Char('d') => self.confirm("Apagar este passo?".into(), ConfirmAction::DeleteStep(index)),
            KeyCode::Char('K') if index > 0 => self.move_step(index, index - 1),
            KeyCode::Char('J') if index + 1 < count => self.move_step(index, index + 1),
            _ => {}
        }
    }

    fn edit_step(&mut self, index: usize) {
        let current = self
            .routine()
            .and_then(|r| r.steps.get(index).cloned())
            .unwrap_or_default();
        self.open_input("Editar passo", &step_hint(), &current, InputPurpose::EditStep(index));
    }

    fn move_step(&mut self, from: usize, to: usize) {
        let result = self.mutate_routine(|r| r.steps.swap(from, to));
        if result.is_ok() {
            self.step_selected = to;
        }
        self.report(result);
    }

    /// Clique, duplo clique e rolagem.
    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        let at = Position::new(mouse.column, mouse.row);
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(at))
            .map(|(_, hit)| hit.clone());
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(hit) = hit else { return };
                let double = self
                    .last_click
                    .as_ref()
                    .is_some_and(|(t, h)| *h == hit && t.elapsed() < DOUBLE_CLICK);
                self.last_click = (!double).then(|| (Instant::now(), hit.clone()));
                self.on_hit(hit, double);
            }
            MouseEventKind::ScrollUp => self.on_wheel(hit, KeyCode::Up),
            MouseEventKind::ScrollDown => self.on_wheel(hit, KeyCode::Down),
            _ => {}
        }
    }

    fn on_wheel(&mut self, hit: Option<Hit>, code: KeyCode) {
        if self.recording.is_some() {
            return;
        }
        if self.modal.is_some() {
            self.on_modal_key(KeyEvent::from(code));
            return;
        }
        let pane = match hit {
            Some(Hit::Routine(_) | Hit::Pane(Pane::Routines)) => Pane::Routines,
            Some(Hit::Trigger(_) | Hit::Pane(Pane::Triggers)) => Pane::Triggers,
            Some(Hit::Step(_) | Hit::Pane(Pane::Steps)) => Pane::Steps,
            _ => return,
        };
        self.focus = pane;
        self.on_key(KeyEvent::from(code));
    }

    fn on_hit(&mut self, hit: Hit, double: bool) {
        match hit {
            Hit::StopRecording => {
                if let Some(recording) = &self.recording {
                    recording.session.stop();
                }
            }
            _ if self.recording.is_some() => {}
            Hit::Key(code) => self.on_key(KeyEvent::from(code)),
            Hit::Backdrop => {
                if !matches!(self.modal, Some(Modal::Input { .. } | Modal::Review { .. })) {
                    self.modal = None;
                }
            }
            Hit::Option(index) => {
                if let Some(Modal::Pick { selected, .. }) = &mut self.modal {
                    *selected = index;
                    self.on_modal_key(KeyEvent::from(KeyCode::Enter));
                }
            }
            Hit::ReviewRow(index) => {
                if let Some(Modal::Review { draft, selected, .. }) = &mut self.modal {
                    *selected = index;
                    if double && let Some(item) = draft.items.get_mut(index) {
                        item.cycle(true);
                    }
                }
            }
            Hit::HistoryRow(index) => {
                if let Some(Modal::History { selected, .. }) = &mut self.modal {
                    *selected = index;
                    if double {
                        self.on_modal_key(KeyEvent::from(KeyCode::Enter));
                    }
                }
            }
            Hit::Routine(index) => {
                self.focus = Pane::Routines;
                self.select_routine(index);
                if double {
                    self.run_selected(Selection::All);
                }
            }
            Hit::Trigger(index) => {
                self.focus = Pane::Triggers;
                self.trigger_selected = index;
                if double {
                    self.edit_trigger(index);
                }
            }
            Hit::Step(index) => {
                self.focus = Pane::Steps;
                self.step_selected = index;
                if double {
                    self.edit_step(index);
                }
            }
            Hit::Pane(pane) => {
                if self.routine().is_some() {
                    self.focus = pane;
                }
            }
        }
    }

    fn open_input(&mut self, title: &str, hint: &str, value: &str, purpose: InputPurpose) {
        self.open_modal(Modal::Input {
            title: title.to_string(),
            hint: hint.to_string(),
            input: TextInput::with_value(value),
            purpose,
            error: None,
        });
    }

    fn confirm(&mut self, prompt: String, action: ConfirmAction) {
        self.open_modal(Modal::Confirm { prompt, action });
    }

    fn pick_trigger_kind(&mut self, editing: Option<usize>) {
        self.open_modal(Modal::Pick {
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
        options.push(("personalizado…".into(), "disparado por `pcue emit <nome>`".into()));
        self.open_modal(Modal::Pick {
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
                "nome do evento (pcue emit <nome>)",
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
        let current = self.routine().map(|r| r.name.clone());
        let Some(modal) = self.modal.as_mut() else { return };
        let esc = key.code == KeyCode::Esc;
        match modal {
            Modal::Help => {
                if esc || matches!(key.code, KeyCode::Char('q' | '?') | KeyCode::Enter) {
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
            Modal::History { runs, selected, only } => {
                if let Some(index) = moved(*selected, runs.len(), key.code) {
                    *selected = index;
                    return;
                }
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q' | 'H') => self.modal = None,
                    KeyCode::Char('f') => {
                        let next = if only.is_some() { None } else { current };
                        self.open_history(next);
                    }
                    KeyCode::Enter => {
                        let Some(run) = runs.get(*selected).cloned() else { return };
                        self.modal = None;
                        self.jump_to(&run);
                    }
                    _ => {}
                }
            }
            Modal::Review {
                routine,
                draft,
                selected,
            } => {
                if let Some(index) = moved(*selected, draft.items.len(), key.code) {
                    *selected = index;
                    return;
                }
                let item = draft.items.get_mut(*selected);
                match key.code {
                    KeyCode::Tab
                    | KeyCode::Right
                    | KeyCode::Char('l')
                    | KeyCode::BackTab
                    | KeyCode::Left
                    | KeyCode::Char('h') => {
                        let forward = matches!(key.code, KeyCode::Tab | KeyCode::Right | KeyCode::Char('l'));
                        if let Some(item) = item {
                            item.cycle(forward);
                        }
                    }
                    KeyCode::Char('p') => {
                        if let Some(item) = item.filter(|i| i.pause.is_some()) {
                            item.keep_pause = !item.keep_pause;
                        }
                    }
                    KeyCode::Char('d') | KeyCode::Delete => {
                        if item.is_some() {
                            draft.items.remove(*selected);
                            *selected = (*selected).min(draft.items.len().saturating_sub(1));
                        }
                    }
                    KeyCode::Enter => {
                        let (routine, steps) = (routine.clone(), draft.render());
                        self.modal = None;
                        self.save_recording(&routine, steps);
                    }
                    KeyCode::Esc => {
                        self.modal = None;
                        self.info("gravação descartada");
                    }
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
                        Ok(()) => {
                            if matches!(self.modal, Some(Modal::Input { .. })) {
                                self.modal = None;
                            }
                        }
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
                self.filter = None;
                self.mutate(|c| c.routine_or_create(value).map(|_| ()))?;
                self.select_routine(self.config.routines.len() - 1);
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
            InputPurpose::Export => {
                let name = self.routine().map(|r| r.name.clone()).context("nenhuma rotina")?;
                let path = expand_home(value)?;
                self.config.export(&name, &path)?;
                self.info(format!("'{name}' exportada para {}", path.display()));
            }
            InputPurpose::Import => {
                let path = expand_home(value)?;
                let mut names = Vec::new();
                self.mutate(|c| {
                    names = c.import(&path)?;
                    Ok(())
                })?;
                self.filter = None;
                if let Some(index) = names.first().and_then(|n| self.index_of(n)) {
                    self.select_routine(index);
                }
                self.info(format!("importada(s): {}", names.join(", ")));
            }
        }
        Ok(())
    }

    fn index_of(&self, name: &str) -> Option<usize> {
        self.config.routines.iter().position(|r| r.name == name)
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
                "nome do evento (pcue emit <nome>)",
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

    /// Esvazia os passos; rotina de boot volta só com a espera do desktop.
    fn clear_steps(&mut self) -> Result<()> {
        let boot = self.routine().is_some_and(|r| r.has_event(BOOT_EVENT));
        let desktop = boot.then(Desktop::connect);
        self.mutate_routine(|r| {
            r.steps.clear();
            if boot {
                boot::ensure_prefix(&mut r.steps, desktop.as_ref());
            }
        })
    }

    /// Adiciona ou troca um gatilho; com gatilho a rotina é agendada.
    fn save_trigger(&mut self, editing: Option<usize>, trigger: Trigger) -> Result<()> {
        trigger.validate()?;
        let boot = trigger.is_event(BOOT_EVENT);
        let desktop = boot.then(Desktop::connect);
        let mut prefixed = false;
        self.mutate_routine(|r| {
            match editing {
                Some(index) => r.triggers[index] = trigger,
                None => r.triggers.push(trigger),
            }
            if boot {
                prefixed = boot::ensure_prefix(&mut r.steps, desktop.as_ref());
            }
        })?;
        self.focus = Pane::Triggers;
        if editing.is_none() {
            self.trigger_selected = self.routine().map_or(0, |r| r.triggers.len() - 1);
        }
        if !self.daemon_up {
            self.info("gatilho salvo; o daemon não está rodando (I registra no autostart)");
        } else if prefixed {
            self.info("adicionei no início os passos que esperam o desktop carregar no boot");
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
            ConfirmAction::ClearSteps => {
                let result = self.clear_steps();
                self.step_selected = 0;
                result
            }
            ConfirmAction::DeleteTrigger(index) => self.mutate_routine(|r| {
                r.triggers.remove(index);
            }),
            ConfirmAction::MakeFree => self.mutate_routine(|r| {
                r.triggers.clear();
            }),
            ConfirmAction::Discard => {
                self.config = self.saved.clone();
                self.clamp();
                self.info("alterações descartadas");
                return;
            }
            ConfirmAction::Quit => {
                self.quit = true;
                return;
            }
            ConfirmAction::SaveAndRun(selection) => {
                if let Err(err) = self.save() {
                    return self.error(format!("{err:#}"));
                }
                self.run_selected(selection);
                return;
            }
            ConfirmAction::Update => {
                if let Some(release) = self.update.clone() {
                    self.info(format!("baixando pcue {}...", release.version));
                    self.updates = Some(update::install_in_background(release));
                }
                return;
            }
        };
        self.report(result);
    }

    /// Pergunta antes de instalar a versão nova.
    fn ask_update(&mut self) {
        match &self.update {
            Some(release) => {
                let prompt = format!("Atualizar de {} para {}?", env!("CARGO_PKG_VERSION"), release.version);
                self.confirm(prompt, ConfirmAction::Update);
            }
            None => self.info("nenhuma versão nova encontrada"),
        }
    }

    /// Recebe o resultado da busca ou da instalação de versão nova.
    fn poll_update(&mut self) {
        let Some(event) = self.updates.as_ref().and_then(|rx| rx.try_recv().ok()) else { return };
        self.updates = None;
        match event {
            UpdateEvent::Available(release) => {
                self.info(format!("versão {} disponível (U atualiza)", release.version));
                self.update = Some(release);
            }
            UpdateEvent::Installed(version) => {
                self.update = None;
                self.info(format!("atualizado para {version}; reabra o pcue"));
            }
            UpdateEvent::Failed(err) => self.error(format!("atualização falhou: {err}")),
        }
    }

    fn toggle_enabled(&mut self) {
        let result = self.mutate_routine(|r| r.enabled = !r.enabled);
        self.report(result);
    }

    fn duplicate_selected(&mut self) {
        let index = self.selected;
        let mut name = String::new();
        let result = self.mutate(|c| {
            name = c.duplicate(index)?;
            Ok(())
        });
        match result {
            Ok(()) => {
                self.select_routine(index + 1);
                self.info(format!("cópia '{name}' criada (desativada)"));
            }
            Err(err) => self.error(format!("{err:#}")),
        }
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
    fn run_selected(&mut self, selection: Selection) {
        let Some(name) = self.routine().map(|r| r.name.clone()) else { return };
        if self.running.contains_key(&name) {
            return self.error(format!("'{name}' já está rodando (s para parar)"));
        }
        if self.dirty() {
            return self.confirm(
                "A execução usa o que está salvo. Salvar as alterações e rodar?".into(),
                ConfirmAction::SaveAndRun(selection),
            );
        }
        match spawn_run(&name, "manual", TUI_RUN_DELAY_MS, selection) {
            Ok(child) => {
                self.children.push(child);
                self.info(format!(
                    "'{name}' começa em {:.1}s",
                    TUI_RUN_DELAY_MS as f64 / 1000.0
                ));
            }
            Err(err) => self.error(format!("falha ao executar: {err}")),
        }
    }

    /// Depuração: roda só o passo ou a partir dele.
    fn run_step(&mut self, index: usize, selection: Selection) {
        let step = self.routine().and_then(|r| r.steps.get(index).cloned());
        if step.is_none_or(|s| is_comment(&s)) {
            return self.error("comentário não executa; escolha um passo");
        }
        self.run_selected(selection);
    }

    fn stop_selected(&mut self) {
        let Some(progress) = self.routine().and_then(|r| self.running.get(&r.name)).cloned() else {
            return self.error("a rotina não está rodando");
        };
        match progress::stop(&progress) {
            Ok(()) => {
                self.running.remove(&progress.routine);
                self.info(format!("'{}' interrompida", progress.routine));
            }
            Err(err) => self.error(format!("{err:#}")),
        }
    }

    fn open_history(&mut self, only: Option<String>) {
        let runs: Vec<Run> = self
            .runs
            .iter()
            .rev()
            .filter(|run| only.as_ref().is_none_or(|name| &run.routine == name))
            .cloned()
            .collect();
        self.open_modal(Modal::History {
            runs,
            selected: 0,
            only,
        });
    }

    /// Seleciona a rotina e o passo que falhou na execução.
    fn jump_to(&mut self, run: &Run) {
        let Some(index) = self.index_of(&run.routine) else {
            return self.error(format!("a rotina '{}' não existe mais", run.routine));
        };
        self.filter = None;
        self.select_routine(index);
        if let Some(line) = run.failed_line {
            self.focus = Pane::Steps;
            self.step_selected = line.saturating_sub(1);
            self.clamp();
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
            install::uninstall().map(|f| f.map(|p| format!("removido do autostart ({p})")))
        } else {
            install::install().map(|p| Some(format!("daemon registrado em {p}")))
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
        self.open_modal(Modal::Log { lines, scroll });
    }

    fn start_recording(&mut self) {
        let Some(routine) = self.routine().map(|r| r.name.clone()) else { return };
        self.recording = Some(Recording {
            session: recorder::start(),
            routine,
            phase: RecPhase::Countdown(crate::constants::RECORD_COUNTDOWN_SECS),
            steps: Vec::new(),
            actions: 0,
            started: None,
            last_step: Instant::now(),
        });
    }

    /// Consome os eventos da gravação em andamento.
    fn poll_recording(&mut self) {
        let Some(recording) = self.recording.as_mut() else { return };
        let outcome = loop {
            match recording.session.events.try_recv() {
                Ok(RecordEvent::Countdown(n)) => recording.phase = RecPhase::Countdown(n),
                Ok(RecordEvent::Started) => {
                    recording.phase = RecPhase::Recording;
                    recording.started = Some(Instant::now());
                }
                Ok(RecordEvent::Paused(paused)) => {
                    recording.phase = if paused { RecPhase::Paused } else { RecPhase::Recording };
                }
                Ok(RecordEvent::Step(step)) => {
                    recording.steps.push(step);
                    recording.actions += 1;
                    recording.last_step = Instant::now();
                }
                Ok(RecordEvent::Desktop(change)) => {
                    recording.steps.push(format!("{DESKTOP_MARK} {change}"));
                    recording.last_step = Instant::now();
                }
                Ok(RecordEvent::Done(draft)) => break Some(Ok(draft)),
                Ok(RecordEvent::Failed(err)) => break Some(Err(err)),
                Err(TryRecvError::Empty) => break None,
                Err(TryRecvError::Disconnected) => break Some(Err("gravação interrompida".into())),
            }
        };
        let Some(outcome) = outcome else { return };
        let routine = recording.routine.clone();
        self.recording = None;
        match outcome {
            Ok(draft) if draft.choices() > 0 => {
                self.open_modal(Modal::Review {
                    routine,
                    draft,
                    selected: 0,
                });
            }
            Ok(draft) => self.save_recording(&routine, draft.render()),
            Err(err) => self.error(format!("gravação: {err}")),
        }
    }

    /// Acrescenta os passos gravados à rotina e mostra onde ficaram.
    fn save_recording(&mut self, routine: &str, steps: Vec<String>) {
        let total = steps.len();
        let result = self.mutate(|c| {
            c.routine_or_create(routine)?.steps.extend(steps);
            Ok(())
        });
        match result.map(|()| total) {
            Ok(total) => {
                if let Some(index) = self.index_of(routine) {
                    self.select_routine(index);
                }
                self.focus = Pane::Steps;
                self.step_selected = self.routine().map_or(0, |r| r.steps.len().saturating_sub(1));
                self.info(format!("{total} passo(s) gravados em '{routine}' (S salva)"));
            }
            Err(err) => self.error(format!("gravação: {err:#}")),
        }
    }

    /// Edita todos os passos no $EDITOR; erro guarda o rascunho para tentar de novo.
    pub fn edit_in_editor(&mut self) {
        let result = self.run_editor();
        if let Err(err) = result {
            self.error(format!("editor: {err:#}"));
        }
    }

    fn run_editor(&mut self) -> Result<()> {
        let Some(routine) = self.routine() else { return Ok(()) };
        let name = routine.name.clone();
        let current = routine.steps.join("\n");
        let body = self.editor_draft.take().unwrap_or(current);
        let content = format!("{}\n{body}\n", editor_header(&name));
        let dir = runtime_dir()?;
        std::fs::create_dir_all(&dir)?;
        let file = dir.join(format!("{name}.pcue"));
        install::private_file(&file)?.write_all(content.as_bytes())?;

        let editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_else(|_| DEFAULT_EDITOR.into());
        let status = install::editor_command(&editor, &file)
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
        self.info(format!("{total} passo(s) aplicados (S salva)"));
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
        KeyCode::PageUp => Some(index.saturating_sub(10)),
        KeyCode::PageDown => Some((index + 10).min(len.saturating_sub(1))),
        KeyCode::Home => Some(0),
        KeyCode::End => Some(len.saturating_sub(1)),
        _ => None,
    }
}

const DAILY_HINT: &str = "HH:MM e dias opcionais, ex.: 09:00 seg,ter,qua,qui,sex";
const INTERVAL_HINT: &str = "ex.: 30m, 2h ou 1.5h (mínimo 1m)";
