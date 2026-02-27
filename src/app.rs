use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::error::Result;
use crate::fs::tree::{NodeType, TreeState};

/// The kind of dialog being displayed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum DialogKind {
    CreateFile,
    CreateDirectory,
    Rename { original: PathBuf },
    DeleteConfirm { targets: Vec<PathBuf> },
    Error { message: String },
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
}

impl App {
    /// Create a new App rooted at the given path.
    pub fn new(path: &Path) -> Result<Self> {
        let tree_state = TreeState::new(path)?;
        Ok(Self {
            tree_state,
            should_quit: false,
            mode: AppMode::Normal,
            dialog_state: DialogState::default(),
            status_message: None,
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
}
