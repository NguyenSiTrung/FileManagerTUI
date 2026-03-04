mod app;
mod components;
mod config;
mod editor;
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;

use crate::app::{App, AppMode};
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
                max_entries_per_page: None,
                search_max_entries: None,
                snapshot_max_entries: None,
                max_editor_bytes: None,
                max_editor_lines: None,
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
                preview_timeout_ms: None,
            },
            tree: TreeConfig {
                sort_by: None,
                dirs_first: None,
                use_icons: if self.no_icons { Some(false) } else { None },
                scroll_lines: None,
            },
            watcher: WatcherConfig {
                enabled: if self.no_watcher { Some(false) } else { None },
                debounce_ms: None,
                auto_refresh: None,
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
    app.event_tx = Some(event_tx.clone());

    let (watcher_flag_tx, watcher_flag_rx) = std::sync::mpsc::channel::<Arc<AtomicBool>>();
    let mut watcher_flag_rx = Some(watcher_flag_rx);
    let mut watcher_flag: Option<Arc<AtomicBool>> = None;

    // Initialize filesystem watcher in the background so startup stays responsive
    // even for very deep/large directory trees.
    let _watcher_thread = if !app.config.watcher_enabled() {
        app.watcher_active = false;
        None
    } else {
        let root = path.clone();
        let debounce = Duration::from_millis(app.config.debounce_ms());
        let watcher_tx = event_tx.clone();
        let watcher_flag_tx = watcher_flag_tx.clone();
        let ignore_patterns: Vec<String> = fs::watcher::DEFAULT_IGNORE_PATTERNS
            .iter()
            .map(|s| s.to_string())
            .collect();

        Some(std::thread::spawn(move || {
            match FsWatcher::new(
                &root,
                debounce,
                ignore_patterns,
                fs::watcher::DEFAULT_FLOOD_THRESHOLD,
                watcher_tx.clone(),
            ) {
                Ok(watcher) => {
                    let _ = watcher_flag_tx.send(watcher.active_flag());
                    loop {
                        std::thread::sleep(Duration::from_secs(3600));
                    }
                }
                Err(e) => {
                    let _ = watcher_tx.send(Event::WatcherInitFailed(e.to_string()));
                }
            }
        }))
    };

    // Kick off async loading of root directory contents.
    // The UI renders immediately with a "Loading..." placeholder.
    app.spawn_initial_load(&event_tx);

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
            Event::DirScanComplete { path, snapshot } => {
                app.handle_dir_scan_complete(&path, snapshot);
            }
            Event::DirCountComplete { path, count } => {
                app.handle_dir_count_complete(&path, count);
            }
            Event::DirSummaryUpdate {
                path,
                files,
                dirs,
                size,
                done,
            } => {
                app.handle_dir_summary_update(&path, files, dirs, size, done);
            }
            Event::ShallowDirSummary { path, lines, total } => {
                app.handle_shallow_dir_summary(&path, lines, total);
            }
            Event::ClipboardCopyComplete(message) => {
                // Re-enable mouse capture if we were in CopyOverlay mode
                if app.config.mouse_enabled() {
                    let _ = crossterm::execute!(
                        tui.terminal_mut().backend_mut(),
                        crossterm::event::EnableMouseCapture
                    );
                }
                if !message.is_empty() {
                    app.set_status_message(message);
                }
            }
            Event::ShowCopyableText(text) => {
                // Save to temp file as backup
                let _ = std::fs::write("/tmp/.fm_clipboard", &text);

                // Try OSC 52 for auto-clipboard (works on terminals that support it)
                {
                    use base64::Engine;
                    use std::io::Write;
                    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
                    let backend = tui.terminal_mut().backend_mut();
                    let _ = write!(backend, "\x1b]52;c;{}\x07", encoded);
                    let _ = write!(backend, "\x1b]52;c;{}\x1b\\", encoded);
                    let _ = backend.flush();
                }

                // Disable mouse capture so the browser/xterm.js can handle
                // native text selection + Ctrl+C for clipboard copy.
                // The TUI stays visible with a copy overlay.
                let _ = crossterm::execute!(
                    tui.terminal_mut().backend_mut(),
                    crossterm::event::DisableMouseCapture
                );

                app.copy_overlay_text = Some(text);
                app.mode = AppMode::CopyOverlay;
            }
            Event::WatcherInitFailed(msg) => {
                app.watcher_active = false;
                app.set_status_message(format!("⚠ Watcher unavailable: {}", msg));
            }
        }

        if watcher_flag.is_none() {
            if let Some(rx) = &watcher_flag_rx {
                match rx.try_recv() {
                    Ok(flag) => {
                        watcher_flag = Some(flag);
                        watcher_flag_rx = None;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        watcher_flag_rx = None;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }
            }
        }

        if let Some(flag) = &watcher_flag {
            flag.store(app.watcher_active, Ordering::Relaxed);
        }

        if app.should_quit {
            break;
        }
    }

    app.shutdown_terminal();
    tui.restore()?;
    Ok(())
}
