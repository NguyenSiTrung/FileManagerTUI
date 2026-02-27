use std::path::Path;

use crate::error::Result;
use crate::fs::tree::TreeState;

/// Application mode.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    #[default]
    Normal,
}

/// Main application state.
pub struct App {
    pub tree_state: TreeState,
    pub should_quit: bool,
    #[allow(dead_code)]
    pub mode: AppMode,
}

impl App {
    /// Create a new App rooted at the given path.
    pub fn new(path: &Path) -> Result<Self> {
        let tree_state = TreeState::new(path)?;
        Ok(Self {
            tree_state,
            should_quit: false,
            mode: AppMode::Normal,
        })
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
}
