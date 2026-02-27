use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use ratatui::text::Line;
use syntect::highlighting::Theme;
use syntect::parsing::SyntaxSet;
use tokio::sync::mpsc;

use crate::error::Result;
use crate::fs::clipboard::{ClipboardOp, ClipboardState};
use crate::fs::tree::{NodeType, TreeState};
use crate::preview_content;

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

/// Application mode.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum AppMode {
    #[default]
    Normal,
    Dialog(DialogKind),
}

/// State for a dialog's text input.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct DialogState {
    pub input: String,
    pub cursor_position: usize,
}

/// Main application state.
pub struct App {
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
}

impl App {
    /// Create a new App rooted at the given path.
    pub fn new(path: &Path) -> Result<Self> {
        let tree_state = TreeState::new(path)?;
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let syntax_theme = preview_content::load_theme(None);
        Ok(Self {
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

    /// Toggle focus between tree and preview panels.
    pub fn toggle_focus(&mut self) {
        self.focused_panel = match self.focused_panel {
            FocusedPanel::Tree => FocusedPanel::Preview,
            FocusedPanel::Preview => FocusedPanel::Tree,
        };
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

    /// Scroll preview down by one line.
    pub fn preview_scroll_down(&mut self) {
        if self.preview_state.scroll_offset < self.preview_state.total_lines.saturating_sub(1) {
            self.preview_state.scroll_offset += 1;
        }
    }

    /// Scroll preview up by one line.
    pub fn preview_scroll_up(&mut self) {
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
        self.preview_state.scroll_offset = self.preview_state.total_lines.saturating_sub(1);
    }

    /// Scroll preview down by half a page.
    pub fn preview_half_page_down(&mut self, visible_height: usize) {
        let half = visible_height / 2;
        let max = self.preview_state.total_lines.saturating_sub(1);
        self.preview_state.scroll_offset = (self.preview_state.scroll_offset + half).min(max);
    }

    /// Scroll preview up by half a page.
    pub fn preview_half_page_up(&mut self, visible_height: usize) {
        let half = visible_height / 2;
        self.preview_state.scroll_offset = self.preview_state.scroll_offset.saturating_sub(half);
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

        // Only preview files, not directories
        if item.node_type == NodeType::Directory {
            let path = item.path.clone();
            let (lines, total) = preview_content::load_directory_summary(&path);
            self.preview_state = PreviewState {
                current_path: Some(path),
                content_lines: lines,
                scroll_offset: 0,
                view_mode: ViewMode::default(),
                line_wrap: false,
                total_lines: total,
                is_large_file: false,
                head_lines: preview_content::DEFAULT_HEAD_LINES,
                tail_lines: preview_content::DEFAULT_TAIL_LINES,
            };
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
                scroll_offset: 0,
                view_mode: ViewMode::default(),
                line_wrap: false,
                total_lines: total,
                is_large_file: false,
                head_lines: preview_content::DEFAULT_HEAD_LINES,
                tail_lines: preview_content::DEFAULT_TAIL_LINES,
            };
            return;
        }

        // Check if binary file
        if preview_content::is_binary_file(&path) {
            let (lines, total) = preview_content::load_binary_metadata(&path);
            self.preview_state = PreviewState {
                current_path: Some(path),
                content_lines: lines,
                scroll_offset: 0,
                view_mode: ViewMode::default(),
                line_wrap: false,
                total_lines: total,
                is_large_file: false,
                head_lines: preview_content::DEFAULT_HEAD_LINES,
                tail_lines: preview_content::DEFAULT_TAIL_LINES,
            };
            return;
        }

        // Check file size for large-file mode
        let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

        let is_large = file_size > preview_content::DEFAULT_MAX_FULL_PREVIEW_BYTES;

        if is_large {
            let head = preview_content::DEFAULT_HEAD_LINES;
            let tail = preview_content::DEFAULT_TAIL_LINES;
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
                scroll_offset: 0,
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
                scroll_offset: 0,
                view_mode: ViewMode::default(),
                line_wrap: false,
                total_lines: total,
                is_large_file: false,
                head_lines: preview_content::DEFAULT_HEAD_LINES,
                tail_lines: preview_content::DEFAULT_TAIL_LINES,
            };
        }
    }

    /// Cycle view mode for large file preview (Ctrl+T).
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
    }

    /// Collapse the selected directory, or jump to parent if on a file or collapsed directory.
    pub fn collapse_selected(&mut self) {
        self.tree_state.collapse_selected();
    }

    /// Toggle hidden file visibility.
    pub fn toggle_hidden(&mut self) {
        self.tree_state.toggle_hidden();
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
        let app = App::new(dir.path()).unwrap();
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
}
