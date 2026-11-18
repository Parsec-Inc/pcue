//! Interpreta uma gravação: junta as ações cruas com o que mudou no desktop
//! (janela aberta, foco, workspace) e propõe alternativas para cada trecho.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::config::Workspace;
use crate::constants::{
    CAUSE_GAP, DEFAULT_WAIT_TIMEOUT, FOCUS_ATTRIBUTION, FOCUS_FOLLOW, RECORD_MIN_SLEEP, RECORD_SLEEP_ROUNDING_MS,
};
use crate::script::Step;

/// Tipo da ação crua, para decidir o que pode causar uma mudança.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Combo,
    Text,
    Click,
    Scroll,
}

/// Ação gravada, já como passo.
#[derive(Debug, Clone)]
pub struct Action {
    pub start: Instant,
    pub end: Instant,
    pub kind: ActionKind,
    pub step: Step,
}

/// Janela vista na gravação, já no formato dos passos.
#[derive(Debug, Clone)]
pub struct Seen {
    pub id: String,
    /// Class como regex literal.
    pub class: String,
    /// Posição entre as janelas da class (1 = a mais antiga).
    pub nth: usize,
    pub workspace: String,
}

/// O que mudou no desktop.
#[derive(Debug, Clone)]
pub enum Change {
    Opened {
        window: Seen,
        /// Comando que abre o app, se descoberto.
        command: Option<String>,
        /// Já havia janela dessa class antes.
        others: bool,
        /// A janela é de um processo novo (e não de um app já aberto).
        new_process: bool,
    },
    Focused {
        window: Option<Seen>,
        workspace: String,
        workspace_changed: bool,
    },
}

/// Mudança no desktop e quando foi percebida.
#[derive(Debug, Clone)]
pub struct DesktopEvent {
    pub at: Instant,
    pub change: Change,
}

/// Uma forma de escrever o trecho.
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub label: &'static str,
    pub steps: Vec<String>,
}

/// Trecho da gravação com as alternativas e a pausa antes dele.
#[derive(Debug, Clone)]
pub struct Item {
    pub variants: Vec<Variant>,
    pub chosen: usize,
    pub pause: Option<Duration>,
    pub keep_pause: bool,
}

impl Item {
    fn recorded(step: String) -> Self {
        Self::new(vec![Variant { label: "gravado", steps: vec![step] }])
    }

    fn new(variants: Vec<Variant>) -> Self {
        Self {
            variants,
            chosen: 0,
            pause: None,
            keep_pause: true,
        }
    }

    /// Variante escolhida.
    pub fn current(&self) -> &Variant {
        &self.variants[self.chosen]
    }

    /// Passa para a próxima alternativa.
    pub fn cycle(&mut self, forward: bool) {
        let count = self.variants.len();
        self.chosen = if forward { (self.chosen + 1) % count } else { (self.chosen + count - 1) % count };
    }
}

/// Gravação interpretada, pronta para revisar.
#[derive(Debug, Clone, Default)]
pub struct Draft {
    pub items: Vec<Item>,
}

impl Draft {
    /// Passos finais com as escolhas atuais.
    pub fn render(&self) -> Vec<String> {
        let mut steps = Vec::new();
        for item in &self.items {
            if let Some(pause) = item.pause.filter(|_| item.keep_pause) {
                steps.push(Step::Sleep(pause).to_string());
            }
            steps.extend(item.current().steps.iter().cloned());
        }
        steps
    }

    /// Quantos trechos têm mais de uma forma.
    pub fn choices(&self) -> usize {
        self.items.iter().filter(|i| i.variants.len() > 1).count()
    }

    /// Troca o trecho por um trecho por passo da forma escolhida; a pausa fica no primeiro.
    pub fn split(&mut self, index: usize) {
        let Some(item) = self.items.get(index).filter(|i| i.current().steps.len() > 1) else { return };
        let mut pieces: Vec<Item> = item.current().steps.iter().map(|step| Item::recorded(step.clone())).collect();
        pieces[0].pause = item.pause;
        pieces[0].keep_pause = item.keep_pause;
        self.items.splice(index..=index, pieces);
    }

    /// Substitui o trecho por um único passo, mantendo a pausa antes dele.
    pub fn edit(&mut self, index: usize, step: String) {
        if let Some(item) = self.items.get_mut(index) {
            item.variants = vec![Variant { label: "editado", steps: vec![step] }];
            item.chosen = 0;
        }
    }
}

/// Junta as ações às mudanças do desktop que elas causaram.
pub fn build(actions: &[Action], events: &[DesktopEvent]) -> Draft {
    let mut builder = Builder::default();
    let mut next = 0;
    let mut absorbed: HashSet<String> = HashSet::new();
    for (index, event) in events.iter().enumerate() {
        let upto = actions[next..]
            .iter()
            .position(|a| a.start > event.at)
            .map_or(actions.len(), |p| next + p);
        let piece = match &event.change {
            Change::Opened {
                window,
                command,
                others,
                new_process,
            } => {
                let follows = events[index + 1..]
                    .iter()
                    .take_while(|e| e.at.saturating_duration_since(event.at) <= FOCUS_FOLLOW)
                    .any(|e| focused_on(e, &window.id));
                if follows {
                    absorbed.insert(window.id.clone());
                }
                let start = opened_cause(actions, next, upto);
                let launch = command.as_deref().filter(|_| *new_process);
                Some((start, opened_variants(window, launch, *others, &actions[start..upto], follows)))
            }
            Change::Focused {
                window,
                workspace,
                workspace_changed,
            } => {
                if window.as_ref().is_some_and(|w| absorbed.remove(&w.id)) {
                    None
                } else {
                    let start = focus_cause(actions, next, upto);
                    let cause = &actions[start..upto];
                    let caused = cause
                        .last()
                        .is_some_and(|a| event.at.saturating_duration_since(a.end) <= FOCUS_ATTRIBUTION);
                    let workspace = workspace_changed.then_some(workspace.as_str());
                    let variants = focus_variants(window.as_ref(), workspace, cause);
                    (caused && !variants.is_empty()).then_some((start, variants))
                }
            }
        };
        let Some((start, variants)) = piece else { continue };
        builder.plain(&actions[next..start]);
        let begin = actions.get(start).filter(|_| start < upto).map_or(event.at, |a| a.start);
        builder.push(begin, event.at, Item::new(variants));
        next = upto;
    }
    builder.plain(&actions[next..]);
    builder.draft
}

/// O evento é o foco indo para a janela?
fn focused_on(event: &DesktopEvent, id: &str) -> bool {
    matches!(&event.change, Change::Focused { window: Some(w), .. } if w.id == id)
}

/// Início do que abriu a janela: o último atalho ou clique, ou um lançador (atalho, texto e Enter).
fn opened_cause(actions: &[Action], next: usize, upto: usize) -> usize {
    let linked = |i: usize| i == upto || close(&actions[i - 1], &actions[i]);
    let back_while = |mut i: usize, kind: ActionKind| {
        while i > next && actions[i - 1].kind == kind && linked(i) {
            i -= 1;
        }
        i
    };
    let combos = back_while(upto, ActionKind::Combo);
    let text = back_while(combos, ActionKind::Text);
    if text < combos {
        let opener = text > next && linked(text) && is_trigger(&actions[text - 1]);
        return if opener { text - 1 } else { combos };
    }
    match upto.checked_sub(1) {
        Some(last) if last >= next && is_trigger(&actions[last]) => last,
        _ => upto,
    }
}

/// Só o último atalho troca o foco (Alt+Tab, SUPER+N...); os anteriores ficam como gravados.
fn focus_cause(actions: &[Action], next: usize, upto: usize) -> usize {
    match upto.checked_sub(1) {
        Some(last) if last >= next && actions[last].kind == ActionKind::Combo => last,
        _ => upto,
    }
}

fn is_trigger(action: &Action) -> bool {
    matches!(action.kind, ActionKind::Combo | ActionKind::Click)
}

fn close(before: &Action, after: &Action) -> bool {
    after.start.saturating_duration_since(before.end) <= CAUSE_GAP
}

/// Formas para uma janela aberta; `launch` só vem quando é um app novo com comando conhecido.
fn opened_variants(window: &Seen, launch: Option<&str>, others: bool, cause: &[Action], focus: bool) -> Vec<Variant> {
    let wait = Step::WaitWindow {
        class: window.class.clone(),
        nth: window.nth,
        timeout: DEFAULT_WAIT_TIMEOUT,
    }
    .to_string();
    if cause.is_empty() {
        return vec![Variant {
            label: "esperar a janela",
            steps: vec![wait],
        }];
    }
    let mut variants = Vec::new();
    if let Some(command) = launch {
        let launch = Step::Launch {
            command: command.to_string(),
            workspace: Some(Workspace(window.workspace.clone())),
            class: Some(window.class.clone()),
            focus,
            always: others,
        };
        variants.push(Variant {
            label: "abrir o app",
            steps: vec![launch.to_string()],
        });
    }
    let mut raw = raw_steps(cause);
    raw.push(wait);
    variants.push(Variant {
        label: "teclas originais + esperar a janela",
        steps: raw,
    });
    variants
}

/// Formas para uma troca de foco; `workspace` só quando o workspace mudou.
fn focus_variants(window: Option<&Seen>, workspace: Option<&str>, cause: &[Action]) -> Vec<Variant> {
    let mut variants = Vec::new();
    if let Some(window) = window {
        let focus = Step::Focus {
            class: window.class.clone(),
            nth: window.nth,
            timeout: DEFAULT_WAIT_TIMEOUT,
        };
        variants.push(Variant {
            label: "focar o app",
            steps: vec![focus.to_string()],
        });
    }
    if let Some(workspace) = workspace {
        variants.push(Variant {
            label: "ir para o workspace",
            steps: vec![Step::Workspace(Workspace(workspace.to_string())).to_string()],
        });
    }
    if !variants.is_empty() {
        variants.push(Variant {
            label: "teclas originais",
            steps: raw_steps(cause),
        });
    }
    variants
}

/// Ações como passos, com as pausas entre elas.
fn raw_steps(actions: &[Action]) -> Vec<String> {
    let mut steps = Vec::new();
    for (index, action) in actions.iter().enumerate() {
        if let Some(pause) = index.checked_sub(1).and_then(|i| pause_between(actions[i].end, action.start)) {
            steps.push(Step::Sleep(pause).to_string());
        }
        steps.push(action.step.to_string());
    }
    steps
}

/// Pausa perceptível, arredondada para cima.
fn pause_between(before: Instant, after: Instant) -> Option<Duration> {
    let gap = after.saturating_duration_since(before);
    (gap >= RECORD_MIN_SLEEP).then(|| round_recorded(gap))
}

/// Arredonda um tempo gravado para cima, no passo das pausas gravadas.
pub fn round_recorded(duration: Duration) -> Duration {
    let rounded = duration.as_millis().div_ceil(RECORD_SLEEP_ROUNDING_MS) * RECORD_SLEEP_ROUNDING_MS;
    Duration::from_millis(rounded as u64)
}

/// Monta os trechos guardando o fim do anterior para calcular a pausa.
#[derive(Default)]
struct Builder {
    draft: Draft,
    last_end: Option<Instant>,
}

impl Builder {
    fn push(&mut self, start: Instant, end: Instant, mut item: Item) {
        item.pause = self.last_end.and_then(|last| pause_between(last, start));
        self.draft.items.push(item);
        self.last_end = Some(end);
    }

    /// Ações sem mudança no desktop: um trecho por ação, sem alternativas.
    fn plain(&mut self, actions: &[Action]) {
        for action in actions {
            self.push(action.start, action.end, Item::recorded(action.step.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::parse_combo;

    struct Clock(Instant);

    impl Clock {
        fn at(&self, ms: u64) -> Instant {
            self.0 + Duration::from_millis(ms)
        }

        fn combo(&self, ms: u64, combo: &str) -> Action {
            Action {
                start: self.at(ms),
                end: self.at(ms),
                kind: ActionKind::Combo,
                step: Step::Key(parse_combo(combo).unwrap()),
            }
        }

        fn text(&self, ms: u64, text: &str) -> Action {
            Action {
                start: self.at(ms),
                end: self.at(ms + 200),
                kind: ActionKind::Text,
                step: Step::Type(text.into()),
            }
        }
    }

    fn seen(id: &str, class: &str, workspace: &str) -> Seen {
        Seen {
            id: id.into(),
            class: class.into(),
            nth: 1,
            workspace: workspace.into(),
        }
    }

    fn focused(at: Instant, window: Option<Seen>, workspace: &str, changed: bool) -> DesktopEvent {
        DesktopEvent {
            at,
            change: Change::Focused {
                window,
                workspace: workspace.into(),
                workspace_changed: changed,
            },
        }
    }

    #[test]
    fn launcher_typing_becomes_launch() {
        let c = Clock(Instant::now());
        let actions = vec![
            c.text(0, "notas da reunião"),
            c.combo(3000, "SUPER"),
            c.text(3400, "zen"),
            c.combo(3800, "RETURN"),
            c.text(9000, "oi"),
        ];
        let events = vec![
            DesktopEvent {
                at: c.at(6000),
                change: Change::Opened {
                    window: seen("a", "zen", "6"),
                    command: Some("zen-browser".into()),
                    others: false,
                    new_process: true,
                },
            },
            focused(c.at(6100), Some(seen("a", "zen", "6")), "6", true),
        ];
        let draft = build(&actions, &events);
        let steps = draft.render();
        assert_eq!(
            steps,
            vec![
                "type \"notas da reunião\"",
                "sleep 2800ms",
                "launch zen-browser workspace=6 class=zen focus",
                "sleep 3s",
                "type oi",
            ]
        );
        assert_eq!(draft.choices(), 1);
        let launch = &draft.items[1];
        assert_eq!(launch.variants[1].steps.last().unwrap(), "wait_window class=zen");
        assert!(launch.variants[1].steps.contains(&"type zen".to_string()));
    }

    #[test]
    fn shortcut_focus_prefers_app_over_workspace() {
        let c = Clock(Instant::now());
        let actions = vec![c.combo(0, "SUPER+3"), c.text(800, "ls")];
        let events = vec![focused(c.at(150), Some(seen("k", "kitty", "3")), "3", true)];
        let mut draft = build(&actions, &events);
        assert_eq!(draft.render()[0], "focus class=kitty");
        draft.items[0].cycle(true);
        assert_eq!(draft.render()[0], "workspace 3");
        draft.items[0].cycle(true);
        assert_eq!(draft.render()[0], "key SUPER+3");
    }

    #[test]
    fn empty_workspace_and_click_focus() {
        let c = Clock(Instant::now());
        let click = Action {
            start: c.at(2000),
            end: c.at(2000),
            kind: ActionKind::Click,
            step: Step::Scroll(1),
        };
        let actions = vec![c.combo(0, "SUPER+5"), click];
        let events = vec![
            focused(c.at(100), None, "5", true),
            focused(c.at(2100), Some(seen("x", "code", "5")), "5", false),
        ];
        let draft = build(&actions, &events);
        assert_eq!(draft.render(), vec!["workspace 5", "sleep 1900ms", "scroll 1"]);
    }

    #[test]
    fn quick_actions_before_a_change_stay_recorded() {
        let c = Clock(Instant::now());
        let actions = vec![c.combo(0, "CTRL+S"), c.combo(300, "CTRL+C"), c.combo(600, "SUPER+3")];
        let events = vec![focused(c.at(700), Some(seen("k", "kitty", "3")), "3", true)];
        let draft = build(&actions, &events);
        assert_eq!(draft.render(), vec!["key CTRL+S", "sleep 300ms", "key CTRL+C", "sleep 300ms", "focus class=kitty"]);

        let actions = vec![c.combo(0, "CTRL+S"), c.combo(300, "SUPER"), c.text(500, "zen"), c.combo(900, "RETURN")];
        let events = vec![DesktopEvent {
            at: c.at(1500),
            change: Change::Opened {
                window: seen("z", "zen", "6"),
                command: Some("zen-browser".into()),
                others: false,
                new_process: true,
            },
        }];
        let draft = build(&actions, &events);
        assert_eq!(draft.render(), vec!["key CTRL+S", "sleep 300ms", "launch zen-browser workspace=6 class=zen"]);
    }

    #[test]
    fn split_and_edit_keep_the_pause() {
        let c = Clock(Instant::now());
        let actions = vec![c.combo(0, "CTRL+S"), c.combo(300, "SUPER+3")];
        let events = vec![focused(c.at(400), Some(seen("k", "kitty", "3")), "3", true)];
        let mut draft = build(&actions, &events);
        let focus = &mut draft.items[1];
        focus.cycle(false);
        assert_eq!(focus.current().steps, vec!["key SUPER+3"]);
        draft.items.push(Item::recorded("type oi".into()));
        draft.items[2].variants[0].steps.push("key RETURN".into());
        draft.items[2].pause = Some(Duration::from_secs(2));
        draft.split(2);
        assert_eq!(draft.items.len(), 4);
        draft.edit(2, "type olá".into());
        assert_eq!(draft.render()[3..], ["sleep 2s", "type olá", "key RETURN"]);
    }

    #[test]
    fn window_of_running_app_keeps_keys() {
        let c = Clock(Instant::now());
        let actions = vec![c.combo(0, "CTRL+SHIFT+N")];
        let mut window = seen("b2", "beekeeper", "4");
        window.nth = 2;
        let events = vec![DesktopEvent {
            at: c.at(900),
            change: Change::Opened {
                window,
                command: Some("beekeeper-studio".into()),
                others: true,
                new_process: false,
            },
        }];
        let draft = build(&actions, &events);
        assert_eq!(draft.render(), vec!["key CTRL+SHIFT+N", "wait_window class=beekeeper nth=2"]);
    }
}
