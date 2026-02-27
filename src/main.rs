mod app;
mod components;
mod error;
mod event;
mod fs;
mod handler;
mod tui;
mod ui;

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;

use crate::app::App;
use crate::event::{Event, EventHandler};
use crate::tui::{install_panic_hook, Tui};

/// A terminal-based file manager TUI.
#[derive(Parser, Debug)]
#[command(name = "file_manager_tui", version, about)]
struct Cli {
    /// Root path to display (defaults to current directory)
    #[arg(default_value = ".")]
    path: PathBuf,
}

#[tokio::main]
async fn main() -> error::Result<()> {
    let cli = Cli::parse();

    let path = cli.path.canonicalize().map_err(|_| {
        error::AppError::InvalidPath(format!("{} does not exist", cli.path.display()))
    })?;

    install_panic_hook();

    let mut tui = Tui::new()?;
    let mut app = App::new(&path)?;
    let mut events = EventHandler::new(Duration::from_millis(16));

    loop {
        tui.terminal_mut().draw(|frame| {
            ui::render(&mut app, frame);
        })?;

        match events.next().await? {
            Event::Key(key) => handler::handle_key_event(&mut app, key),
            Event::Tick => {}
            Event::Resize(_, _) => {}
        }

        if app.should_quit {
            break;
        }
    }

    tui.restore()?;
    Ok(())
}
