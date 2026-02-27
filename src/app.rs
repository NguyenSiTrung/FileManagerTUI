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
}
