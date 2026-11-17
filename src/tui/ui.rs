use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};

use super::app::{App, Modal, Pane};
use crate::config::Routine;
use crate::schedule::describe_next;
use crate::script::{SYNTAX, is_comment};

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const OK: Color = Color::Green;
const BAD: Color = Color::Red;

/// Desenha a tela inteira.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .areas(frame.area());
    draw_header(frame, header, app);

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(32), Constraint::Min(0)]).areas(body);
    draw_routines(frame, left, app);
    draw_details(frame, right, app);
    draw_footer(frame, footer, app);

    if let Some(modal) = &app.modal {
        draw_modal(frame, modal);
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let chip = |on: bool, yes: &str, no: &str| {
        if on {
            Span::styled(format!(" ● {yes} "), Style::new().fg(OK))
        } else {
            Span::styled(format!(" ○ {no} "), Style::new().fg(MUTED))
        }
    };
    let line = Line::from(vec![
        Span::styled(" pcue ", Style::new().fg(Color::Black).bg(ACCENT).bold()),
        Span::raw(" "),
        chip(app.daemon_up, "daemon ativo", "daemon parado"),
        chip(app.installed, "no autostart", "fora do autostart"),
        chip(app.hyprland, "Hyprland", "sem Hyprland"),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn pane_block(title: &str, focused: bool) -> Block<'_> {
    let color = if focused { ACCENT } else { MUTED };
    Block::bordered()
        .title(Span::styled(format!(" {title} "), Style::new().fg(color).bold()))
        .border_style(Style::new().fg(color))
}

fn draw_routines(frame: &mut Frame, area: Rect, app: &App) {
    let block = pane_block("Rotinas", app.focus == Pane::Routines);
    if app.config.routines.is_empty() {
        let text = Paragraph::new("Nenhuma rotina.\n\nn cria a primeira.")
            .fg(MUTED)
            .block(block)
            .wrap(Wrap { trim: true });
        frame.render_widget(text, area);
        return;
    }
    let items: Vec<ListItem> = app
        .config
        .routines
        .iter()
        .map(|routine| routine_item(routine, app))
        .collect();
    render_list(frame, area, block, items, app.selected, true);
}

fn routine_item<'a>(routine: &'a Routine, app: &App) -> ListItem<'a> {
    let dim = !routine.enabled;
    let kind = routine.kind_label();
    let kind_color = if routine.is_scheduled() { Color::Yellow } else { Color::Magenta };
    let last = match app.history.get(&routine.name) {
        Some(run) if run.ok => Span::styled(" ✓", Style::new().fg(OK)),
        Some(_) => Span::styled(" ✗", Style::new().fg(BAD)),
        None => Span::raw(""),
    };
    let name_style = if dim { Style::new().fg(MUTED) } else { Style::new() };
    let mut spans = vec![
        Span::styled(routine.name.as_str(), name_style),
        Span::raw(" "),
        Span::styled(kind, Style::new().fg(if dim { MUTED } else { kind_color })),
        last,
    ];
    if dim {
        spans.push(Span::styled(" (desativada)", Style::new().fg(MUTED).italic()));
    }
    ListItem::new(Line::from(spans))
}

fn draw_details(frame: &mut Frame, area: Rect, app: &App) {
    let Some(routine) = app.routine() else {
        let intro = Paragraph::new(vec![
            Line::from("Rotinas são sequências de passos: abrir apps em workspaces,"),
            Line::from("esperar janelas, atalhos, texto e cliques."),
            Line::from(""),
            Line::from("Livres rodam quando você chama (Enter aqui, ou `pcue run nome`)."),
            Line::from("Agendadas rodam sozinhas por evento (login, desbloqueio...) ou horário."),
        ])
        .fg(MUTED)
        .block(pane_block("Detalhes", false))
        .wrap(Wrap { trim: true });
        frame.render_widget(intro, area);
        return;
    };

    let trigger_rows = routine.triggers.len() as u16 + 2;
    let [summary, triggers, steps] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(trigger_rows.clamp(3, 10)),
        Constraint::Min(0),
    ])
    .areas(area);
    draw_summary(frame, summary, app, routine);
    draw_triggers(frame, triggers, app, routine);
    draw_steps(frame, steps, app, routine);
}

fn draw_summary(frame: &mut Frame, area: Rect, app: &App, routine: &Routine) {
    let kind = routine.kind_label();
    let state = if routine.enabled { "ativa" } else { "desativada" };
    let steps = routine.steps.iter().filter(|s| !is_comment(s)).count();
    let mut lines = vec![Line::from(vec![
        Span::styled(routine.name.as_str(), Style::new().bold()),
        Span::styled(format!("  {kind} · {state} · {steps} passo(s)"), Style::new().fg(MUTED)),
    ])];
    lines.push(match app.history.get(&routine.name) {
        None => Line::styled("nunca executada", Style::new().fg(MUTED)),
        Some(run) => {
            let seconds = run.duration_ms as f64 / 1000.0;
            let head = format!("última: {} ({}, {seconds:.1}s) ", run.started, run.trigger);
            match &run.error {
                None => Line::from(vec![Span::raw(head), Span::styled("ok", Style::new().fg(OK))]),
                Some(err) => Line::from(vec![Span::raw(head), Span::styled(err.as_str(), Style::new().fg(BAD))]),
            }
        }
    });
    let paragraph = Paragraph::new(lines)
        .block(pane_block("Detalhes", false))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

fn draw_triggers(frame: &mut Frame, area: Rect, app: &App, routine: &Routine) {
    let block = pane_block("Gatilhos", app.focus == Pane::Triggers);
    if !routine.is_scheduled() {
        let text = Paragraph::new("Livre: roda quando chamada. t ou a (aqui) adiciona um gatilho.")
            .fg(MUTED)
            .block(block);
        frame.render_widget(text, area);
        return;
    }
    let now = jiff::Zoned::now();
    let items: Vec<ListItem> = routine
        .triggers
        .iter()
        .map(|trigger| {
            let mut spans = vec![Span::raw(trigger.to_string())];
            if let Some(next) = describe_next(trigger, &now) {
                spans.push(Span::styled(format!("  → {next}"), Style::new().fg(MUTED)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    render_list(frame, area, block, items, app.trigger_selected, app.focus == Pane::Triggers);
}

fn draw_steps(frame: &mut Frame, area: Rect, app: &App, routine: &Routine) {
    let block = pane_block("Passos", app.focus == Pane::Steps);
    if routine.steps.is_empty() {
        let text = Paragraph::new("Sem passos. a adiciona, R grava teclado/mouse, c captura as janelas abertas.")
            .fg(MUTED)
            .block(block)
            .wrap(Wrap { trim: true });
        frame.render_widget(text, area);
        return;
    }
    let width = routine.steps.len().to_string().len();
    let items: Vec<ListItem> = routine
        .steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let mut spans = vec![Span::styled(format!("{:>width$} ", i + 1), Style::new().fg(MUTED))];
            spans.extend(highlight_step(step));
            ListItem::new(Line::from(spans))
        })
        .collect();
    render_list(frame, area, block, items, app.step_selected, app.focus == Pane::Steps);
}

fn render_list(frame: &mut Frame, area: Rect, block: Block, items: Vec<ListItem>, selected: usize, focused: bool) {
    let highlight = if focused {
        Style::new().bg(Color::Rgb(40, 44, 52)).add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    let list = List::new(items)
        .block(block)
        .highlight_style(highlight)
        .highlight_symbol(if focused { "▶ " } else { "  " });
    let mut state = ListState::default().with_selected(focused.then_some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// Verbo em destaque, comentários apagados.
fn highlight_step(step: &str) -> Vec<Span<'_>> {
    if is_comment(step) {
        return vec![Span::styled(step, Style::new().fg(MUTED).italic())];
    }
    let trimmed = step.trim_start();
    let (verb, rest) = trimmed.split_at(trimmed.find(' ').unwrap_or(trimmed.len()));
    let color = match verb {
        "launch" => Color::Green,
        "wait_window" | "sleep" => Color::Yellow,
        "key" | "type" => Color::Magenta,
        "click" | "scroll" => Color::Blue,
        _ => BAD,
    };
    vec![Span::styled(verb, Style::new().fg(color).bold()), Span::raw(rest)]
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let status = match app.status() {
        Some(status) if status.error => Line::styled(status.text.clone(), Style::new().fg(BAD)),
        Some(status) => Line::styled(status.text.clone(), Style::new().fg(OK)),
        None => Line::raw(""),
    };
    let hints = match app.focus {
        Pane::Routines => "Enter rodar · n nova · r renomear · d apagar · espaço ativar · t livre/agendada · Tab painel · L log · I autostart · ? ajuda · q sair",
        Pane::Triggers => "a adicionar · Enter editar · d apagar · Tab painel · Esc voltar",
        Pane::Steps => "a adicionar · Enter editar · d apagar · J/K mover · R gravar · c capturar janelas · E editor · Esc voltar",
    };
    let text = Paragraph::new(vec![status, Line::styled(hints, Style::new().fg(MUTED))]);
    frame.render_widget(text, area);
}

fn draw_modal(frame: &mut Frame, modal: &Modal) {
    match modal {
        Modal::Input {
            title,
            hint,
            input,
            error,
            ..
        } => {
            let area = centered(frame.area(), 80, 9);
            frame.render_widget(Clear, area);
            let block = pane_block(title, true);
            let inner = block.inner(area);
            frame.render_widget(block, area);
            let [hint_area, input_area, error_area] = Layout::vertical([
                Constraint::Length(4),
                Constraint::Length(1),
                Constraint::Length(2),
            ])
            .areas(inner);
            frame.render_widget(
                Paragraph::new(hint.as_str()).fg(MUTED).wrap(Wrap { trim: true }),
                hint_area,
            );
            let prefix = "› ";
            let visible = input_area.width.saturating_sub(prefix.len() as u16 + 1) as usize;
            let skip = input.cursor().saturating_sub(visible);
            let shown: String = input.value().chars().skip(skip).collect();
            frame.render_widget(Paragraph::new(format!("{prefix}{shown}")), input_area);
            let cursor_x = input_area.x + 2 + (input.cursor() - skip) as u16;
            frame.set_cursor_position(Position::new(cursor_x, input_area.y));
            if let Some(err) = error {
                frame.render_widget(
                    Paragraph::new(err.as_str()).fg(BAD).wrap(Wrap { trim: true }),
                    error_area,
                );
            }
        }
        Modal::Confirm { prompt, .. } => {
            let area = centered(frame.area(), 60, 5);
            frame.render_widget(Clear, area);
            let text = Paragraph::new(vec![
                Line::raw(prompt.as_str()),
                Line::raw(""),
                Line::styled("s confirma · n ou Esc cancela", Style::new().fg(MUTED)),
            ])
            .block(pane_block("Confirmar", true))
            .wrap(Wrap { trim: true });
            frame.render_widget(text, area);
        }
        Modal::Pick {
            title,
            options,
            selected,
            ..
        } => {
            let area = centered(frame.area(), 70, options.len() as u16 + 2);
            frame.render_widget(Clear, area);
            let items: Vec<ListItem> = options
                .iter()
                .map(|(label, desc)| {
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{label:<16}"), Style::new().bold()),
                        Span::styled(desc.as_str(), Style::new().fg(MUTED)),
                    ]))
                })
                .collect();
            render_list(frame, area, pane_block(title, true), items, *selected, true);
        }
        Modal::Help => {
            let lines: Vec<Line> = HELP_ROUTINES
                .iter()
                .map(|l| Line::raw(*l))
                .chain(SYNTAX.iter().map(|l| Line::raw(format!("  {l}"))))
                .chain(HELP_GENERAL.iter().map(|l| Line::raw(*l)))
                .collect();
            let area = centered(frame.area(), 96, lines.len() as u16 + 2);
            frame.render_widget(Clear, area);
            frame.render_widget(Paragraph::new(lines).block(pane_block("Ajuda", true)), area);
        }
        Modal::Log { lines, scroll } => {
            let full = frame.area();
            let area = centered(full, full.width.saturating_sub(4), full.height.saturating_sub(2));
            frame.render_widget(Clear, area);
            let height = area.height.saturating_sub(2) as usize;
            let top = (*scroll).min(lines.len().saturating_sub(height));
            let shown: Vec<Line> = lines.iter().skip(top).take(height).map(|l| Line::raw(l.as_str())).collect();
            let title = "Log · j/k PgUp/PgDn g/G · Esc fecha";
            frame.render_widget(Paragraph::new(shown).block(pane_block(title, true)), area);
        }
    }
}

/// Retângulo centralizado com tamanho máximo.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

const HELP_ROUTINES: &[&str] = &[
    "Rotinas",
    "  livre     roda quando chamada: Enter na TUI, `pcue run nome` ou um atalho do Hyprland",
    "  agendada  roda pelo daemon: eventos (login, lock, unlock, resume, monitor_*),",
    "            horário diário ou intervalo; eventos próprios via `pcue emit nome`",
    "",
    "Passos",
];

const HELP_GENERAL: &[&str] = &[
    "",
    "Gravação (R no painel de passos)",
    "  SUPER+F9 pausa/retoma (use em senhas) · SUPER+F10 encerra",
    "  cliques viram posição relativa à janela, com wait_window antes",
    "",
    "Geral",
    "  Tab/←/→ troca painel · L log · I liga/desliga autostart do daemon · q sai",
];
