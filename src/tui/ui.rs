use std::time::Duration;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use super::anim::{self, FLASH, POP, Slide, countdown, mix, spinner, wave};
use super::app::{App, CELEBRATE_FOR, Hit, Modal, Pane, RecPhase, Recording};
use super::sprites::{self, Sprite};
use crate::config::{Routine, Trigger};
use crate::schedule::describe_next;
use crate::script::{SYNTAX, is_comment};

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const OK: Color = Color::Green;
const BAD: Color = Color::Red;
const WARN: Color = Color::Yellow;
const SELECTED_BG: Color = Color::Rgb(40, 46, 60);
const RUNNING_BG: (u8, u8, u8) = (60, 52, 20);
const RUNNING_BG_DIM: (u8, u8, u8) = (34, 30, 14);
const GREEN_LOW: (u8, u8, u8) = (40, 120, 70);
const GREEN_HIGH: (u8, u8, u8) = (110, 235, 150);
const ACCENT_RGB: (u8, u8, u8) = (95, 215, 255);
const FLASH_RGB: (u8, u8, u8) = (120, 240, 150);
/// Período do pulso do indicador do daemon.
const PULSE: Duration = Duration::from_millis(1800);
/// Linhas da caixa com o mascote.
const BRAND_HEIGHT: u16 = 10;

/// Desenha a tela inteira e registra as regiões clicáveis.
pub fn draw(frame: &mut Frame, app: &mut App) {
    app.hits.clear();
    if app.recording.is_some() {
        draw_recording(frame, app);
        return;
    }
    let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(3)]).areas(frame.area());
    let [left, right] = Layout::horizontal([Constraint::Percentage(32), Constraint::Min(0)]).areas(body);
    let [brand, routines] =
        Layout::vertical([Constraint::Length(BRAND_HEIGHT), Constraint::Min(0)]).areas(left);
    draw_brand(frame, brand, app);
    draw_routines(frame, routines, app);
    draw_details(frame, right, app);
    draw_footer(frame, footer, app);

    if app.modal.is_some() {
        app.hits.clear();
        app.hits.push((frame.area(), Hit::Backdrop));
        let area = frame.area();
        frame.buffer_mut().set_style(area, Style::new().add_modifier(Modifier::DIM));
        draw_modal(frame, app);
    }
}

fn millis(app: &App) -> u64 {
    app.clock.elapsed().as_millis() as u64
}

fn pane_block(title: &str, focused: bool) -> Block<'_> {
    let color = if focused { ACCENT } else { MUTED };
    Block::bordered()
        .border_type(ratatui::widgets::BorderType::Rounded)
        .title(Span::styled(format!(" {title} "), Style::new().fg(color).bold()))
        .border_style(Style::new().fg(color))
}

/// Borda que brilha em verde logo depois de salvar.
fn focus_block<'a>(app: &App, title: &'a str, focused: bool) -> Block<'a> {
    let block = pane_block(title, focused);
    match app.flash.filter(|t| focused && t.elapsed() < FLASH) {
        Some(at) => block.border_style(Style::new().fg(mix(FLASH_RGB, ACCENT_RGB, anim::ease_out(at, FLASH)))),
        None => block,
    }
}

fn draw_brand(frame: &mut Frame, area: Rect, app: &mut App) {
    let block = pane_block("pcue", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let t = millis(app);
    let mascot = match &app.finished {
        Some(f) if f.ok && f.at.elapsed() < CELEBRATE_FOR => Sprite::MascotCheer,
        _ if app.config.routines.is_empty() => Sprite::MascotSleep,
        _ => Sprite::Mascot,
    };
    let (w, _) = sprites::size(mascot);
    let [art, text] = Layout::horizontal([Constraint::Length(w + 2), Constraint::Min(0)]).areas(inner);
    sprites::draw(frame.buffer_mut(), Rect { x: art.x + 1, ..art }, mascot, t);

    let pulse = wave(app.clock, PULSE);
    let chip = |on: bool, yes: &'static str, no: &'static str, color: Color| {
        if on {
            Line::from(vec![Span::styled("● ", Style::new().fg(color)), Span::raw(yes)])
        } else {
            Line::from(vec![Span::styled("○ ", Style::new().fg(MUTED)), Span::styled(no, Style::new().fg(MUTED))])
        }
    };
    let running = app.running.len();
    let mut lines = vec![
        Line::from(vec![
            Span::styled("pcue", Style::new().fg(ACCENT).bold()),
            Span::styled("  rotinas dos parças", Style::new().fg(MUTED)),
        ]),
        Line::raw(""),
        chip(app.daemon_up, "daemon ativo", "daemon parado", mix(GREEN_LOW, GREEN_HIGH, pulse)),
        chip(app.installed, "no autostart", "fora do autostart", OK),
        chip(app.hyprland, "Hyprland", "sem Hyprland", OK),
    ];
    if running > 0 {
        lines.push(Line::styled(
            format!("{} {running} rodando", spinner(app.clock)),
            Style::new().fg(WARN),
        ));
    } else if let Some(at) = app.flash.filter(|t| t.elapsed() < FLASH) {
        lines.push(Line::styled("✓ salvo", Style::new().fg(mix(FLASH_RGB, (60, 60, 60), anim::ease_out(at, FLASH)))));
    }
    frame.render_widget(Paragraph::new(lines), text);
    let autostart = Rect::new(text.x, text.y + 3, text.width, 1);
    app.hits.push((autostart, Hit::Key(KeyCode::Char('I'))));
}

fn draw_routines(frame: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Pane::Routines;
    let filter = app.filter.as_ref().map(|f| (f.input.value().to_string(), f.input.cursor(), f.editing));
    let title = match &filter {
        Some((text, _, _)) if !text.is_empty() => format!("Rotinas · /{text}"),
        _ => "Rotinas".to_string(),
    };
    let block = focus_block(app, &title, focused);
    let mut inner = block.inner(area);
    frame.render_widget(block, area);
    app.hits.push((area, Hit::Pane(Pane::Routines)));

    if let Some((text, cursor, editing)) = filter {
        let [search, rest] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
        let style = if editing { Style::new().fg(ACCENT) } else { Style::new().fg(MUTED) };
        frame.render_widget(Paragraph::new(format!("/ {text}")).style(style), search);
        if editing {
            frame.set_cursor_position(Position::new(search.x + 2 + cursor as u16, search.y));
        }
        inner = rest;
    }

    if app.config.routines.is_empty() {
        let text = Paragraph::new("Nenhuma rotina ainda.\n\nn cria a primeira, i importa um arquivo.")
            .fg(MUTED)
            .wrap(Wrap { trim: true });
        frame.render_widget(text, inner);
        return;
    }
    let visible = app.visible();
    if visible.is_empty() {
        frame.render_widget(Paragraph::new("nada encontrado").fg(MUTED), inner);
        return;
    }
    let now = jiff::Timestamp::now().as_second();
    let rows: Vec<Line> = visible
        .iter()
        .map(|i| routine_row(&app.config.routines[*i], app, now))
        .collect();
    let position = visible.iter().position(|i| *i == app.selected).unwrap_or(0);
    app.slides.routines.follow(position);
    let slide = app.slides.routines;
    let first = draw_list(frame.buffer_mut(), inner, rows, position, focused, slide, app.scroll.entry("routines").or_default());
    for (row, index) in visible.iter().enumerate().skip(first).take(inner.height as usize) {
        let rect = Rect::new(inner.x, inner.y + (row - first) as u16, inner.width, 1);
        app.hits.push((rect, Hit::Routine(*index)));
    }
}

fn routine_row(routine: &Routine, app: &App, now: i64) -> Line<'static> {
    let dim = !routine.enabled;
    let icon = match (app.running.get(&routine.name), app.last_run(&routine.name)) {
        (Some(_), _) => Span::styled(format!("{} ", spinner(app.clock)), Style::new().fg(WARN)),
        (None, Some(run)) if run.ok => Span::styled("✓ ", Style::new().fg(OK)),
        (None, Some(_)) => Span::styled("✗ ", Style::new().fg(BAD)),
        (None, None) => Span::styled("· ", Style::new().fg(MUTED)),
    };
    let kind_color = if routine.is_scheduled() { WARN } else { Color::Magenta };
    let mut spans = vec![
        icon,
        Span::styled(routine.name.clone(), if dim { Style::new().fg(MUTED) } else { Style::new() }),
        Span::raw(" "),
        Span::styled(routine.kind_label(), Style::new().fg(if dim { MUTED } else { kind_color })),
    ];
    if let Some(progress) = app.running.get(&routine.name) {
        spans.push(Span::styled(format!(" {}/{}", progress.position, progress.total), Style::new().fg(WARN)));
    } else if dim {
        spans.push(Span::styled(" (desativada)", Style::new().fg(MUTED).italic()));
    } else if let Some(next) = app.next_runs.get(&routine.name).filter(|_| app.daemon_up) {
        spans.push(Span::styled(format!(" ⏱ {}", countdown(next - now)), Style::new().fg(MUTED)));
    }
    Line::from(spans)
}

/// Lista com barra de seleção que desliza; devolve a primeira linha visível.
fn draw_list(
    buf: &mut Buffer,
    area: Rect,
    rows: Vec<Line>,
    selected: usize,
    focused: bool,
    slide: Slide,
    offset: &mut usize,
) -> usize {
    let height = area.height as usize;
    if height == 0 {
        return 0;
    }
    if selected < *offset {
        *offset = selected;
    } else if selected >= *offset + height {
        *offset = selected + 1 - height;
    }
    *offset = (*offset).min(rows.len().saturating_sub(height));
    let marker_row = slide.position().round() as usize;
    for (index, line) in rows.into_iter().enumerate().skip(*offset).take(height) {
        let y = area.y + (index - *offset) as u16;
        let row = Rect::new(area.x, y, area.width, 1);
        if line.style.bg.is_some() {
            buf.set_style(row, line.style);
        }
        if focused && index == selected {
            let width = ((area.width as f32) * slide.progress()).ceil() as u16;
            buf.set_style(Rect { width, ..row }, Style::new().bg(SELECTED_BG));
        }
        let marker = if focused && index == marker_row { "▶ " } else { "  " };
        let marker_style = Style::new().fg(ACCENT);
        buf.set_span(row.x, y, &Span::styled(marker, marker_style), 2);
        let style = if focused && index == selected { Style::new().bold() } else { Style::new() };
        buf.set_line(row.x + 2, y, &line.patch_style(style), row.width.saturating_sub(2));
    }
    *offset
}

fn draw_details(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(routine) = app.routine().cloned() else {
        let intro = Paragraph::new(vec![
            Line::from("Rotinas são sequências de passos: abrir apps em workspaces,"),
            Line::from("esperar janelas, atalhos, texto e cliques."),
            Line::from(""),
            Line::from("Livres rodam quando você chama (Enter, duplo clique ou `pcue run nome`)."),
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
        Constraint::Length(6),
        Constraint::Length(trigger_rows.clamp(3, 10)),
        Constraint::Min(0),
    ])
    .areas(area);
    draw_summary(frame, summary, app, &routine);
    draw_triggers(frame, triggers, app, &routine);
    draw_steps(frame, steps, app, &routine);
}

fn draw_summary(frame: &mut Frame, area: Rect, app: &App, routine: &Routine) {
    let block = pane_block("Detalhes", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [icon, text] = Layout::horizontal([Constraint::Length(10), Constraint::Min(0)]).areas(inner);

    let progress = app.running.get(&routine.name);
    let finished = app
        .finished
        .as_ref()
        .filter(|f| f.routine == routine.name && f.at.elapsed() < CELEBRATE_FOR);
    let (sprite, t) = match (progress, finished) {
        (Some(_), _) => (Some(Sprite::Running), millis(app)),
        (None, Some(f)) => (Some(if f.ok { Sprite::Ok } else { Sprite::Fail }), f.at.elapsed().as_millis() as u64),
        (None, None) => match (routine.triggers.first(), app.last_run(&routine.name)) {
            (Some(trigger), _) => (Some(trigger_sprite(trigger)), millis(app)),
            (None, Some(run)) => (Some(if run.ok { Sprite::Ok } else { Sprite::Fail }), sprites::HOLD),
            (None, None) => (None, 0),
        },
    };
    if let Some(sprite) = sprite {
        sprites::draw(frame.buffer_mut(), Rect { x: icon.x + 1, ..icon }, sprite, t);
    }

    let state = if routine.enabled { "ativa" } else { "desativada" };
    let steps = routine.steps.iter().filter(|s| !is_comment(s)).count();
    let mut lines = vec![Line::from(vec![
        Span::styled(routine.name.as_str(), Style::new().bold()),
        Span::styled(format!("  {} · {state} · {steps} passo(s)", routine.kind_label()), Style::new().fg(MUTED)),
    ])];
    lines.push(match app.last_run(&routine.name) {
        None => Line::styled("nunca executada", Style::new().fg(MUTED)),
        Some(run) => {
            let head = format!("última: {} ({}, {}) ", run.started, run.trigger, secs(run.duration_ms));
            match &run.error {
                None => Line::from(vec![Span::raw(head), Span::styled("ok", Style::new().fg(OK))]),
                Some(err) => {
                    let place = run.failed_line.map(|n| format!("passo {n}: ")).unwrap_or_default();
                    Line::from(vec![Span::raw(head), Span::styled(format!("{place}{err}"), Style::new().fg(BAD))])
                }
            }
        }
    });
    let now = jiff::Timestamp::now().as_second();
    lines.push(match app.next_runs.get(&routine.name) {
        Some(next) if routine.enabled && app.daemon_up => Line::from(vec![
            Span::styled("próxima: ", Style::new().fg(MUTED)),
            Span::styled(format!("em {}", countdown(next - now)), Style::new().fg(WARN)),
        ]),
        _ if routine.is_scheduled() && routine.enabled && !app.daemon_up => {
            Line::styled("daemon parado: gatilhos não disparam", Style::new().fg(BAD))
        }
        _ => Line::raw(""),
    });
    frame.render_widget(Paragraph::new(lines), text);

    if let Some(progress) = progress {
        let bar = Rect::new(text.x, text.y + 3, text.width, 1);
        draw_progress(frame.buffer_mut(), bar, progress.position, progress.total, app);
    }
}

fn trigger_sprite(trigger: &Trigger) -> Sprite {
    match trigger {
        Trigger::Event { .. } => Sprite::Event,
        Trigger::Daily { .. } => Sprite::Clock,
        Trigger::Interval { .. } => Sprite::Interval,
    }
}

/// Barra de progresso com um brilho correndo por cima.
fn draw_progress(buf: &mut Buffer, area: Rect, done: usize, total: usize, app: &App) {
    let label = format!(" {done}/{total}");
    let width = area.width.saturating_sub(label.len() as u16) as usize;
    let filled = (width * done.saturating_sub(1) / total.max(1)).min(width);
    let sheen = (millis(app) / 40) as usize % (width + 8);
    for i in 0..width {
        let x = area.x + i as u16;
        let (ch, color) = if i < filled {
            let glow = 1.0 - (i as f32 - sheen as f32).abs().min(4.0) / 4.0;
            ('█', mix((200, 160, 40), (255, 235, 140), glow))
        } else if i == filled {
            ('▌', Color::Yellow)
        } else {
            ('─', MUTED)
        };
        buf[(x, area.y)].set_char(ch).set_fg(color);
    }
    buf.set_string(area.x + width as u16, area.y, label, Style::new().fg(WARN));
}

fn draw_triggers(frame: &mut Frame, area: Rect, app: &mut App, routine: &Routine) {
    let focused = app.focus == Pane::Triggers;
    let block = focus_block(app, "Gatilhos", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.hits.push((area, Hit::Pane(Pane::Triggers)));
    if !routine.is_scheduled() {
        let text = Paragraph::new("Livre: roda quando chamada. a (aqui) adiciona um gatilho.").fg(MUTED);
        frame.render_widget(text, inner);
        return;
    }
    let now = jiff::Zoned::now();
    let rows: Vec<Line> = routine
        .triggers
        .iter()
        .map(|trigger| {
            let icon = match trigger {
                Trigger::Event { .. } => "⚡ ",
                Trigger::Daily { .. } => "◷ ",
                Trigger::Interval { .. } => "↻ ",
            };
            let mut spans = vec![Span::styled(icon, Style::new().fg(WARN)), Span::raw(trigger.to_string())];
            if let Some(next) = describe_next(trigger, &now) {
                spans.push(Span::styled(format!("  → {next}"), Style::new().fg(MUTED)));
            }
            Line::from(spans)
        })
        .collect();
    let count = rows.len();
    app.slides.triggers.follow(app.trigger_selected);
    let slide = app.slides.triggers;
    let first = draw_list(frame.buffer_mut(), inner, rows, app.trigger_selected, focused, slide, app.scroll.entry("triggers").or_default());
    push_row_hits(app, inner, first, count, Hit::Trigger);
}

fn push_row_hits(app: &mut App, area: Rect, first: usize, count: usize, hit: fn(usize) -> Hit) {
    for index in first..count.min(first + area.height as usize) {
        let rect = Rect::new(area.x, area.y + (index - first) as u16, area.width, 1);
        app.hits.push((rect, hit(index)));
    }
}

fn draw_steps(frame: &mut Frame, area: Rect, app: &mut App, routine: &Routine) {
    let focused = app.focus == Pane::Steps;
    let block = focus_block(app, "Passos", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.hits.push((area, Hit::Pane(Pane::Steps)));
    if routine.steps.is_empty() {
        let text = Paragraph::new("Sem passos. a adiciona, R grava teclado/mouse, c captura as janelas abertas.")
            .fg(MUTED)
            .wrap(Wrap { trim: true });
        frame.render_widget(text, inner);
        return;
    }
    let current = app.running.get(&routine.name).map(|p| p.line);
    let failed = app.last_run(&routine.name)
        .filter(|_| current.is_none())
        .and_then(|run| run.failed_line);
    let pulse = wave(app.clock, Duration::from_millis(900));
    let width = routine.steps.len().to_string().len();
    let rows: Vec<Line> = routine
        .steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let number = i + 1;
            let gutter = match (current, failed) {
                (Some(line), _) if line == number => Span::styled(format!("{:>width$}▸", number), Style::new().fg(WARN).bold()),
                (_, Some(line)) if line == number => Span::styled(format!("{:>width$}✗", number), Style::new().fg(BAD)),
                _ => Span::styled(format!("{:>width$} ", number), Style::new().fg(MUTED)),
            };
            let mut spans = vec![gutter];
            spans.extend(highlight_step(step));
            let line = Line::from(spans);
            if current == Some(number) {
                line.style(Style::new().bg(mix(RUNNING_BG_DIM, RUNNING_BG, pulse)))
            } else {
                line
            }
        })
        .collect();
    let count = rows.len();
    let selected = match current {
        Some(line) if !focused => line - 1,
        _ => app.step_selected,
    };
    app.slides.steps.follow(selected);
    let slide = app.slides.steps;
    let first = draw_list(frame.buffer_mut(), inner, rows, selected, focused, slide, app.scroll.entry("steps").or_default());
    push_row_hits(app, inner, first, count, Hit::Step);
}

/// Verbo em destaque, condições apagadas, comentários em itálico.
fn highlight_step(step: &str) -> Vec<Span<'_>> {
    if is_comment(step) {
        return vec![Span::styled(step, Style::new().fg(MUTED).italic())];
    }
    let trimmed = step.trim_start();
    let (verb, rest) = trimmed.split_at(trimmed.find(' ').unwrap_or(trimmed.len()));
    let color = match verb {
        "launch" => OK,
        "wait_window" | "sleep" => WARN,
        "key" | "type" => Color::Magenta,
        "click" | "scroll" => Color::Blue,
        "call" => ACCENT,
        _ => BAD,
    };
    let mut spans = vec![Span::styled(verb, Style::new().fg(color).bold())];
    for (i, word) in rest.split(' ').enumerate() {
        let guard = ["optional", "repeat=", "if_window=", "unless_window="]
            .iter()
            .any(|g| word.starts_with(g));
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(if guard {
            Span::styled(word, Style::new().fg(ACCENT).italic())
        } else {
            Span::raw(word)
        });
    }
    spans
}

/// Botões da barra inferior para o painel atual.
fn hints(app: &App) -> Vec<(&'static str, &'static str, KeyCode)> {
    let running = app.routine().is_some_and(|r| app.running.contains_key(&r.name));
    let c = KeyCode::Char;
    let mut list = match app.focus {
        Pane::Routines => {
            let mut list = vec![("⏎", "rodar", KeyCode::Enter)];
            if running {
                list.push(("s", "parar", c('s')));
            }
            list.extend([
                ("n", "nova", c('n')),
                ("D", "duplicar", c('D')),
                ("r", "renomear", c('r')),
                ("d", "apagar", c('d')),
                ("␣", "ativar", c(' ')),
                ("t", "livre/agendada", c('t')),
                ("/", "buscar", c('/')),
                ("X", "exportar", c('X')),
                ("i", "importar", c('i')),
            ]);
            list
        }
        Pane::Triggers => vec![("a", "adicionar", c('a')), ("⏎", "editar", KeyCode::Enter), ("d", "apagar", c('d'))],
        Pane::Steps => vec![
            ("a", "adicionar", c('a')),
            ("⏎", "editar", KeyCode::Enter),
            ("x", "rodar só este", c('x')),
            ("f", "rodar daqui", c('f')),
            ("d", "apagar", c('d')),
            ("J/K", "mover", c('J')),
            ("R", "gravar", c('R')),
            ("c", "capturar janelas", c('c')),
            ("E", "editor", c('E')),
        ],
    };
    list.extend([
        ("H", "histórico", c('H')),
        ("L", "log", c('L')),
        ("?", "ajuda", c('?')),
        ("q", "sair", c('q')),
    ]);
    list
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &mut App) {
    let [status, buttons] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    if let Some(s) = app.status() {
        let fresh = anim::ease_out(s.at, Duration::from_millis(300));
        let text = format!(" {}", s.text);
        let shown: String = text.chars().take((text.chars().count() as f32 * fresh).ceil() as usize).collect();
        let color = if s.error { BAD } else { OK };
        frame.render_widget(Paragraph::new(shown).fg(color), status);
    }
    let mut x = buttons.x + 1;
    let mut y = buttons.y;
    for (key, label, code) in hints(app) {
        let width = (key.chars().count() + label.chars().count() + 3) as u16;
        if x + width > buttons.right() {
            x = buttons.x + 1;
            y += 1;
            if y >= buttons.bottom() {
                break;
            }
        }
        let line = Line::from(vec![
            Span::styled(format!(" {key} "), Style::new().fg(Color::Black).bg(MUTED)),
            Span::styled(format!(" {label}"), Style::new().fg(MUTED)),
        ]);
        frame.buffer_mut().set_line(x, y, &line, width);
        app.hits.push((Rect::new(x, y, width, 1), Hit::Key(code)));
        x += width + 1;
    }
}

/// Retângulo da janela sobreposta, crescendo do centro na abertura.
fn popup(app: &App, full: Rect, width: u16, height: u16) -> Rect {
    let target = centered(full, width, height);
    let grow = anim::ease_out(app.modal_at, POP);
    let height = ((target.height as f32) * (0.3 + 0.7 * grow)).round().max(3.0).min(target.height as f32) as u16;
    let lift = ((1.0 - grow) * 2.0).round() as u16;
    Rect {
        y: target.y + (target.height - height) / 2 + lift,
        height,
        ..target
    }
    .intersection(full)
}

fn draw_modal(frame: &mut Frame, app: &mut App) {
    let full = frame.area();
    let Some(modal) = app.modal.take() else { return };
    match &modal {
        Modal::Input { title, hint, input, error, .. } => {
            let area = popup(app, full, 84, 10);
            frame.render_widget(Clear, area);
            let block = pane_block(title, true);
            let inner = block.inner(area);
            frame.render_widget(block, area);
            let [hint_area, input_area, error_area, buttons] = Layout::vertical([
                Constraint::Length(4),
                Constraint::Length(1),
                Constraint::Length(2),
                Constraint::Length(1),
            ])
            .areas(inner);
            frame.render_widget(Paragraph::new(hint.as_str()).fg(MUTED).wrap(Wrap { trim: true }), hint_area);
            let prefix = "› ";
            let visible = input_area.width.saturating_sub(prefix.len() as u16 + 1) as usize;
            let skip = input.cursor().saturating_sub(visible);
            let shown: String = input.value().chars().skip(skip).collect();
            frame.render_widget(Paragraph::new(format!("{prefix}{shown}")), input_area);
            if input_area.height > 0 {
                let cursor_x = input_area.x + 2 + (input.cursor() - skip) as u16;
                frame.set_cursor_position(Position::new(cursor_x, input_area.y));
            }
            if let Some(err) = error {
                frame.render_widget(Paragraph::new(err.as_str()).fg(BAD).wrap(Wrap { trim: true }), error_area);
            }
            draw_buttons(frame, app, buttons, &[("salvar", KeyCode::Enter, true), ("cancelar", KeyCode::Esc, false)]);
        }
        Modal::Confirm { prompt, .. } => {
            let area = popup(app, full, 60, 7);
            frame.render_widget(Clear, area);
            let block = pane_block("Confirmar", true);
            let inner = block.inner(area);
            frame.render_widget(block, area);
            let [text, buttons] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
            frame.render_widget(Paragraph::new(prompt.as_str()).wrap(Wrap { trim: true }), text);
            draw_buttons(frame, app, buttons, &[("sim (s)", KeyCode::Char('s'), true), ("não (n)", KeyCode::Esc, false)]);
        }
        Modal::Pick { title, options, selected, .. } => {
            let area = popup(app, full, 72, options.len() as u16 + 2);
            frame.render_widget(Clear, area);
            let block = pane_block(title, true);
            let inner = block.inner(area);
            frame.render_widget(block, area);
            let rows: Vec<Line> = options
                .iter()
                .map(|(label, desc)| {
                    Line::from(vec![
                        Span::styled(format!("{label:<16}"), Style::new().bold()),
                        Span::styled(desc.as_str(), Style::new().fg(MUTED)),
                    ])
                })
                .collect();
            app.slides.modal.follow(*selected);
            let first = draw_list(frame.buffer_mut(), inner, rows, *selected, true, app.slides.modal, app.scroll.entry("pick").or_default());
            push_row_hits(app, inner, first, options.len(), Hit::Option);
        }
        Modal::Help => {
            let lines: Vec<Line> = HELP_ROUTINES
                .iter()
                .map(|l| Line::raw(*l))
                .chain(SYNTAX.iter().map(|l| Line::raw(format!("  {l}"))))
                .chain(HELP_GENERAL.iter().map(|l| Line::raw(*l)))
                .collect();
            let area = popup(app, full, 100, lines.len() as u16 + 2);
            frame.render_widget(Clear, area);
            frame.render_widget(Paragraph::new(lines).block(pane_block("Ajuda", true)), area);
        }
        Modal::Log { lines, scroll } => {
            let area = popup(app, full, full.width.saturating_sub(4), full.height.saturating_sub(2));
            frame.render_widget(Clear, area);
            let height = area.height.saturating_sub(2) as usize;
            let top = (*scroll).min(lines.len().saturating_sub(height));
            let shown: Vec<Line> = lines.iter().skip(top).take(height).map(|l| log_line(l)).collect();
            let title = "Log · j/k PgUp/PgDn g/G · Esc fecha";
            frame.render_widget(Paragraph::new(shown).block(pane_block(title, true)), area);
            app.hits.push((area, Hit::Pane(Pane::Routines)));
        }
        Modal::History { runs, selected, only } => {
            let area = popup(app, full, full.width.saturating_sub(4), full.height.saturating_sub(2));
            draw_history(frame, app, area, runs, *selected, only.as_deref());
        }
    }
    app.modal = Some(modal);
}

/// Linha do log com falhas em vermelho e pulos apagados.
fn log_line(line: &str) -> Line<'_> {
    let style = if line.contains("falhou") || line.contains("erro") {
        Style::new().fg(BAD)
    } else if line.contains("pulado") || line.contains("opcional") {
        Style::new().fg(MUTED)
    } else if line.contains("concluída") {
        Style::new().fg(OK)
    } else {
        Style::new()
    };
    Line::styled(line, style)
}

fn draw_buttons(frame: &mut Frame, app: &mut App, area: Rect, buttons: &[(&str, KeyCode, bool)]) {
    let mut x = area.x;
    for (label, code, primary) in buttons {
        let text = format!("  {label}  ");
        let width = text.chars().count() as u16;
        let style = if *primary {
            Style::new().fg(Color::Black).bg(ACCENT).bold()
        } else {
            Style::new().fg(Color::White).bg(SELECTED_BG)
        };
        frame.buffer_mut().set_stringn(x, area.y, &text, width as usize, style);
        app.hits.push((Rect::new(x, area.y, width, 1), Hit::Key(*code)));
        x += width + 2;
    }
}

fn draw_history(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    runs: &[crate::history::Run],
    selected: usize,
    only: Option<&str>,
) {
    frame.render_widget(Clear, area);
    let title = match only {
        Some(name) => format!("Histórico · {name}"),
        None => "Histórico".to_string(),
    };
    let block = pane_block(&title, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [body, hint] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    frame.render_widget(
        Paragraph::new("⏎ ir ao passo · f filtra pela rotina selecionada · Esc fecha").fg(MUTED),
        hint,
    );
    if runs.is_empty() {
        frame.render_widget(Paragraph::new("nenhuma execução registrada").fg(MUTED), body);
        return;
    }
    let [list, detail] = Layout::horizontal([Constraint::Percentage(58), Constraint::Min(0)]).areas(body);
    let rows: Vec<Line> = runs
        .iter()
        .map(|run| {
            let (icon, color) = if run.ok { ("✓", OK) } else { ("✗", BAD) };
            Line::from(vec![
                Span::styled(format!("{icon} "), Style::new().fg(color)),
                Span::styled(format!("{} ", run.started), Style::new().fg(MUTED)),
                Span::styled(format!("{:<16} ", run.routine), Style::new().bold()),
                Span::raw(format!("{:>7} ", secs(run.duration_ms))),
                Span::styled(run.trigger.as_str(), Style::new().fg(MUTED)),
            ])
        })
        .collect();
    app.slides.modal.follow(selected);
    let first = draw_list(frame.buffer_mut(), list, rows, selected, true, app.slides.modal, app.scroll.entry("history").or_default());
    push_row_hits(app, list, first, runs.len(), Hit::HistoryRow);

    let Some(run) = runs.get(selected) else { return };
    let mut lines = vec![
        Line::from(Span::styled(run.routine.as_str(), Style::new().bold().fg(ACCENT))),
        Line::raw(""),
        Line::from(vec![Span::styled("início   ", Style::new().fg(MUTED)), Span::raw(run.started.as_str())]),
        Line::from(vec![Span::styled("gatilho  ", Style::new().fg(MUTED)), Span::raw(run.trigger.as_str())]),
        Line::from(vec![
            Span::styled("duração  ", Style::new().fg(MUTED)),
            Span::raw(secs(run.duration_ms)),
        ]),
        Line::from(vec![
            Span::styled("resultado ", Style::new().fg(MUTED)),
            if run.ok { Span::styled("ok", Style::new().fg(OK)) } else { Span::styled("falhou", Style::new().fg(BAD)) },
        ]),
    ];
    if let Some(line) = run.failed_line {
        let step = app
            .config
            .find(&run.routine)
            .and_then(|r| r.steps.get(line - 1))
            .cloned()
            .unwrap_or_else(|| "(passo não existe mais)".into());
        lines.push(Line::raw(""));
        lines.push(Line::styled(format!("passo {line}"), Style::new().fg(MUTED)));
        let mut spans = vec![Span::raw("  ")];
        spans.extend(highlight_step(&step).into_iter().map(|s| Span::styled(s.content.into_owned(), s.style)));
        lines.push(Line::from(spans));
    }
    if let Some(err) = &run.error {
        lines.push(Line::raw(""));
        lines.push(Line::styled("erro", Style::new().fg(MUTED)));
        lines.push(Line::styled(err.clone(), Style::new().fg(BAD)));
    }
    let detail = Rect { x: detail.x + 2, width: detail.width.saturating_sub(2), ..detail };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), detail);
}

fn draw_recording(frame: &mut Frame, app: &mut App) {
    let full = frame.area();
    let Some(recording) = app.recording.as_ref() else { return };
    let t = millis(app);
    let (title, color) = match recording.phase {
        RecPhase::Countdown(_) => ("Prepare-se", WARN),
        RecPhase::Recording => ("Gravando", BAD),
        RecPhase::Paused => ("Pausado", WARN),
    };
    let block = pane_block("", true).border_style(Style::new().fg(color)).title(Span::styled(
        format!(" {title} · {} ", recording.routine),
        Style::new().fg(color).bold(),
    ));
    let inner = block.inner(full);
    frame.render_widget(block, full);
    let [top, steps, bottom] =
        Layout::vertical([Constraint::Length(9), Constraint::Min(0), Constraint::Length(2)]).areas(inner);

    let [art, big, info] =
        Layout::horizontal([Constraint::Length(20), Constraint::Length(44), Constraint::Min(0)]).areas(top);
    let mascot = if recording.phase == RecPhase::Paused { Sprite::MascotSleep } else { Sprite::Mascot };
    sprites::draw(frame.buffer_mut(), Rect { x: art.x + 2, y: art.y, ..art }, mascot, t);
    let icon = match recording.phase {
        RecPhase::Paused => Sprite::Paused,
        _ => Sprite::Rec,
    };
    if recording.phase != RecPhase::Countdown(0) {
        sprites::draw(frame.buffer_mut(), Rect::new(big.x, big.y + 2, 8, 4), icon, t);
    }
    let text = match recording.phase {
        RecPhase::Countdown(n) => n.to_string(),
        _ => {
            let secs = recording.started.map_or(0, |s| s.elapsed().as_secs());
            format!("{:02}:{:02}", secs / 60, secs % 60)
        }
    };
    draw_big_text(frame.buffer_mut(), Rect::new(big.x + 10, big.y + 2, big.width.saturating_sub(10), 5), &text, color);

    let info_lines = vec![
        Line::raw(""),
        Line::from(match recording.phase {
            RecPhase::Countdown(_) => "Troque para a janela onde vai agir.",
            RecPhase::Recording => "Teclado e mouse viram passos.",
            RecPhase::Paused => "Nada é gravado agora (bom para senhas).",
        }),
        Line::raw(""),
        Line::from(vec![Span::styled(" SUPER+F9 ", Style::new().fg(Color::Black).bg(WARN)), Span::raw(" pausa/retoma")]),
        Line::from(vec![Span::styled(" SUPER+F10 ", Style::new().fg(Color::Black).bg(BAD)), Span::raw(" encerra e salva")]),
        Line::raw(""),
        Line::styled(format!("{} passo(s)", recording.steps.len()), Style::new().fg(MUTED)),
    ];
    frame.render_widget(Paragraph::new(info_lines), info);

    draw_recorded_steps(frame, steps, recording);

    let stop = "  ■ parar e salvar  ";
    let width = stop.chars().count() as u16;
    let button = Rect::new(bottom.x + 1, bottom.y + 1, width, 1).intersection(bottom);
    frame.buffer_mut().set_string(button.x, button.y, stop, Style::new().fg(Color::White).bg(BAD).bold());
    frame.buffer_mut().set_string(
        button.right() + 2,
        button.y,
        "o terminal ignora as teclas enquanto grava",
        Style::new().fg(MUTED),
    );
    app.hits.push((button, Hit::StopRecording));
}

fn draw_recorded_steps(frame: &mut Frame, area: Rect, recording: &Recording) {
    let block = pane_block("Passos gravados", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let height = inner.height as usize;
    let start = recording.steps.len().saturating_sub(height);
    let fresh = anim::ease_out(recording.last_step, Duration::from_millis(500));
    let last = recording.steps.len().saturating_sub(1);
    for (row, (index, step)) in recording.steps.iter().enumerate().skip(start).enumerate() {
        let y = inner.y + row as u16;
        let mut spans = vec![Span::styled(format!("{:>3} ", index + 1), Style::new().fg(MUTED))];
        spans.extend(highlight_step(step));
        let mut line = Line::from(spans);
        let mut x = inner.x;
        if index == last {
            line = line.style(Style::new().bg(mix((90, 30, 30), (30, 30, 36), fresh)));
            x += ((1.0 - fresh) * 6.0) as u16;
        }
        frame.buffer_mut().set_line(x, y, &line, inner.right().saturating_sub(x));
    }
    if recording.steps.is_empty() {
        frame.render_widget(Paragraph::new("os passos aparecem aqui enquanto você age").fg(MUTED), inner);
    }
}

/// Dígitos grandes 3x5 feitos de blocos.
fn draw_big_text(buf: &mut Buffer, area: Rect, text: &str, color: Color) {
    let mut x = area.x;
    for ch in text.chars() {
        let glyph = big_glyph(ch);
        let width = glyph[0].chars().count() as u16;
        for (row, bits) in glyph.iter().enumerate() {
            for (col, bit) in bits.chars().enumerate() {
                let (cx, cy) = (x + col as u16 * 2, area.y + row as u16);
                if bit == '#' && cx + 1 < area.right() && cy < area.bottom() {
                    buf[(cx, cy)].set_char('█').set_fg(color);
                    buf[(cx + 1, cy)].set_char('█').set_fg(color);
                }
            }
        }
        x += width * 2 + 1;
    }
}

fn big_glyph(ch: char) -> [&'static str; 5] {
    match ch {
        '0' => ["###", "#.#", "#.#", "#.#", "###"],
        '1' => [".#.", "##.", ".#.", ".#.", "###"],
        '2' => ["###", "..#", "###", "#..", "###"],
        '3' => ["###", "..#", ".##", "..#", "###"],
        '4' => ["#.#", "#.#", "###", "..#", "..#"],
        '5' => ["###", "#..", "###", "..#", "###"],
        '6' => ["###", "#..", "###", "#.#", "###"],
        '7' => ["###", "..#", ".#.", ".#.", ".#."],
        '8' => ["###", "#.#", "###", "#.#", "###"],
        '9' => ["###", "#.#", "###", "..#", "###"],
        ':' => [".", "#", ".", "#", "."],
        _ => [".", ".", ".", ".", "."],
    }
}

/// Duração em segundos com uma casa.
fn secs(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
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
    "  livre     roda quando chamada: Enter ou duplo clique, `pcue run nome` ou um atalho do Hyprland",
    "  agendada  roda pelo daemon: eventos (login, lock, unlock, resume, monitor_*),",
    "            horário diário ou intervalo; eventos próprios via `pcue emit nome`",
    "",
    "Passos",
];

const HELP_GENERAL: &[&str] = &[
    "",
    "Depuração (painel de passos)",
    "  x roda só o passo selecionado · f roda a partir dele · s (rotinas) interrompe",
    "  `pcue run nome --from N` ou `--only N` faz o mesmo pela linha de comando",
    "",
    "Gravação (R no painel de passos)",
    "  SUPER+F9 pausa/retoma (use em senhas) · SUPER+F10 encerra",
    "  cliques viram posição relativa à janela, com wait_window antes",
    "",
    "Geral",
    "  mouse: clique seleciona, duplo clique roda/edita, roda do mouse rola",
    "  / busca · D duplica · X exporta · i importa · H histórico · L log · I autostart · q sai",
];
