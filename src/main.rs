mod app;
mod components;
mod error;
mod event;
mod fs;
mod handler;
mod preview_content;
mod tui;
mod ui;

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;

use crate::app::App;
use crate::event::{Event, EventHandler};
use crate::fs::watcher::FsWatcher;
use crate::tui::{install_panic_hook, Tui};

/// A terminal-based file manager TUI.
#[derive(Parser, Debug)]
#[command(name = "file_manager_tui", version, about)]
struct Cli {
    /// Root path to display (defaults to current directory)
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Disable filesystem watcher (auto-refresh)
    #[arg(long)]
    no_watcher: bool,
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
    let event_tx = events.sender();

    // Initialize filesystem watcher (unless --no-watcher)
    let _watcher = if cli.no_watcher {
        app.watcher_active = false;
        None
    } else {
        let ignore_patterns: Vec<String> = fs::watcher::DEFAULT_IGNORE_PATTERNS
            .iter()
            .map(|s| s.to_string())
            .collect();

        match FsWatcher::new(
            &path,
            Duration::from_millis(fs::watcher::DEFAULT_DEBOUNCE_MS),
            ignore_patterns,
            fs::watcher::DEFAULT_FLOOD_THRESHOLD,
            event_tx.clone(),
        ) {
            Ok(watcher) => Some(watcher),
            Err(e) => {
                app.watcher_active = false;
                app.set_status_message(format!("⚠ Watcher unavailable: {}", e));
                None
            }
        }
    };

    loop {
        tui.terminal_mut().draw(|frame| {
            ui::render(&mut app, frame);
        })?;

        match events.next().await? {
            Event::Key(key) => handler::handle_key_event(&mut app, key, &event_tx),
            Event::Tick => {}
            Event::Resize(_, _) => {}
            Event::Progress(update) => app.handle_progress(update),
            Event::OperationComplete(result) => app.handle_operation_complete(result),
            Event::FsChange(paths) => app.handle_fs_change(paths),
        }

        // Sync watcher pause/resume state
        if let Some(ref watcher) = _watcher {
            if app.watcher_active && !watcher.is_active() {
                watcher.resume();
            } else if !app.watcher_active && watcher.is_active() {
                watcher.pause();
            }
        }

        if app.should_quit {
            break;
        }
    }

    tui.restore()?;
    Ok(())
}
