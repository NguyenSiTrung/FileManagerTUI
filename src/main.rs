mod app;
mod components;
mod config;
mod error;
mod event;
mod fs;
mod handler;
mod preview_content;
mod terminal;
mod theme;
mod tui;
mod ui;

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;

use crate::app::App;
use crate::config::{AppConfig, GeneralConfig, PreviewConfig, TreeConfig, WatcherConfig};
use crate::event::{Event, EventHandler};
use crate::fs::watcher::FsWatcher;
use crate::tui::{install_panic_hook, Tui};

/// A terminal-based file manager TUI.
#[derive(Parser, Debug)]
#[command(name = "fm", version, about)]
struct Cli {
    /// Root path to display (defaults to current directory)
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Path to config file
    #[arg(short = 'c', long = "config")]
    config: Option<PathBuf>,

    /// Disable preview panel
    #[arg(long)]
    no_preview: bool,

    /// Disable filesystem watcher (auto-refresh)
    #[arg(long)]
    no_watcher: bool,

    /// Use ASCII instead of Nerd Font icons
    #[arg(long)]
    no_icons: bool,

    /// Disable mouse support
    #[arg(long)]
    no_mouse: bool,

    /// Disable embedded terminal
    #[arg(long)]
    no_terminal: bool,

    /// Lines from top for large file preview
    #[arg(long)]
    head_lines: Option<usize>,

    /// Lines from bottom for large file preview
    #[arg(long)]
    tail_lines: Option<usize>,

    /// Max file size (bytes) for full preview
    #[arg(long)]
    max_preview: Option<u64>,

    /// Color theme: dark, light
    #[arg(long)]
    theme: Option<String>,
}

impl Cli {
    /// Convert CLI flags into a partial `AppConfig` for the merge chain.
    /// Only flags that were explicitly set produce `Some` values.
    fn as_config_overrides(&self) -> AppConfig {
        AppConfig {
            general: GeneralConfig {
                default_path: None, // path is handled separately via positional arg
                show_hidden: None,
                confirm_delete: None,
                mouse: if self.no_mouse { Some(false) } else { None },
            },
            preview: PreviewConfig {
                max_full_preview_bytes: self.max_preview,
                head_lines: self.head_lines,
                tail_lines: self.tail_lines,
                default_view_mode: None,
                tab_width: None,
                line_wrap: None,
                syntax_theme: None,
                enabled: if self.no_preview { Some(false) } else { None },
            },
            tree: TreeConfig {
                sort_by: None,
                dirs_first: None,
                use_icons: if self.no_icons { Some(false) } else { None },
            },
            watcher: WatcherConfig {
                enabled: if self.no_watcher { Some(false) } else { None },
                debounce_ms: None,
            },
            terminal: crate::config::TerminalConfig {
                enabled: if self.no_terminal { Some(false) } else { None },
                default_shell: None,
                scrollback_lines: None,
            },
            theme: crate::config::ThemeConfig {
                scheme: self.theme.clone(),
                custom: None,
            },
        }
    }
}

#[tokio::main]
async fn main() -> error::Result<()> {
    let cli = Cli::parse();

    let path = cli.path.canonicalize().map_err(|_| {
        error::AppError::InvalidPath(format!("{} does not exist", cli.path.display()))
    })?;

    // Load configuration: file sources + CLI overrides
    let cli_overrides = cli.as_config_overrides();
    let config = AppConfig::load(cli.config.as_deref(), Some(&cli_overrides));

    install_panic_hook();

    let mut app = App::new(&path, config)?;
    let mut tui = Tui::new(app.config.mouse_enabled())?;
    let mut events = EventHandler::new(Duration::from_millis(16));
    let event_tx = events.sender();

    // Initialize filesystem watcher (using merged config)
    let _watcher = if !app.config.watcher_enabled() {
        app.watcher_active = false;
        None
    } else {
        let ignore_patterns: Vec<String> = fs::watcher::DEFAULT_IGNORE_PATTERNS
            .iter()
            .map(|s| s.to_string())
            .collect();

        match FsWatcher::new(
            &path,
            Duration::from_millis(app.config.debounce_ms()),
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
            Event::Mouse(mouse) => handler::handle_mouse_event(&mut app, mouse, &event_tx),
            Event::Tick => {}
            Event::Resize(_, _) => {}
            Event::Progress(update) => app.handle_progress(update),
            Event::OperationComplete(result) => app.handle_operation_complete(result),
            Event::FsChange(paths) => app.handle_fs_change(paths),
            Event::TerminalOutput(data) => app.terminal_state.emulator.process(&data),
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

    app.shutdown_terminal();
    tui.restore()?;
    Ok(())
}
