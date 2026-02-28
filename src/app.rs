use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use ratatui::layout::Rect;
use ratatui::text::Line;
use syntect::highlighting::Theme;
use syntect::parsing::SyntaxSet;
use tokio::sync::mpsc;

use crate::components::help::HelpState;
use crate::config::AppConfig;
use crate::error::Result;
use crate::fs::clipboard::{ClipboardOp, ClipboardState};
use crate::fs::tree::{NodeType, TreeState};
use crate::preview_content;
use crate::terminal::TerminalState;
use crate::theme::{self, ThemeColors};

/// The kind of dialog being displayed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum DialogKind {
    CreateFile,
    CreateDirectory,
    Rename {
        original: PathBuf,
    },
    DeleteConfirm {
        targets: Vec<PathBuf>,
    },
    Error {
        message: String,
    },
    Progress {
        message: String,
        current: usize,
        total: usize,
    },
}

/// Which panel currently has focus.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum FocusedPanel {
    #[default]
    Tree,
    Preview,
    Terminal,
}

/// View mode for large-file head+tail preview.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ViewMode {
    #[default]
    HeadAndTail,
    HeadOnly,
    TailOnly,
}

/// State for the file preview panel.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct PreviewState {
    /// Path of the file currently being previewed.
    pub current_path: Option<PathBuf>,
    /// Rendered content lines (syntax-highlighted).
    pub content_lines: Vec<Line<'static>>,
    /// Vertical scroll offset (line index of topmost visible line).
    pub scroll_offset: usize,
    /// Current view mode for large files.
    pub view_mode: ViewMode,
    /// Whether long lines wrap.
    pub line_wrap: bool,
    /// Total number of lines in the content.
    pub total_lines: usize,
    /// Whether the current file is in large-file mode.
    pub is_large_file: bool,
    /// Number of head lines to show in head+tail mode.
    pub head_lines: usize,
    /// Number of tail lines to show in head+tail mode.
    pub tail_lines: usize,
}

/// A single fuzzy search result.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SearchResult {
    /// The relative path string.
    pub path: PathBuf,
    /// Display string (relative path from root).
    pub display: String,
    /// Match score from fuzzy-matcher.
    pub score: i64,
    /// Indices of matched characters in the display string.
    pub match_indices: Vec<usize>,
}

/// State for the fuzzy finder overlay (Ctrl+P).
#[derive(Debug, Default)]
pub struct SearchState {
    /// Current search query string.
    pub query: String,
    /// Cursor position within the query.
    pub cursor_position: usize,
    /// Filtered and scored results.
    pub results: Vec<SearchResult>,
    /// Currently selected result index.
    pub selected_index: usize,
    /// Cached file path index (lazily built, invalidated on tree mutations).
    pub cached_paths: Option<Vec<PathBuf>>,
}

/// Application mode.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum AppMode {
    #[default]
    Normal,
    Dialog(DialogKind),
    Search,
    Filter,
    Help,
}

/// State for a dialog's text input.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct DialogState {
    pub input: String,
    pub cursor_position: usize,
}

/// A reversible operation that can be undone.
#[derive(Debug, Clone)]
pub enum UndoAction {
    /// Undo a rename: rename back from `to` to `from`.
    Rename { from: PathBuf, to: PathBuf },
    /// Undo a copy-paste: delete the created paths.
    CopyPaste { created_paths: Vec<PathBuf> },
    /// Undo a move-paste: move files back from `to` to `from`.
    MovePaste { moves: Vec<(PathBuf, PathBuf)> },
}

/// Main application state.
pub struct App {
    /// Merged configuration (CLI + file + defaults).
    pub config: AppConfig,
    /// Resolved theme colors for the UI.
    pub theme_colors: ThemeColors,
    pub tree_state: TreeState,
    pub should_quit: bool,
    #[allow(dead_code)]
    pub mode: AppMode,
    #[allow(dead_code)]
    pub dialog_state: DialogState,
    #[allow(dead_code)]
    pub status_message: Option<(String, Instant)>,
    pub preview_state: PreviewState,
    #[allow(dead_code)]
    pub focused_panel: FocusedPanel,
    pub syntax_set: SyntaxSet,
    pub syntax_theme: Theme,
    /// Tracks which tree index was last previewed, to avoid re-loading on every frame.
    pub last_previewed_index: Option<usize>,
    /// Internal clipboard for copy/cut/paste operations.
    pub clipboard: ClipboardState,
    /// Cancellation token for async operations.
    pub cancel_token: Arc<AtomicBool>,
    /// Last reversible operation (single-level undo).
    pub last_undo: Option<UndoAction>,
    /// State for the fuzzy finder overlay (Ctrl+P).
    pub search_state: SearchState,
    /// Fuzzy matcher instance (reused across searches).
    pub fuzzy_matcher: SkimMatcherV2,
    /// Whether the filesystem watcher is currently active.
    pub watcher_active: bool,
    /// State for the help overlay.
    pub help_state: HelpState,
    /// Last rendered tree panel area (for mouse click mapping).
    pub tree_area: Rect,
    /// Last rendered preview panel area (for mouse click mapping).
    pub preview_area: Rect,
    /// Embedded terminal state (PTY + emulator).
    pub terminal_state: TerminalState,
    /// Last rendered terminal panel area (for mouse click mapping).
    pub terminal_area: Rect,
}

impl App {
    /// Create a new App rooted at the given path, using the provided config.
    pub fn new(path: &Path, config: AppConfig) -> Result<Self> {
        let mut tree_state = TreeState::new(path)?;
        // Apply config: show_hidden
        tree_state.show_hidden = config.show_hidden();
        // Apply config: sort settings
        tree_state.sort_by = crate::fs::tree::SortBy::from_str(config.sort_by());
        tree_state.dirs_first = config.dirs_first();
        tree_state.sort_all_children();
        tree_state.flatten();

        let syntax_set = SyntaxSet::load_defaults_newlines();
        let syntax_theme = preview_content::load_theme(Some(config.syntax_theme_name()));
        let theme_colors = theme::resolve_theme(&config.theme);
        Ok(Self {
            config,
            theme_colors,
            tree_state,
            should_quit: false,
            mode: AppMode::Normal,
            dialog_state: DialogState::default(),
            status_message: None,
            preview_state: PreviewState::default(),
            focused_panel: FocusedPanel::default(),
            syntax_set,
            syntax_theme,
            last_previewed_index: None,
            clipboard: ClipboardState::new(),
            cancel_token: Arc::new(AtomicBool::new(false)),
            last_undo: None,
            search_state: SearchState::default(),
            fuzzy_matcher: SkimMatcherV2::default(),
            watcher_active: true,
            help_state: HelpState::default(),
            tree_area: Rect::default(),
            preview_area: Rect::default(),
            terminal_state: TerminalState::default(),
            terminal_area: Rect::default(),
        })
    }

    /// Open a dialog of the given kind.
    #[allow(dead_code)]
    pub fn open_dialog(&mut self, kind: DialogKind) {
        self.dialog_state = DialogState::default();
        if let DialogKind::Rename { ref original } = kind {
            if let Some(name) = original.file_name() {
                let name = name.to_string_lossy().to_string();
                self.dialog_state.cursor_position = name.len();
                self.dialog_state.input = name;
            }
        }
        self.mode = AppMode::Dialog(kind);
    }

    /// Close the current dialog and return to normal mode.
    #[allow(dead_code)]
    pub fn close_dialog(&mut self) {
        self.mode = AppMode::Normal;
        self.dialog_state = DialogState::default();
    }

    /// Insert a character at the current cursor position.
    #[allow(dead_code)]
    pub fn dialog_input_char(&mut self, c: char) {
        self.dialog_state
            .input
            .insert(self.dialog_state.cursor_position, c);
        self.dialog_state.cursor_position += c.len_utf8();
    }

    /// Delete the character before the cursor (backspace).
    #[allow(dead_code)]
    pub fn dialog_delete_char(&mut self) {
        if self.dialog_state.cursor_position > 0 {
            let byte_pos = self.dialog_state.cursor_position;
            let prev_char = self.dialog_state.input[..byte_pos]
                .chars()
                .next_back()
                .expect("cursor > 0 guarantees at least one char");
            self.dialog_state.cursor_position -= prev_char.len_utf8();
            self.dialog_state
                .input
                .remove(self.dialog_state.cursor_position);
        }
    }

    /// Move cursor left by one character.
    #[allow(dead_code)]
    pub fn dialog_move_cursor_left(&mut self) {
        if self.dialog_state.cursor_position > 0 {
            let prev_char = self.dialog_state.input[..self.dialog_state.cursor_position]
                .chars()
                .next_back()
                .expect("cursor > 0 guarantees at least one char");
            self.dialog_state.cursor_position -= prev_char.len_utf8();
        }
    }

    /// Move cursor right by one character.
    #[allow(dead_code)]
    pub fn dialog_move_cursor_right(&mut self) {
        if self.dialog_state.cursor_position < self.dialog_state.input.len() {
            let next_char = self.dialog_state.input[self.dialog_state.cursor_position..]
                .chars()
                .next()
                .expect("cursor < len guarantees at least one char");
            self.dialog_state.cursor_position += next_char.len_utf8();
        }
    }

    /// Move cursor to the beginning of the input.
    #[allow(dead_code)]
    pub fn dialog_cursor_home(&mut self) {
        self.dialog_state.cursor_position = 0;
    }

    /// Move cursor to the end of the input.
    #[allow(dead_code)]
    pub fn dialog_cursor_end(&mut self) {
        self.dialog_state.cursor_position = self.dialog_state.input.len();
    }

    /// Set a status message with current timestamp.
    #[allow(dead_code)]
    pub fn set_status_message(&mut self, msg: String) {
        self.status_message = Some((msg, Instant::now()));
    }

    /// Clear the status message if it has been displayed for more than 3 seconds.
    #[allow(dead_code)]
    pub fn clear_expired_status(&mut self) {
        if let Some((_, ref created)) = self.status_message {
            if created.elapsed().as_secs() > 3 {
                self.status_message = None;
            }
        }
    }

    /// Get the directory of the currently selected item.
    #[allow(dead_code)]
    pub fn current_dir(&self) -> PathBuf {
        if let Some(item) = self
            .tree_state
            .flat_items
            .get(self.tree_state.selected_index)
        {
            if item.node_type == NodeType::Directory {
                return item.path.clone();
            }
            if let Some(parent) = item.path.parent() {
                return parent.to_path_buf();
            }
        }
        self.tree_state.root.path.clone()
    }

    /// Toggle focus between panels: Tree → Preview → Terminal (if visible) → Tree.
    pub fn toggle_focus(&mut self) {
        self.focused_panel = match self.focused_panel {
            FocusedPanel::Tree => FocusedPanel::Preview,
            FocusedPanel::Preview => {
                if self.terminal_state.visible {
                    FocusedPanel::Terminal
                } else {
                    FocusedPanel::Tree
                }
            }
            FocusedPanel::Terminal => FocusedPanel::Tree,
        };
    }

    /// Toggle the terminal panel visibility. Spawns PTY on first open.
    pub fn toggle_terminal(&mut self, event_tx: &mpsc::UnboundedSender<crate::event::Event>) {
        // Check if terminal is enabled in config
        if !self.config.terminal_enabled() {
            self.set_status_message("Terminal disabled (--no-terminal or config)".to_string());
            return;
        }

        if self.terminal_state.visible {
            // Hide the terminal panel
            self.terminal_state.visible = false;
            // If focus was on terminal, move it to tree
            if self.focused_panel == FocusedPanel::Terminal {
                self.focused_panel = FocusedPanel::Tree;
            }
        } else {
            // Show the terminal panel
            self.terminal_state.visible = true;

            // If no PTY is running (first open or after exit), spawn one
            let needs_spawn = self.terminal_state.pty.is_none()
                || !self
                    .terminal_state
                    .pty
                    .as_ref()
                    .map(|p| p.is_alive())
                    .unwrap_or(false);

            if needs_spawn {
                self.terminal_state.exited = false;
                let cwd = self.current_dir();
                let shell = self.config.terminal_shell();

                // Calculate terminal dimensions from terminal_area
                let rows = self.terminal_area.height.saturating_sub(2).max(1);
                let cols = self.terminal_area.width.saturating_sub(2).max(1);
                // Use defaults if area hasn't been set yet
                let rows = if rows == 0 { 24 } else { rows };
                let cols = if cols == 0 { 80 } else { cols };

                let (pty_tx, mut pty_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

                match crate::terminal::pty::PtyProcess::spawn(&shell, &cwd, rows, cols, pty_tx) {
                    Ok(pty) => {
                        self.terminal_state.pty = Some(pty);
                        self.terminal_state
                            .emulator
                            .resize(rows as usize, cols as usize);

                        // Bridge PTY output to the main event loop
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            while let Some(data) = pty_rx.recv().await {
                                if event_tx
                                    .send(crate::event::Event::TerminalOutput(data))
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        });
                    }
                    Err(e) => {
                        self.set_status_message(format!("⚠ Terminal: {}", e));
                        self.terminal_state.visible = false;
                        return;
                    }
                }
            }

            self.focused_panel = FocusedPanel::Terminal;
        }
    }

    /// Resize the terminal panel upward (smaller terminal, bigger main area).
    pub fn resize_terminal_up(&mut self) {
        if self.terminal_state.visible && self.terminal_state.height_percent > 10 {
            self.terminal_state.height_percent =
                self.terminal_state.height_percent.saturating_sub(5).max(10);
        }
    }

    /// Resize the terminal panel downward (bigger terminal, smaller main area).
    pub fn resize_terminal_down(&mut self) {
        if self.terminal_state.visible && self.terminal_state.height_percent < 80 {
            self.terminal_state.height_percent = (self.terminal_state.height_percent + 5).min(80);
        }
    }

    /// Shut down the terminal PTY process (called on app exit).
    pub fn shutdown_terminal(&mut self) {
        if let Some(ref pty) = self.terminal_state.pty {
            pty.shutdown();
        }
        self.terminal_state.pty = None;
    }

    /// Collect paths for clipboard: multi-selected if any, else focused item.
    fn collect_target_paths(&self) -> Vec<PathBuf> {
        if !self.tree_state.multi_selected.is_empty() {
            self.tree_state
                .multi_selected
                .iter()
                .filter_map(|&idx| self.tree_state.flat_items.get(idx))
                .map(|item| item.path.clone())
                .collect()
        } else if let Some(item) = self
            .tree_state
            .flat_items
            .get(self.tree_state.selected_index)
        {
            vec![item.path.clone()]
        } else {
            vec![]
        }
    }

    /// Copy selected/focused items to clipboard.
    pub fn copy_to_clipboard(&mut self) {
        let paths = self.collect_target_paths();
        if paths.is_empty() {
            return;
        }
        let count = paths.len();
        self.clipboard.set(paths, ClipboardOp::Copy);
        self.set_status_message(format!(
            "📋 {} item{} copied",
            count,
            if count == 1 { "" } else { "s" }
        ));
    }

    /// Cut selected/focused items to clipboard.
    pub fn cut_to_clipboard(&mut self) {
        let paths = self.collect_target_paths();
        if paths.is_empty() {
            return;
        }
        let count = paths.len();
        self.clipboard.set(paths, ClipboardOp::Cut);
        self.set_status_message(format!(
            "✂ {} item{} cut",
            count,
            if count == 1 { "" } else { "s" }
        ));
    }

    /// Paste clipboard contents — async version that spawns a tokio task.
    pub fn paste_clipboard_async(&mut self, event_tx: mpsc::UnboundedSender<crate::event::Event>) {
        use crate::event::{Event, OperationResult, ProgressUpdate};
        use crate::fs::operations;

        if self.clipboard.is_empty() {
            self.set_status_message("Clipboard is empty".to_string());
            return;
        }

        let dest_dir = self.current_dir();
        let op = self.clipboard.operation;
        let paths = self.clipboard.paths.clone();
        let cancel = self.cancel_token.clone();

        // Reset cancel token
        cancel.store(false, Ordering::SeqCst);

        // Show progress dialog
        self.open_dialog(DialogKind::Progress {
            message: "Preparing...".to_string(),
            current: 0,
            total: paths.len(),
        });

        let was_cut = op == Some(ClipboardOp::Cut);

        tokio::spawn(async move {
            let total = paths.len();
            let mut success_count = 0;
            let mut errors = Vec::new();
            let mut created_paths = Vec::new();

            for (i, src) in paths.iter().enumerate() {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }

                let filename = src
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                let _ = event_tx.send(Event::Progress(ProgressUpdate {
                    current_file: filename,
                    current: i + 1,
                    total,
                }));

                let result = match op {
                    Some(ClipboardOp::Copy) => operations::copy_recursive(src, &dest_dir),
                    Some(ClipboardOp::Cut) => operations::move_item(src, &dest_dir),
                    None => continue,
                };

                match result {
                    Ok(created) => {
                        success_count += 1;
                        created_paths.push(created);
                    }
                    Err(e) => errors.push(format!(
                        "{}: {}",
                        src.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default(),
                        e
                    )),
                }
            }

            let _ = event_tx.send(Event::OperationComplete(OperationResult {
                success_count,
                errors,
                created_paths,
                source_paths: paths,
                dest_dir,
                was_cut,
            }));
        });
    }

    /// Handle an async operation completion.
    pub fn handle_operation_complete(&mut self, result: crate::event::OperationResult) {
        self.close_dialog();

        // Refresh dest dir
        self.tree_state.reload_dir(&result.dest_dir);
        self.invalidate_search_cache();

        // For cut/move, also refresh source parents
        if result.was_cut {
            for src in &result.source_paths {
                if let Some(parent) = src.parent() {
                    self.tree_state.reload_dir(parent);
                }
            }
            // Clear clipboard after successful cut
            if result.errors.is_empty() {
                self.clipboard.clear();
            }
        }

        if result.errors.is_empty() {
            // Record undo action
            if result.was_cut {
                // Build move pairs: (original_src, created_dest)
                let moves: Vec<(PathBuf, PathBuf)> = result
                    .source_paths
                    .iter()
                    .zip(result.created_paths.iter())
                    .map(|(src, dest)| (src.clone(), dest.clone()))
                    .collect();
                self.last_undo = Some(UndoAction::MovePaste { moves });
            } else {
                self.last_undo = Some(UndoAction::CopyPaste {
                    created_paths: result.created_paths.clone(),
                });
            }

            let op_name = if result.was_cut { "Moved" } else { "Pasted" };
            self.set_status_message(format!(
                "{} {} item{}",
                op_name,
                result.success_count,
                if result.success_count == 1 { "" } else { "s" }
            ));
        } else {
            self.set_status_message(format!("Error: {}", result.errors.join("; ")));
        }
    }

    /// Handle a progress update from an async operation.
    pub fn handle_progress(&mut self, update: crate::event::ProgressUpdate) {
        if let AppMode::Dialog(DialogKind::Progress { .. }) = &self.mode {
            self.mode = AppMode::Dialog(DialogKind::Progress {
                message: update.current_file,
                current: update.current,
                total: update.total,
            });
        }
    }

    /// Cancel an ongoing async operation.
    pub fn cancel_operation(&mut self) {
        self.cancel_token.store(true, Ordering::SeqCst);
    }

    /// Undo the last reversible operation.
    pub fn undo(&mut self) {
        use crate::fs::operations;

        let action = match self.last_undo.take() {
            Some(a) => a,
            None => {
                self.set_status_message("Nothing to undo".to_string());
                return;
            }
        };

        match action {
            UndoAction::Rename { from, to } => {
                // Rename back: from is original, to is what it was renamed to
                match operations::rename(&to, &from) {
                    Ok(()) => {
                        if let Some(parent) = from.parent() {
                            self.tree_state.reload_dir(parent);
                        }
                        self.set_status_message("Undo: rename reverted".to_string());
                    }
                    Err(e) => self.set_status_message(format!("Undo failed: {}", e)),
                }
            }
            UndoAction::CopyPaste { created_paths } => {
                let mut errors = Vec::new();
                for path in &created_paths {
                    if let Err(e) = operations::delete(path) {
                        errors.push(format!("{}: {}", path.display(), e));
                    } else if let Some(parent) = path.parent() {
                        self.tree_state.reload_dir(parent);
                    }
                }
                if errors.is_empty() {
                    self.set_status_message(format!(
                        "Undo: deleted {} copied item{}",
                        created_paths.len(),
                        if created_paths.len() == 1 { "" } else { "s" }
                    ));
                } else {
                    self.set_status_message(format!("Undo partial: {}", errors.join("; ")));
                }
            }
            UndoAction::MovePaste { moves } => {
                let mut errors = Vec::new();
                for (original_src, current_dest) in &moves {
                    // Move back: current_dest → original_src
                    if let Some(parent) = original_src.parent() {
                        match operations::move_item(current_dest, parent) {
                            Ok(_) => {
                                self.tree_state.reload_dir(parent);
                                if let Some(dest_parent) = current_dest.parent() {
                                    self.tree_state.reload_dir(dest_parent);
                                }
                            }
                            Err(e) => errors.push(format!("{}: {}", current_dest.display(), e)),
                        }
                    }
                }
                if errors.is_empty() {
                    self.set_status_message(format!(
                        "Undo: moved {} item{} back",
                        moves.len(),
                        if moves.len() == 1 { "" } else { "s" }
                    ));
                } else {
                    self.set_status_message(format!("Undo partial: {}", errors.join("; ")));
                }
            }
        }
    }

    /// Scroll preview down by one line.
    pub fn preview_scroll_down(&mut self) {
        let max = self.preview_max_scroll_offset();
        self.preview_state.scroll_offset = (self.preview_state.scroll_offset + 1).min(max);
    }

    /// Scroll preview up by one line.
    pub fn preview_scroll_up(&mut self) {
        self.clamp_preview_scroll();
        if self.preview_state.scroll_offset > 0 {
            self.preview_state.scroll_offset -= 1;
        }
    }

    /// Jump preview to the first line.
    pub fn preview_jump_top(&mut self) {
        self.preview_state.scroll_offset = 0;
    }

    /// Jump preview to the last line.
    pub fn preview_jump_bottom(&mut self) {
        self.preview_state.scroll_offset = self.preview_max_scroll_offset();
    }

    /// Scroll preview down by half a page.
    pub fn preview_half_page_down(&mut self, visible_height: usize) {
        let half = visible_height / 2;
        let max = self.preview_max_scroll_offset();
        self.preview_state.scroll_offset = (self.preview_state.scroll_offset + half).min(max);
    }

    /// Scroll preview up by half a page.
    pub fn preview_half_page_up(&mut self, visible_height: usize) {
        self.clamp_preview_scroll();
        let half = visible_height / 2;
        self.preview_state.scroll_offset = self.preview_state.scroll_offset.saturating_sub(half);
    }

    /// Clamp preview scroll offset to valid bounds for the current viewport.
    pub fn clamp_preview_scroll(&mut self) {
        let max = self.preview_max_scroll_offset();
        self.preview_state.scroll_offset = self.preview_state.scroll_offset.min(max);
    }

    fn preview_max_scroll_offset(&self) -> usize {
        self.preview_line_count()
            .saturating_sub(self.preview_visible_height())
    }

    fn preview_visible_height(&self) -> usize {
        self.preview_area.height.saturating_sub(2).max(1) as usize
    }

    fn preview_line_count(&self) -> usize {
        if self.preview_state.content_lines.is_empty() {
            self.preview_state.total_lines.max(1)
        } else {
            self.preview_state.content_lines.len()
        }
    }

    /// Update preview content when the selected tree item changes.
    pub fn update_preview(&mut self) {
        let idx = self.tree_state.selected_index;
        if self.last_previewed_index == Some(idx) {
            return; // No change
        }
        self.last_previewed_index = Some(idx);

        let item = match self.tree_state.flat_items.get(idx) {
            Some(item) => item,
            None => return,
        };

        // Check if we're reloading the same path (e.g., after FS watcher event).
        // If so, preserve the current scroll offset.
        let same_path = self
            .preview_state
            .current_path
            .as_ref()
            .map(|p| p == &item.path)
            .unwrap_or(false);
        let preserved_scroll = if same_path {
            self.preview_state.scroll_offset
        } else {
            0
        };

        // Only preview files, not directories
        if item.node_type == NodeType::Directory {
            let path = item.path.clone();
            let (lines, total) = preview_content::load_directory_summary(&path);
            self.preview_state = PreviewState {
                current_path: Some(path),
                content_lines: lines,
                scroll_offset: preserved_scroll,
                view_mode: ViewMode::default(),
                line_wrap: false,
                total_lines: total,
                is_large_file: false,
                head_lines: self.config.head_lines(),
                tail_lines: self.config.tail_lines(),
            };
            self.clamp_preview_scroll();
            return;
        }

        if item.node_type != NodeType::File {
            self.preview_state = PreviewState::default();
            return;
        }

        let path = item.path.clone();

        // Check for notebook files
        if path.extension().and_then(|e| e.to_str()) == Some("ipynb") {
            let (lines, total) =
                preview_content::load_notebook_content(&path, &self.syntax_set, &self.syntax_theme);
            self.preview_state = PreviewState {
                current_path: Some(path),
                content_lines: lines,
                scroll_offset: preserved_scroll,
                view_mode: ViewMode::default(),
                line_wrap: false,
                total_lines: total,
                is_large_file: false,
                head_lines: self.config.head_lines(),
                tail_lines: self.config.tail_lines(),
            };
            self.clamp_preview_scroll();
            return;
        }

        // Check if binary file
        if preview_content::is_binary_file(&path) {
            let (lines, total) = preview_content::load_binary_metadata(&path);
            self.preview_state = PreviewState {
                current_path: Some(path),
                content_lines: lines,
                scroll_offset: preserved_scroll,
                view_mode: ViewMode::default(),
                line_wrap: false,
                total_lines: total,
                is_large_file: false,
                head_lines: self.config.head_lines(),
                tail_lines: self.config.tail_lines(),
            };
            self.clamp_preview_scroll();
            return;
        }

        // Check file size for large-file mode (using config values)
        let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let max_preview = self.config.max_full_preview_bytes();
        let head = self.config.head_lines();
        let tail = self.config.tail_lines();

        let is_large = file_size > max_preview;

        if is_large {
            let (lines, total) = preview_content::load_head_tail_content(
                &path,
                &self.syntax_set,
                &self.syntax_theme,
                head,
                tail,
                ViewMode::HeadAndTail,
            );
            self.preview_state = PreviewState {
                current_path: Some(path),
                content_lines: lines,
                scroll_offset: preserved_scroll,
                view_mode: ViewMode::HeadAndTail,
                line_wrap: false,
                total_lines: total,
                is_large_file: true,
                head_lines: head,
                tail_lines: tail,
            };
        } else {
            let (lines, total) = preview_content::load_highlighted_content(
                &path,
                &self.syntax_set,
                &self.syntax_theme,
            );
            self.preview_state = PreviewState {
                current_path: Some(path),
                content_lines: lines,
                scroll_offset: preserved_scroll,
                view_mode: ViewMode::default(),
                line_wrap: false,
                total_lines: total,
                is_large_file: false,
                head_lines: head,
                tail_lines: tail,
            };
        }
        self.clamp_preview_scroll();
    }

    /// Cycle view mode for large file preview (Ctrl+T).
    #[allow(dead_code)]
    pub fn cycle_view_mode(&mut self) {
        if !self.preview_state.is_large_file {
            return;
        }
        self.preview_state.view_mode = match self.preview_state.view_mode {
            ViewMode::HeadAndTail => ViewMode::HeadOnly,
            ViewMode::HeadOnly => ViewMode::TailOnly,
            ViewMode::TailOnly => ViewMode::HeadAndTail,
        };
        self.reload_large_preview();
    }

    /// Adjust head/tail line counts by a delta (+/- keys).
    pub fn adjust_preview_lines(&mut self, delta: isize) {
        if !self.preview_state.is_large_file {
            return;
        }
        let step = delta.unsigned_abs();
        if delta > 0 {
            self.preview_state.head_lines += step;
            self.preview_state.tail_lines += step;
        } else {
            self.preview_state.head_lines =
                self.preview_state.head_lines.saturating_sub(step).max(5);
            self.preview_state.tail_lines =
                self.preview_state.tail_lines.saturating_sub(step).max(5);
        }
        self.reload_large_preview();
    }

    /// Reload the large file preview with current settings.
    fn reload_large_preview(&mut self) {
        if let Some(ref path) = self.preview_state.current_path {
            let path = path.clone();
            let (lines, total) = preview_content::load_head_tail_content(
                &path,
                &self.syntax_set,
                &self.syntax_theme,
                self.preview_state.head_lines,
                self.preview_state.tail_lines,
                self.preview_state.view_mode,
            );
            self.preview_state.content_lines = lines;
            self.preview_state.total_lines = total;
            self.preview_state.scroll_offset = 0;
        }
    }

    /// Quit the application.
    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    /// Move selection down by one item.
    pub fn select_next(&mut self) {
        let len = self.tree_state.flat_items.len();
        if len > 0 && self.tree_state.selected_index < len - 1 {
            self.tree_state.selected_index += 1;
        }
    }

    /// Move selection up by one item.
    pub fn select_previous(&mut self) {
        if self.tree_state.selected_index > 0 {
            self.tree_state.selected_index -= 1;
        }
    }

    /// Jump to the first item.
    pub fn select_first(&mut self) {
        self.tree_state.selected_index = 0;
    }

    /// Jump to the last item.
    pub fn select_last(&mut self) {
        let len = self.tree_state.flat_items.len();
        if len > 0 {
            self.tree_state.selected_index = len - 1;
        }
    }

    /// Expand the selected directory (or no-op on files).
    pub fn expand_selected(&mut self) {
        self.tree_state.expand_selected();
        self.invalidate_search_cache();
    }

    /// Collapse the selected directory, or jump to parent if on a file or collapsed directory.
    pub fn collapse_selected(&mut self) {
        self.tree_state.collapse_selected();
    }

    /// Toggle hidden file visibility.
    pub fn toggle_hidden(&mut self) {
        self.tree_state.toggle_hidden();
        self.invalidate_search_cache();
    }

    // === Search (Ctrl+P) methods ===

    /// Open the fuzzy finder overlay.
    pub fn open_search(&mut self) {
        // Build path index lazily if not cached
        if self.search_state.cached_paths.is_none() {
            self.search_state.cached_paths = Some(self.build_path_index());
        }
        self.search_state.query.clear();
        self.search_state.cursor_position = 0;
        self.search_state.results.clear();
        self.search_state.selected_index = 0;
        self.mode = AppMode::Search;
    }

    /// Close the fuzzy finder overlay without navigating.
    pub fn close_search(&mut self) {
        self.mode = AppMode::Normal;
        // Filesystem events were silently dropped while in Search mode,
        // so invalidate the cache so the next open_search() rebuilds it.
        self.invalidate_search_cache();
        // Force preview refresh in case selection changed externally.
        self.last_previewed_index = None;
    }

    /// Insert a character into the search query and re-score.
    pub fn search_input_char(&mut self, c: char) {
        self.search_state
            .query
            .insert(self.search_state.cursor_position, c);
        self.search_state.cursor_position += c.len_utf8();
        self.update_search_results();
    }

    /// Delete the character before the cursor in the search query.
    pub fn search_delete_char(&mut self) {
        if self.search_state.cursor_position > 0 {
            let byte_pos = self.search_state.cursor_position;
            let prev_char = self.search_state.query[..byte_pos]
                .chars()
                .next_back()
                .expect("cursor > 0 guarantees at least one char");
            self.search_state.cursor_position -= prev_char.len_utf8();
            self.search_state
                .query
                .remove(self.search_state.cursor_position);
            self.update_search_results();
        }
    }

    /// Move search result selection down.
    pub fn search_select_next(&mut self) {
        if !self.search_state.results.is_empty()
            && self.search_state.selected_index < self.search_state.results.len() - 1
        {
            self.search_state.selected_index += 1;
        }
    }

    /// Move search result selection up.
    pub fn search_select_previous(&mut self) {
        if self.search_state.selected_index > 0 {
            self.search_state.selected_index -= 1;
        }
    }

    /// Confirm the selected search result: navigate tree to that path.
    pub fn search_confirm(&mut self) {
        if let Some(result) = self
            .search_state
            .results
            .get(self.search_state.selected_index)
        {
            let path = result.path.clone();
            self.mode = AppMode::Normal;
            self.navigate_to_path(&path);
        }
    }

    /// Update search results by scoring cached paths against the query.
    fn update_search_results(&mut self) {
        let query = &self.search_state.query;
        if query.is_empty() {
            self.search_state.results.clear();
            self.search_state.selected_index = 0;
            return;
        }

        let paths = match &self.search_state.cached_paths {
            Some(p) => p,
            None => return,
        };

        let root = &self.tree_state.root.path;
        let mut results: Vec<SearchResult> = paths
            .iter()
            .filter_map(|path| {
                let display = path
                    .strip_prefix(root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();
                let (score, indices) = self.fuzzy_matcher.fuzzy_indices(&display, query)?;
                Some(SearchResult {
                    path: path.clone(),
                    display,
                    score,
                    match_indices: indices,
                })
            })
            .collect();

        // Sort by score descending
        results.sort_by(|a, b| b.score.cmp(&a.score));
        // Limit to top 50
        results.truncate(50);

        self.search_state.results = results;
        self.search_state.selected_index = 0;
    }

    /// Build a flat list of all file paths by walking the tree recursively.
    /// Uses iterative stack-based walk with 10K entry cap.
    fn build_path_index(&self) -> Vec<PathBuf> {
        const MAX_ENTRIES: usize = 10_000;
        let mut paths = Vec::new();
        let mut stack: Vec<PathBuf> = vec![self.tree_state.root.path.clone()];

        while let Some(dir) = stack.pop() {
            if paths.len() >= MAX_ENTRIES {
                break;
            }
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries {
                if paths.len() >= MAX_ENTRIES {
                    break;
                }
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    paths.push(path);
                }
            }
        }
        paths
    }

    /// Invalidate the cached path index (call after tree mutations).
    pub fn invalidate_search_cache(&mut self) {
        self.search_state.cached_paths = None;
    }

    /// Navigate tree to a specific path: expand all ancestors, select the target.
    pub fn navigate_to_path(&mut self, target: &Path) {
        // Collect ancestor directories that need to be expanded
        let root_path = self.tree_state.root.path.clone();
        let mut ancestors = Vec::new();
        let mut current = target.parent();
        while let Some(p) = current {
            if p == root_path {
                break;
            }
            ancestors.push(p.to_path_buf());
            current = p.parent();
        }
        ancestors.reverse();

        // Clone sort fields before mutable borrow
        let sort_by = self.tree_state.sort_by.clone();
        let dirs_first = self.tree_state.dirs_first;

        // Expand each ancestor and apply sorting
        for ancestor in &ancestors {
            if let Some(node) = TreeState::find_node_mut_pub(&mut self.tree_state.root, ancestor) {
                if !node.is_expanded {
                    let _ = node.load_children();
                    TreeState::sort_children_of_pub(node, &sort_by, dirs_first);
                    node.is_expanded = true;
                }
            }
        }

        // Re-flatten to reflect expansions
        self.tree_state.flatten();

        // Find and select the target in flat_items
        for (i, item) in self.tree_state.flat_items.iter().enumerate() {
            if item.path == target {
                self.tree_state.selected_index = i;
                break;
            }
        }
    }

    // === Filter (/) methods ===

    /// Activate inline tree filter mode.
    pub fn start_filter(&mut self) {
        self.tree_state.filter_query.clear();
        self.tree_state.is_filtering = false;
        self.mode = AppMode::Filter;
    }

    /// Clear the filter and restore the full tree.
    pub fn clear_filter(&mut self) {
        self.tree_state.filter_query.clear();
        self.tree_state.is_filtering = false;
        self.tree_state.flatten();
        self.mode = AppMode::Normal;
        // Filesystem events were silently dropped while in Filter mode,
        // so invalidate the search cache and force preview refresh.
        self.invalidate_search_cache();
        self.last_previewed_index = None;
    }

    /// Accept the current filter and return to normal mode (filtered view stays).
    pub fn accept_filter(&mut self) {
        self.mode = AppMode::Normal;
    }

    /// Insert a character into the filter query and re-filter.
    pub fn filter_input_char(&mut self, c: char) {
        self.tree_state.filter_query.push(c);
        self.tree_state.apply_filter();
    }

    /// Delete the last character from the filter query and re-filter.
    pub fn filter_delete_char(&mut self) {
        self.tree_state.filter_query.pop();
        if self.tree_state.filter_query.is_empty() {
            self.tree_state.is_filtering = false;
            self.tree_state.flatten();
        } else {
            self.tree_state.apply_filter();
        }
    }

    // === Filesystem watcher methods ===

    /// Handle filesystem change events by refreshing affected subtrees.
    ///
    /// Preserves: selected path, scroll offset, expanded directories.
    /// Clears: multi-select, search cache.
    ///
    /// Skipped when in Search or Filter mode to avoid destroying the search
    /// cache or overwriting the filtered flat_items view.
    pub fn handle_fs_change(&mut self, paths: Vec<PathBuf>) {
        // Don't process filesystem changes while search/filter is active:
        // - Search: would invalidate_search_cache(), clearing cached_paths so
        //   fuzzy scoring returns no results.
        // - Filter: would call flatten() which rebuilds flat_items without the
        //   filter, undoing the filtered view.
        if matches!(self.mode, AppMode::Search | AppMode::Filter) {
            return;
        }
        // Capture current state
        let selected_path = self
            .tree_state
            .flat_items
            .get(self.tree_state.selected_index)
            .map(|item| item.path.clone());
        let scroll_offset = self.tree_state.scroll_offset;
        let expanded = self.tree_state.collect_expanded_paths();

        // Deduplicate parent directories to reload
        let mut dirs_to_reload = std::collections::HashSet::new();
        for path in &paths {
            // If the changed path IS the root, do a full reload
            if path == &self.tree_state.root.path {
                dirs_to_reload.clear();
                dirs_to_reload.insert(self.tree_state.root.path.clone());
                break;
            }
            // Otherwise reload the parent directory of the changed file
            if let Some(parent) = path.parent() {
                dirs_to_reload.insert(parent.to_path_buf());
            }
        }

        // Clone sort fields before mutable borrow (avoids borrow checker conflict)
        let sort_by = self.tree_state.sort_by.clone();
        let dirs_first = self.tree_state.dirs_first;

        // Reload each affected directory and apply sorting
        for dir in &dirs_to_reload {
            if let Some(node) =
                crate::fs::tree::TreeState::find_node_mut_pub(&mut self.tree_state.root, dir)
            {
                if node.node_type == crate::fs::tree::NodeType::Directory {
                    let _ = node.load_children();
                    TreeState::sort_children_of_pub(node, &sort_by, dirs_first);
                }
            }
        }

        // Restore expanded directories then re-flatten
        self.tree_state.restore_expanded(&expanded);
        self.tree_state.flatten();

        // Restore selection
        if let Some(ref prev_path) = selected_path {
            if let Some(new_idx) = self.tree_state.find_index_by_path(prev_path) {
                self.tree_state.selected_index = new_idx;
            } else {
                // Selected path was deleted — find nearest surviving
                if let Some(fallback) = self.tree_state.find_nearest_surviving(prev_path) {
                    self.tree_state.selected_index = fallback;
                }
            }
        }

        // Restore scroll offset (clamped)
        let max_scroll = self.tree_state.flat_items.len().saturating_sub(1);
        self.tree_state.scroll_offset = scroll_offset.min(max_scroll);

        // Invalidate caches
        self.invalidate_search_cache();
        // Force preview refresh
        self.last_previewed_index = None;
    }

    /// Force a full tree refresh from root, preserving state.
    ///
    /// Used by F5 keybinding; works regardless of watcher state.
    pub fn full_refresh(&mut self) {
        self.handle_fs_change(vec![self.tree_state.root.path.clone()]);
        self.set_status_message("🔄 Tree refreshed".to_string());
    }

    /// Toggle the filesystem watcher active state.
    ///
    /// Returns the new state (true = active, false = paused).
    pub fn toggle_watcher(&mut self) -> bool {
        self.watcher_active = !self.watcher_active;
        if self.watcher_active {
            self.set_status_message("👁 Watcher resumed".to_string());
        } else {
            self.set_status_message("⏸ Watcher paused".to_string());
        }
        self.watcher_active
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::TempDir;

    fn setup_app() -> (TempDir, App) {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("alpha")).unwrap();
        fs::create_dir(dir.path().join("beta")).unwrap();
        File::create(dir.path().join("file_a.txt")).unwrap();
        File::create(dir.path().join("file_b.rs")).unwrap();
        File::create(dir.path().join(".hidden")).unwrap();
        let app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        (dir, app)
    }

    #[test]
    fn select_next_moves_down() {
        let (_dir, mut app) = setup_app();
        assert_eq!(app.tree_state.selected_index, 0);
        app.select_next();
        assert_eq!(app.tree_state.selected_index, 1);
    }

    #[test]
    fn select_next_clamps_at_end() {
        let (_dir, mut app) = setup_app();
        let last = app.tree_state.flat_items.len() - 1;
        app.tree_state.selected_index = last;
        app.select_next();
        assert_eq!(app.tree_state.selected_index, last);
    }

    #[test]
    fn select_previous_moves_up() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 2;
        app.select_previous();
        assert_eq!(app.tree_state.selected_index, 1);
    }

    #[test]
    fn select_previous_clamps_at_start() {
        let (_dir, mut app) = setup_app();
        app.select_previous();
        assert_eq!(app.tree_state.selected_index, 0);
    }

    #[test]
    fn select_first_and_last() {
        let (_dir, mut app) = setup_app();
        app.select_last();
        assert_eq!(
            app.tree_state.selected_index,
            app.tree_state.flat_items.len() - 1
        );
        app.select_first();
        assert_eq!(app.tree_state.selected_index, 0);
    }

    #[test]
    fn toggle_hidden_changes_count() {
        let (_dir, mut app) = setup_app();
        let without_hidden = app.tree_state.flat_items.len();
        app.toggle_hidden();
        let with_hidden = app.tree_state.flat_items.len();
        assert!(with_hidden > without_hidden);
    }

    #[test]
    fn expand_directory() {
        let (_dir, mut app) = setup_app();
        // Select first child (should be a directory: "alpha")
        app.select_next();
        assert_eq!(app.tree_state.flat_items[1].name, "alpha");
        app.expand_selected();
        // alpha is empty so flat items count stays same, but it's now expanded
        assert!(app.tree_state.flat_items[1].is_expanded);
    }

    #[test]
    fn quit_sets_flag() {
        let (_dir, mut app) = setup_app();
        assert!(!app.should_quit);
        app.quit();
        assert!(app.should_quit);
    }

    #[test]
    fn open_dialog_sets_mode() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        assert_eq!(app.mode, AppMode::Dialog(DialogKind::CreateFile));
    }

    #[test]
    fn close_dialog_returns_to_normal() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateDirectory);
        app.close_dialog();
        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.dialog_state.input.is_empty());
        assert_eq!(app.dialog_state.cursor_position, 0);
    }

    #[test]
    fn dialog_input_char_inserts() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        app.dialog_input_char('a');
        app.dialog_input_char('b');
        app.dialog_input_char('c');
        assert_eq!(app.dialog_state.input, "abc");
        assert_eq!(app.dialog_state.cursor_position, 3);
    }

    #[test]
    fn dialog_delete_char_removes() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        app.dialog_input_char('a');
        app.dialog_input_char('b');
        app.dialog_delete_char();
        assert_eq!(app.dialog_state.input, "a");
        assert_eq!(app.dialog_state.cursor_position, 1);
    }

    #[test]
    fn dialog_delete_char_at_start_is_noop() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        app.dialog_delete_char();
        assert!(app.dialog_state.input.is_empty());
        assert_eq!(app.dialog_state.cursor_position, 0);
    }

    #[test]
    fn dialog_cursor_left_right() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        app.dialog_input_char('a');
        app.dialog_input_char('b');
        app.dialog_move_cursor_left();
        assert_eq!(app.dialog_state.cursor_position, 1);
        app.dialog_move_cursor_right();
        assert_eq!(app.dialog_state.cursor_position, 2);
    }

    #[test]
    fn dialog_cursor_boundaries() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        app.dialog_move_cursor_left();
        assert_eq!(app.dialog_state.cursor_position, 0);
        app.dialog_input_char('x');
        app.dialog_move_cursor_right();
        assert_eq!(app.dialog_state.cursor_position, 1);
    }

    #[test]
    fn dialog_cursor_home_end() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        app.dialog_input_char('a');
        app.dialog_input_char('b');
        app.dialog_input_char('c');
        app.dialog_cursor_home();
        assert_eq!(app.dialog_state.cursor_position, 0);
        app.dialog_cursor_end();
        assert_eq!(app.dialog_state.cursor_position, 3);
    }

    #[test]
    fn rename_prefills_input() {
        let (_dir, mut app) = setup_app();
        let path = PathBuf::from("/some/dir/hello.txt");
        app.open_dialog(DialogKind::Rename { original: path });
        assert_eq!(app.dialog_state.input, "hello.txt");
        assert_eq!(app.dialog_state.cursor_position, 9);
    }

    #[test]
    fn set_status_message_stores_message() {
        let (_dir, mut app) = setup_app();
        app.set_status_message("test message".to_string());
        assert!(app.status_message.is_some());
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert_eq!(msg, "test message");
    }

    #[test]
    fn clear_expired_status_keeps_recent() {
        let (_dir, mut app) = setup_app();
        app.set_status_message("fresh".to_string());
        app.clear_expired_status();
        assert!(app.status_message.is_some());
    }

    #[test]
    fn clear_expired_status_removes_old() {
        let (_dir, mut app) = setup_app();
        app.status_message = Some((
            "old".to_string(),
            Instant::now() - std::time::Duration::from_secs(5),
        ));
        app.clear_expired_status();
        assert!(app.status_message.is_none());
    }

    #[test]
    fn current_dir_returns_root_for_directory() {
        let (dir, app) = setup_app();
        // selected_index 0 is root, which is a directory
        assert_eq!(app.current_dir(), dir.path().to_path_buf());
    }

    #[test]
    fn current_dir_returns_parent_for_file() {
        let (dir, mut app) = setup_app();
        // Navigate to a file (files come after directories in flat_items)
        // flat_items: root(dir), alpha(dir), beta(dir), file_a.txt, file_b.rs
        app.tree_state.selected_index = 3; // file_a.txt
        assert_eq!(app.current_dir(), dir.path().to_path_buf());
    }

    // === Preview state tests ===

    #[test]
    fn default_focused_panel_is_tree() {
        let (_dir, app) = setup_app();
        assert_eq!(app.focused_panel, FocusedPanel::Tree);
    }

    #[test]
    fn toggle_focus_switches_panel() {
        let (_dir, mut app) = setup_app();
        assert_eq!(app.focused_panel, FocusedPanel::Tree);
        app.toggle_focus();
        assert_eq!(app.focused_panel, FocusedPanel::Preview);
        app.toggle_focus();
        assert_eq!(app.focused_panel, FocusedPanel::Tree);
    }

    #[test]
    fn preview_state_defaults() {
        let (_dir, app) = setup_app();
        assert!(app.preview_state.current_path.is_none());
        assert!(app.preview_state.content_lines.is_empty());
        assert_eq!(app.preview_state.scroll_offset, 0);
        assert_eq!(app.preview_state.view_mode, ViewMode::HeadAndTail);
        assert!(!app.preview_state.line_wrap);
        assert_eq!(app.preview_state.total_lines, 0);
    }

    #[test]
    fn preview_scroll_down_up() {
        let (_dir, mut app) = setup_app();
        app.preview_state.total_lines = 100;
        app.preview_scroll_down();
        assert_eq!(app.preview_state.scroll_offset, 1);
        app.preview_scroll_down();
        assert_eq!(app.preview_state.scroll_offset, 2);
        app.preview_scroll_up();
        assert_eq!(app.preview_state.scroll_offset, 1);
    }

    #[test]
    fn preview_scroll_clamps_at_boundaries() {
        let (_dir, mut app) = setup_app();
        app.preview_state.total_lines = 3;
        // Can't scroll past end
        app.preview_state.scroll_offset = 2;
        app.preview_scroll_down();
        assert_eq!(app.preview_state.scroll_offset, 2);
        // Can't scroll before start
        app.preview_state.scroll_offset = 0;
        app.preview_scroll_up();
        assert_eq!(app.preview_state.scroll_offset, 0);
    }

    #[test]
    fn preview_scroll_down_noop_when_empty() {
        let (_dir, mut app) = setup_app();
        app.preview_state.total_lines = 0;
        app.preview_scroll_down();
        assert_eq!(app.preview_state.scroll_offset, 0);
    }

    #[test]
    fn preview_jump_top_bottom() {
        let (_dir, mut app) = setup_app();
        app.preview_state.total_lines = 100;
        app.preview_jump_bottom();
        assert_eq!(app.preview_state.scroll_offset, 99);
        app.preview_jump_top();
        assert_eq!(app.preview_state.scroll_offset, 0);
    }

    #[test]
    fn preview_half_page_scroll() {
        let (_dir, mut app) = setup_app();
        app.preview_state.total_lines = 100;
        app.preview_half_page_down(20);
        assert_eq!(app.preview_state.scroll_offset, 10);
        app.preview_half_page_down(20);
        assert_eq!(app.preview_state.scroll_offset, 20);
        app.preview_half_page_up(20);
        assert_eq!(app.preview_state.scroll_offset, 10);
    }

    #[test]
    fn preview_half_page_clamps() {
        let (_dir, mut app) = setup_app();
        app.preview_state.total_lines = 10;
        app.preview_half_page_down(100);
        assert_eq!(app.preview_state.scroll_offset, 9);
        app.preview_half_page_up(100);
        assert_eq!(app.preview_state.scroll_offset, 0);
    }

    #[test]
    fn preview_jump_bottom_respects_viewport_height() {
        let (_dir, mut app) = setup_app();
        app.preview_state.content_lines =
            (0..100).map(|i| Line::from(format!("line {i}"))).collect();
        app.preview_state.total_lines = 100;
        app.preview_area = Rect::new(0, 0, 80, 12); // inner height = 10
        app.preview_jump_bottom();
        assert_eq!(app.preview_state.scroll_offset, 90);
    }

    #[test]
    fn clamp_preview_scroll_after_resize() {
        let (_dir, mut app) = setup_app();
        app.preview_state.content_lines =
            (0..100).map(|i| Line::from(format!("line {i}"))).collect();
        app.preview_state.total_lines = 100;
        app.preview_area = Rect::new(0, 0, 80, 12); // inner height = 10
        app.preview_jump_bottom();
        assert_eq!(app.preview_state.scroll_offset, 90);

        app.preview_area = Rect::new(0, 0, 80, 22); // inner height = 20
        app.clamp_preview_scroll();
        assert_eq!(app.preview_state.scroll_offset, 80);
    }

    // === Integration tests: preview update flow ===

    #[test]
    fn update_preview_loads_file_content() {
        let (dir, mut app) = setup_app();
        // Write content to a file
        std::fs::write(dir.path().join("file_a.txt"), "hello world\n").unwrap();
        // Select file_a.txt (index 3)
        app.tree_state.selected_index = 3;
        app.update_preview();
        assert!(app.preview_state.current_path.is_some());
        assert!(!app.preview_state.content_lines.is_empty());
        assert!(app.preview_state.total_lines >= 1);
    }

    #[test]
    fn update_preview_directory_shows_summary() {
        let (_dir, mut app) = setup_app();
        // Select alpha directory (index 1)
        app.tree_state.selected_index = 1;
        app.update_preview();
        assert!(app.preview_state.current_path.is_some());
        let all_text: String = app
            .preview_state
            .content_lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("Directory:"));
    }

    #[test]
    fn update_preview_binary_file_shows_metadata() {
        let (dir, mut app) = setup_app();
        // Create a binary file
        let bin_path = dir.path().join("model.pt");
        std::fs::write(&bin_path, &[0u8; 100]).unwrap();
        app.tree_state.reload_dir(dir.path());

        // Find the .pt file in flat_items
        let idx = app
            .tree_state
            .flat_items
            .iter()
            .position(|item| item.name == "model.pt")
            .unwrap();
        app.tree_state.selected_index = idx;
        app.update_preview();

        let all_text: String = app
            .preview_state
            .content_lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("Binary file"));
        assert!(all_text.contains("model.pt"));
    }

    #[test]
    fn update_preview_notebook_file() {
        let (dir, mut app) = setup_app();
        let nb_path = dir.path().join("test.ipynb");
        std::fs::write(
            &nb_path,
            r#"{"cells":[{"cell_type":"code","source":["x=1"],"outputs":[]}],"metadata":{}}"#,
        )
        .unwrap();
        app.tree_state.reload_dir(dir.path());

        let idx = app
            .tree_state
            .flat_items
            .iter()
            .position(|item| item.name == "test.ipynb")
            .unwrap();
        app.tree_state.selected_index = idx;
        app.update_preview();

        let all_text: String = app
            .preview_state
            .content_lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("Cell 1"));
        assert!(all_text.contains("code"));
    }

    #[test]
    fn update_preview_resets_scroll_on_selection_change() {
        let (dir, mut app) = setup_app();
        std::fs::write(dir.path().join("file_a.txt"), "line1\nline2\nline3\n").unwrap();
        // Select file
        app.tree_state.selected_index = 3;
        app.update_preview();
        app.preview_state.scroll_offset = 2;
        // Change selection
        app.tree_state.selected_index = 1; // directory
        app.last_previewed_index = None; // force update
        app.update_preview();
        assert_eq!(app.preview_state.scroll_offset, 0);
    }

    #[test]
    fn update_preview_skips_if_same_selection() {
        let (dir, mut app) = setup_app();
        std::fs::write(dir.path().join("file_a.txt"), "hello\n").unwrap();
        app.tree_state.selected_index = 3;
        app.update_preview();
        let first_path = app.preview_state.current_path.clone();
        // Call again without changing selection
        app.preview_state.scroll_offset = 5;
        app.update_preview();
        // Should not reset scroll
        assert_eq!(app.preview_state.scroll_offset, 5);
        assert_eq!(app.preview_state.current_path, first_path);
    }

    // === Search (Ctrl+P) tests ===

    #[test]
    fn open_search_sets_mode() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        assert_eq!(app.mode, AppMode::Search);
        assert!(app.search_state.cached_paths.is_some());
    }

    #[test]
    fn close_search_returns_to_normal() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.close_search();
        assert_eq!(app.mode, AppMode::Normal);
    }

    #[test]
    fn search_input_updates_query() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.search_input_char('f');
        assert_eq!(app.search_state.query, "f");
        app.search_input_char('i');
        assert_eq!(app.search_state.query, "fi");
    }

    #[test]
    fn search_delete_char_removes() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.search_input_char('a');
        app.search_input_char('b');
        app.search_delete_char();
        assert_eq!(app.search_state.query, "a");
    }

    #[test]
    fn search_delete_at_empty_is_noop() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.search_delete_char();
        assert_eq!(app.search_state.query, "");
    }

    #[test]
    fn search_results_update_on_input() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.search_input_char('f');
        app.search_input_char('i');
        app.search_input_char('l');
        app.search_input_char('e');
        // Should find file_a.txt and file_b.rs
        assert!(app.search_state.results.len() >= 2);
    }

    #[test]
    fn search_empty_query_clears_results() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.search_input_char('a');
        assert!(!app.search_state.results.is_empty());
        app.search_delete_char();
        assert!(app.search_state.results.is_empty());
    }

    #[test]
    fn search_no_matches_empty_results() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.search_input_char('z');
        app.search_input_char('z');
        app.search_input_char('z');
        assert!(app.search_state.results.is_empty());
    }

    #[test]
    fn search_select_navigation() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.search_input_char('f');
        assert_eq!(app.search_state.selected_index, 0);
        app.search_select_next();
        assert_eq!(app.search_state.selected_index, 1);
        app.search_select_previous();
        assert_eq!(app.search_state.selected_index, 0);
    }

    #[test]
    fn search_select_clamps() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.search_select_previous(); // at 0, should stay
        assert_eq!(app.search_state.selected_index, 0);
    }

    #[test]
    fn search_confirm_navigates_to_file() {
        let (dir, mut app) = setup_app();
        // Create a nested file for navigation test
        fs::create_dir_all(dir.path().join("alpha").join("nested")).unwrap();
        File::create(dir.path().join("alpha").join("nested").join("deep.txt")).unwrap();
        app.tree_state.reload_dir(dir.path());
        app.invalidate_search_cache();

        app.open_search();
        app.search_input_char('d');
        app.search_input_char('e');
        app.search_input_char('e');
        app.search_input_char('p');

        assert!(!app.search_state.results.is_empty());
        app.search_confirm();
        assert_eq!(app.mode, AppMode::Normal);

        // Should have navigated to the deep.txt file
        let selected = &app.tree_state.flat_items[app.tree_state.selected_index];
        assert_eq!(selected.name, "deep.txt");
    }

    #[test]
    fn build_path_index_finds_files() {
        let (_dir, app) = setup_app();
        let index = app.build_path_index();
        // Should find file_a.txt, file_b.rs, .hidden
        assert!(index.len() >= 2);
    }

    #[test]
    fn invalidate_search_cache_clears() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        assert!(app.search_state.cached_paths.is_some());
        app.invalidate_search_cache();
        assert!(app.search_state.cached_paths.is_none());
    }

    // === Filter (/) tests ===

    #[test]
    fn start_filter_sets_mode() {
        let (_dir, mut app) = setup_app();
        app.start_filter();
        assert_eq!(app.mode, AppMode::Filter);
    }

    #[test]
    fn filter_input_filters_tree() {
        let (_dir, mut app) = setup_app();
        app.start_filter();
        let total_before = app.tree_state.flat_items.len();
        app.filter_input_char('a');
        // Should show fewer items (only matching + ancestors)
        assert!(app.tree_state.flat_items.len() <= total_before);
        assert!(app.tree_state.is_filtering);
    }

    #[test]
    fn filter_preserves_parent_dirs() {
        let (dir, mut app) = setup_app();
        // Create inner.txt inside alpha
        File::create(dir.path().join("alpha").join("inner.txt")).unwrap();
        // Expand alpha so its children are loaded
        app.tree_state.selected_index = 1; // alpha dir
        app.expand_selected();

        app.start_filter();
        app.filter_input_char('i');
        app.filter_input_char('n');
        app.filter_input_char('n');
        app.filter_input_char('e');
        app.filter_input_char('r');

        // "alpha" directory should be preserved as parent of "inner.txt"
        let names: Vec<&str> = app
            .tree_state
            .flat_items
            .iter()
            .map(|i| i.name.as_str())
            .collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"inner.txt"));
    }

    #[test]
    fn clear_filter_restores_tree() {
        let (_dir, mut app) = setup_app();
        let original_count = app.tree_state.flat_items.len();
        app.start_filter();
        app.filter_input_char('x');
        app.clear_filter();
        assert_eq!(app.mode, AppMode::Normal);
        assert!(!app.tree_state.is_filtering);
        assert_eq!(app.tree_state.flat_items.len(), original_count);
    }

    #[test]
    fn accept_filter_keeps_filtered_view() {
        let (_dir, mut app) = setup_app();
        app.start_filter();
        app.filter_input_char('f');
        let filtered_count = app.tree_state.flat_items.len();
        app.accept_filter();
        assert_eq!(app.mode, AppMode::Normal);
        // Filtered view should persist
        assert_eq!(app.tree_state.flat_items.len(), filtered_count);
    }

    #[test]
    fn filter_backspace_updates_filter() {
        let (_dir, mut app) = setup_app();
        let original_count = app.tree_state.flat_items.len();
        app.start_filter();
        app.filter_input_char('z');
        app.filter_input_char('z');
        app.filter_delete_char();
        app.filter_delete_char();
        // Should restore full tree when filter query becomes empty
        assert!(!app.tree_state.is_filtering);
        assert_eq!(app.tree_state.flat_items.len(), original_count);
    }

    #[test]
    fn filter_case_insensitive() {
        let (_dir, mut app) = setup_app();
        app.start_filter();
        app.filter_input_char('F');
        app.filter_input_char('I');
        app.filter_input_char('L');
        app.filter_input_char('E');
        // Should match "file_a.txt" and "file_b.rs" despite uppercase query
        let names: Vec<&str> = app
            .tree_state
            .flat_items
            .iter()
            .map(|i| i.name.as_str())
            .collect();
        assert!(names.contains(&"file_a.txt"));
        assert!(names.contains(&"file_b.rs"));
    }

    #[test]
    fn navigate_to_path_expands_ancestors() {
        let (dir, mut app) = setup_app();
        // Create nested structure
        fs::create_dir_all(dir.path().join("alpha").join("nested")).unwrap();
        File::create(dir.path().join("alpha").join("nested").join("target.txt")).unwrap();
        app.tree_state.reload_dir(dir.path());

        let target = dir.path().join("alpha").join("nested").join("target.txt");
        app.navigate_to_path(&target);

        let selected = &app.tree_state.flat_items[app.tree_state.selected_index];
        assert_eq!(selected.name, "target.txt");
    }

    // === Filesystem watcher tests ===

    #[test]
    fn handle_fs_change_detects_new_file() {
        let (dir, mut app) = setup_app();
        let original_count = app.tree_state.flat_items.len();
        // Create a new file externally
        File::create(dir.path().join("new_file.txt")).unwrap();
        // Simulate watcher event
        app.handle_fs_change(vec![dir.path().join("new_file.txt")]);
        assert!(app.tree_state.flat_items.len() > original_count);
        let names: Vec<&str> = app
            .tree_state
            .flat_items
            .iter()
            .map(|i| i.name.as_str())
            .collect();
        assert!(names.contains(&"new_file.txt"));
    }

    #[test]
    fn handle_fs_change_preserves_selection() {
        let (dir, mut app) = setup_app();
        // Select "file_a.txt"
        let file_a_idx = app
            .tree_state
            .flat_items
            .iter()
            .position(|i| i.name == "file_a.txt")
            .unwrap();
        app.tree_state.selected_index = file_a_idx;

        // Create a new file externally, trigger refresh
        File::create(dir.path().join("zzz_newfile.txt")).unwrap();
        app.handle_fs_change(vec![dir.path().join("zzz_newfile.txt")]);

        // Selection should still point to file_a.txt
        let selected = &app.tree_state.flat_items[app.tree_state.selected_index];
        assert_eq!(selected.name, "file_a.txt");
    }

    #[test]
    fn handle_fs_change_selection_fallback_on_delete() {
        let (dir, mut app) = setup_app();
        // Select "file_a.txt"
        let file_a_idx = app
            .tree_state
            .flat_items
            .iter()
            .position(|i| i.name == "file_a.txt")
            .unwrap();
        app.tree_state.selected_index = file_a_idx;

        // Delete file_a.txt externally
        fs::remove_file(dir.path().join("file_a.txt")).unwrap();
        app.handle_fs_change(vec![dir.path().join("file_a.txt")]);

        // Selection should have moved to a valid index
        assert!(app.tree_state.selected_index < app.tree_state.flat_items.len());
    }

    #[test]
    fn handle_fs_change_preserves_expanded_dirs() {
        let (dir, mut app) = setup_app();
        // Expand "alpha" directory
        let alpha_idx = app
            .tree_state
            .flat_items
            .iter()
            .position(|i| i.name == "alpha")
            .unwrap();
        app.tree_state.selected_index = alpha_idx;
        app.expand_selected();
        let count_after_expand = app.tree_state.flat_items.len();

        // Create a file in root, trigger refresh
        File::create(dir.path().join("extra.txt")).unwrap();
        app.handle_fs_change(vec![dir.path().join("extra.txt")]);

        // alpha should still be expanded (count increased by 1 for new file)
        assert!(app.tree_state.flat_items.len() > count_after_expand);
        let alpha_item = app
            .tree_state
            .flat_items
            .iter()
            .find(|i| i.name == "alpha")
            .unwrap();
        assert!(alpha_item.is_expanded);
    }

    #[test]
    fn handle_fs_change_invalidates_search_cache() {
        let (dir, mut app) = setup_app();
        // Build search cache by opening and closing,
        // then rebuild it manually since close_search invalidates.
        app.open_search();
        app.close_search();
        // Rebuild the cache after close.
        app.search_state.cached_paths = Some(app.build_path_index());
        assert!(app.search_state.cached_paths.is_some());

        // Trigger fs change in Normal mode
        File::create(dir.path().join("cache_buster.txt")).unwrap();
        app.handle_fs_change(vec![dir.path().join("cache_buster.txt")]);
        assert!(app.search_state.cached_paths.is_none());
    }

    #[test]
    fn fs_change_skipped_during_search_mode() {
        let (_dir, mut app) = setup_app();
        // Open search — builds path cache and sets mode to Search
        app.open_search();
        assert!(app.search_state.cached_paths.is_some());
        assert_eq!(app.mode, AppMode::Search);

        // While in Search mode, fs change should be silently ignored
        app.handle_fs_change(vec![app.tree_state.root.path.clone()]);
        // Cache must NOT be invalidated while searching
        assert!(
            app.search_state.cached_paths.is_some(),
            "search cache should survive fs events during search mode"
        );
    }

    #[test]
    fn fs_change_skipped_during_filter_mode() {
        let (_dir, mut app) = setup_app();
        app.start_filter();
        app.filter_input_char('f');
        let filtered_count = app.tree_state.flat_items.len();
        assert_eq!(app.mode, AppMode::Filter);

        // While in Filter mode, fs change should be silently ignored
        app.handle_fs_change(vec![app.tree_state.root.path.clone()]);
        // flat_items should still be the filtered set, not the full tree
        assert_eq!(
            app.tree_state.flat_items.len(),
            filtered_count,
            "filtered view should survive fs events during filter mode"
        );
    }

    #[test]
    fn fs_change_works_after_closing_search() {
        let (dir, mut app) = setup_app();
        // Open and close search to return to Normal mode
        app.open_search();
        app.close_search();
        assert_eq!(app.mode, AppMode::Normal);

        // Now fs change should process normally
        let original_count = app.tree_state.flat_items.len();
        File::create(dir.path().join("new_file.txt")).unwrap();
        app.handle_fs_change(vec![dir.path().join("new_file.txt")]);
        assert!(app.tree_state.flat_items.len() > original_count);
    }

    #[test]
    fn full_refresh_reloads_tree() {
        let (dir, mut app) = setup_app();
        let original_count = app.tree_state.flat_items.len();
        // Create a file externally
        File::create(dir.path().join("f5_file.txt")).unwrap();
        app.full_refresh();
        assert!(app.tree_state.flat_items.len() > original_count);
        assert!(app.status_message.is_some());
    }

    #[test]
    fn toggle_watcher_flips_state() {
        let (_dir, mut app) = setup_app();
        assert!(app.watcher_active); // default on
        let result = app.toggle_watcher();
        assert!(!result);
        assert!(!app.watcher_active);
        let result2 = app.toggle_watcher();
        assert!(result2);
        assert!(app.watcher_active);
    }

    #[test]
    fn toggle_watcher_sets_status_message() {
        let (_dir, mut app) = setup_app();
        app.toggle_watcher();
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(msg.contains("paused") || msg.contains("⏸"));
        app.toggle_watcher();
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(msg.contains("resumed") || msg.contains("👁"));
    }

    #[test]
    fn handle_fs_change_preserves_sort_order() {
        let (dir, mut app) = setup_app();

        // Verify initial sort order: dirs first (alpha, beta), then files (file_a.txt, file_b.rs)
        // flat_items[0] = root, [1] = alpha, [2] = beta, [3] = file_a.txt, [4] = file_b.rs
        assert_eq!(app.tree_state.flat_items[1].name, "alpha");
        assert_eq!(app.tree_state.flat_items[2].name, "beta");
        assert_eq!(app.tree_state.flat_items[3].name, "file_a.txt");
        assert_eq!(app.tree_state.flat_items[4].name, "file_b.rs");

        // Create a new file externally to trigger a change
        File::create(dir.path().join("aaa_new.txt")).unwrap();
        fs::create_dir(dir.path().join("gamma")).unwrap();
        app.handle_fs_change(vec![
            dir.path().join("aaa_new.txt"),
            dir.path().join("gamma"),
        ]);

        // After fs change, dirs must still appear first, alphabetically sorted
        let names: Vec<&str> = app
            .tree_state
            .flat_items
            .iter()
            .skip(1) // skip root
            .map(|item| item.name.as_str())
            .collect();

        // Find the boundary between dirs and files
        let dir_count = names
            .iter()
            .take_while(|n| {
                app.tree_state
                    .flat_items
                    .iter()
                    .find(|i| i.name == **n)
                    .map(|i| i.node_type == crate::fs::tree::NodeType::Directory)
                    .unwrap_or(false)
            })
            .count();

        // All directories should come first
        assert!(
            dir_count >= 3,
            "Expected at least 3 dirs (alpha, beta, gamma), got {dir_count}"
        );

        // Directories should be alphabetically sorted
        let dir_names: Vec<&str> = names[..dir_count].to_vec();
        assert_eq!(dir_names, vec!["alpha", "beta", "gamma"]);

        // Files should be alphabetically sorted
        let file_names: Vec<&str> = names[dir_count..].to_vec();
        let mut expected_files = file_names.clone();
        expected_files.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        assert_eq!(
            file_names, expected_files,
            "Files should be alphabetically sorted"
        );
    }

    #[test]
    fn navigate_to_path_preserves_sort_order() {
        let (dir, mut app) = setup_app();

        // Create nested structure: alpha/z_file.txt, alpha/a_file.txt, alpha/nested_dir/
        fs::create_dir_all(dir.path().join("alpha").join("nested_dir")).unwrap();
        File::create(dir.path().join("alpha").join("z_file.txt")).unwrap();
        File::create(dir.path().join("alpha").join("a_file.txt")).unwrap();

        // Navigate to a nested file — this forces alpha to expand
        let target = dir.path().join("alpha").join("a_file.txt");
        app.navigate_to_path(&target);

        // Find alpha's children in the flat list
        let alpha_children: Vec<&str> = app
            .tree_state
            .flat_items
            .iter()
            .filter(|i| i.depth == 2) // alpha's children are at depth 2
            .map(|i| i.name.as_str())
            .collect();

        // nested_dir (directory) should come first, then a_file, z_file (alphabetical)
        assert!(
            !alpha_children.is_empty(),
            "Alpha should have children after navigate_to_path"
        );
        assert_eq!(
            alpha_children[0], "nested_dir",
            "Directory should come first"
        );
    }

    #[test]
    fn preview_scroll_with_content_and_area() {
        let (dir, mut app) = setup_app();
        // Write a file with many lines
        let content: String = (0..200).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(dir.path().join("file_a.txt"), &content).unwrap();

        // Simulate a real terminal preview area (height=30, visible=28)
        app.preview_area = Rect::new(40, 0, 80, 30);

        // Select file_a.txt
        app.tree_state.selected_index = 3;
        app.update_preview();

        let content_lines_len = app.preview_state.content_lines.len();
        let visible_height = app.preview_area.height.saturating_sub(2) as usize;
        let expected_max = content_lines_len.saturating_sub(visible_height);

        // Scroll down 50 times, simulating render cycle each time
        for _ in 0..50 {
            app.preview_scroll_down();
            app.update_preview();
            app.clamp_preview_scroll();
        }

        assert_eq!(
            app.preview_state.scroll_offset,
            50.min(expected_max),
            "After scrolling 50 times, offset should be 50 (or max if less)"
        );
    }

    #[test]
    fn preview_scroll_preserved_after_fs_change() {
        let (dir, mut app) = setup_app();
        // Write a file with many lines
        let content: String = (0..200).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(dir.path().join("file_a.txt"), &content).unwrap();

        // Simulate a real terminal preview area
        app.preview_area = Rect::new(40, 0, 80, 30);

        // Select file_a.txt and update preview
        app.tree_state.selected_index = 3;
        app.update_preview();

        // Scroll down to position 25
        for _ in 0..25 {
            app.preview_scroll_down();
        }
        assert_eq!(app.preview_state.scroll_offset, 25);

        // Simulate a file watcher event (unrelated file change)
        app.handle_fs_change(vec![dir.path().join("file_b.rs")]);

        // The next render cycle calls update_preview + clamp
        app.update_preview();
        app.clamp_preview_scroll();

        // Scroll offset should be preserved since we're still viewing the same file
        assert_eq!(
            app.preview_state.scroll_offset, 25,
            "Scroll offset should be preserved after FS change event for same file"
        );
    }

    #[test]
    fn preview_scroll_resets_on_different_file() {
        let (dir, mut app) = setup_app();
        let content: String = (0..200).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(dir.path().join("file_a.txt"), &content).unwrap();
        std::fs::write(dir.path().join("file_b.rs"), "fn main() {}\n").unwrap();

        app.preview_area = Rect::new(40, 0, 80, 30);
        app.tree_state.selected_index = 3; // file_a.txt
        app.update_preview();

        // Scroll down
        for _ in 0..25 {
            app.preview_scroll_down();
        }
        assert_eq!(app.preview_state.scroll_offset, 25);

        // Switch to a different file
        app.tree_state.selected_index = 4; // file_b.rs
        app.last_previewed_index = None;
        app.update_preview();

        // Scroll should reset to 0 for different file
        assert_eq!(
            app.preview_state.scroll_offset, 0,
            "Scroll should reset when switching to a different file"
        );
    }
}
