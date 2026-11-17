mod anim;
mod app;
mod sprites;
mod text_input;
mod ui;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use ratatui::crossterm::execute;

use app::App;

/// Intervalo entre quadros enquanto algo anima.
const FRAME: Duration = Duration::from_millis(33);
/// Intervalo entre quadros com a tela parada.
const IDLE_FRAME: Duration = Duration::from_millis(250);

/// Abre a interface até o usuário sair.
pub fn run(path: PathBuf) -> Result<()> {
    let mut app = App::new(path)?;
    let mut terminal = enter()?;
    let result = loop {
        if let Err(err) = terminal.draw(|frame| ui::draw(frame, &mut app)) {
            break Err(err.into());
        }
        if app.should_quit() {
            break Ok(());
        }
        let wait = if app.animating() { FRAME } else { IDLE_FRAME };
        match event::poll(wait) {
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => app.on_key(key),
                Ok(Event::Mouse(mouse)) => app.on_mouse(mouse),
                Ok(_) => {}
                Err(err) => break Err(err.into()),
            },
            Ok(false) => {}
            Err(err) => break Err(err.into()),
        }
        app.refresh();
        if app.take_editor_request() {
            leave();
            app.edit_in_editor();
            terminal = enter()?;
            drain_events();
        }
    };
    leave();
    result
}

fn enter() -> Result<ratatui::DefaultTerminal> {
    let terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture)?;
    Ok(terminal)
}

fn leave() {
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
}

/// Descarta teclas digitadas enquanto o terminal estava fora da TUI.
fn drain_events() {
    while event::poll(Duration::ZERO).unwrap_or(false) {
        let _ = event::read();
    }
}
