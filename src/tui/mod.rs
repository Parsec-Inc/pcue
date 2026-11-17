mod app;
mod text_input;
mod ui;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use app::{App, External};

/// Frequência de leitura do teclado.
const INPUT_POLL: Duration = Duration::from_millis(250);

/// Abre a interface até o usuário sair.
pub fn run(path: PathBuf) -> Result<()> {
    let mut app = App::new(path)?;
    let mut terminal = ratatui::init();
    let result = loop {
        if let Err(err) = terminal.draw(|frame| ui::draw(frame, &mut app)) {
            break Err(err.into());
        }
        if app.should_quit() {
            break Ok(());
        }
        match event::poll(INPUT_POLL) {
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => app.on_key(key),
                Ok(_) => {}
                Err(err) => break Err(err.into()),
            },
            Ok(false) => {}
            Err(err) => break Err(err.into()),
        }
        app.refresh();
        if let Some(external) = app.take_external() {
            ratatui::restore();
            app.run_external(external);
            terminal = ratatui::init();
            drain_events();
        }
    };
    ratatui::restore();
    result
}

/// Descarta teclas digitadas enquanto o terminal estava fora da TUI.
fn drain_events() {
    while event::poll(Duration::ZERO).unwrap_or(false) {
        let _ = event::read();
    }
}

impl External {
    /// Rótulo para mensagens.
    pub fn label(&self) -> &'static str {
        match self {
            External::Record => "gravação",
            External::Editor => "editor",
        }
    }
}
