use std::path::PathBuf;

/// The type of clipboard operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardOp {
    Copy,
    Cut,
}

/// Internal clipboard buffer holding file paths and operation type.
#[derive(Debug, Clone)]
pub struct ClipboardState {
    pub paths: Vec<PathBuf>,
    pub operation: Option<ClipboardOp>,
}

impl Default for ClipboardState {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardState {
    /// Create a new empty clipboard.
    pub fn new() -> Self {
        Self {
            paths: Vec::new(),
            operation: None,
        }
    }

    /// Set the clipboard with paths and operation type.
    pub fn set(&mut self, paths: Vec<PathBuf>, op: ClipboardOp) {
        self.paths = paths;
        self.operation = Some(op);
    }

    /// Clear the clipboard.
    pub fn clear(&mut self) {
        self.paths.clear();
        self.operation = None;
    }

    /// Whether the clipboard has content.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Number of items in the clipboard.
    pub fn len(&self) -> usize {
        self.paths.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_clipboard_is_empty() {
        let cb = ClipboardState::new();
        assert!(cb.is_empty());
        assert_eq!(cb.len(), 0);
        assert_eq!(cb.operation, None);
    }

    #[test]
    fn default_clipboard_is_empty() {
        let cb = ClipboardState::default();
        assert!(cb.is_empty());
    }

    #[test]
    fn set_copy_operation() {
        let mut cb = ClipboardState::new();
        cb.set(
            vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.txt")],
            ClipboardOp::Copy,
        );
        assert!(!cb.is_empty());
        assert_eq!(cb.len(), 2);
        assert_eq!(cb.operation, Some(ClipboardOp::Copy));
        assert_eq!(cb.paths[0], PathBuf::from("/tmp/a.txt"));
        assert_eq!(cb.paths[1], PathBuf::from("/tmp/b.txt"));
    }

    #[test]
    fn set_cut_operation() {
        let mut cb = ClipboardState::new();
        cb.set(vec![PathBuf::from("/tmp/file.rs")], ClipboardOp::Cut);
        assert_eq!(cb.operation, Some(ClipboardOp::Cut));
        assert_eq!(cb.len(), 1);
    }

    #[test]
    fn clear_resets_clipboard() {
        let mut cb = ClipboardState::new();
        cb.set(vec![PathBuf::from("/tmp/a.txt")], ClipboardOp::Copy);
        assert!(!cb.is_empty());
        cb.clear();
        assert!(cb.is_empty());
        assert_eq!(cb.operation, None);
    }

    #[test]
    fn set_overwrites_previous() {
        let mut cb = ClipboardState::new();
        cb.set(vec![PathBuf::from("/tmp/old.txt")], ClipboardOp::Copy);
        cb.set(vec![PathBuf::from("/tmp/new.txt")], ClipboardOp::Cut);
        assert_eq!(cb.len(), 1);
        assert_eq!(cb.operation, Some(ClipboardOp::Cut));
        assert_eq!(cb.paths[0], PathBuf::from("/tmp/new.txt"));
    }

    #[test]
    fn set_with_empty_paths() {
        let mut cb = ClipboardState::new();
        cb.set(vec![], ClipboardOp::Copy);
        assert!(cb.is_empty());
        assert_eq!(cb.operation, Some(ClipboardOp::Copy));
    }
}
