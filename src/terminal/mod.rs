//! Embedded terminal panel: PTY process management, terminal emulation, and state.

pub mod emulator;
pub mod pty;

use ratatui::text::Line;

use crate::theme::ThemeColors;

/// A terminal selection anchor/endpoint coordinate in terminal-local space.
/// `line` is 0-based relative to the combined buffer: scrollback lines first,
/// then visible grid lines. `col` is 0-based column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCoord {
    pub line: usize,
    pub col: usize,
}

/// Terminal text selection state.
#[derive(Debug, Clone, Default)]
pub struct TerminalSelection {
    /// The anchor point where the selection started (mouse-down position).
    pub anchor: Option<TerminalCoord>,
    /// The moving endpoint of the selection (current mouse position during drag).
    pub endpoint: Option<TerminalCoord>,
}

impl TerminalSelection {
    /// Clear the selection entirely.
    pub fn clear(&mut self) {
        self.anchor = None;
        self.endpoint = None;
    }

    /// Set the anchor (start of selection).
    pub fn set_anchor(&mut self, coord: TerminalCoord) {
        self.anchor = Some(coord);
        self.endpoint = Some(coord);
    }

    /// Update the moving endpoint.
    pub fn set_endpoint(&mut self, coord: TerminalCoord) {
        self.endpoint = Some(coord);
    }

    /// Returns true if a selection is active (both anchor and endpoint set).
    pub fn is_active(&self) -> bool {
        self.anchor.is_some() && self.endpoint.is_some()
    }

    /// Get selection range normalized so start <= end.
    /// Returns (start, end) where start.line < end.line, or
    /// start.line == end.line && start.col <= end.col.
    pub fn normalized(&self) -> Option<(TerminalCoord, TerminalCoord)> {
        match (self.anchor, self.endpoint) {
            (Some(a), Some(b)) => {
                if a.line < b.line || (a.line == b.line && a.col <= b.col) {
                    Some((a, b))
                } else {
                    Some((b, a))
                }
            }
            _ => None,
        }
    }
}

/// Overall state for the embedded terminal panel.
pub struct TerminalState {
    /// The terminal emulator (screen buffer + ANSI parser).
    pub emulator: emulator::TerminalEmulator,
    /// The PTY child process (None if not yet spawned or exited).
    pub pty: Option<pty::PtyProcess>,
    /// Whether the terminal panel is visible.
    pub visible: bool,
    /// Terminal panel height as a percentage of screen height (default 30).
    pub height_percent: u16,
    /// Scrollback scroll offset (0 = at bottom / live).
    pub scroll_offset: usize,
    /// Whether the shell process has exited.
    pub exited: bool,
    /// Current mouse text selection.
    pub selection: TerminalSelection,
}

impl Default for TerminalState {
    fn default() -> Self {
        Self {
            emulator: emulator::TerminalEmulator::new(24, 80),
            pty: None,
            visible: false,
            height_percent: 30,
            scroll_offset: 0,
            exited: false,
            selection: TerminalSelection::default(),
        }
    }
}

impl std::fmt::Debug for TerminalState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalState")
            .field("visible", &self.visible)
            .field("height_percent", &self.height_percent)
            .field("scroll_offset", &self.scroll_offset)
            .field("exited", &self.exited)
            .field("pty_active", &self.pty.is_some())
            .field("selection_active", &self.selection.is_active())
            .finish()
    }
}

impl TerminalState {
    /// Get rendered lines from the emulator for display.
    pub fn render_lines(&self, _theme: &ThemeColors) -> Vec<Line<'static>> {
        self.emulator.render_lines()
    }

    /// Total number of lines (visible screen + scrollback).
    #[allow(dead_code)]
    pub fn total_lines(&self) -> usize {
        self.emulator.total_lines()
    }

    /// Extract selected text from the terminal emulator's grid + scrollback.
    /// Returns None if no selection is active.
    pub fn extract_selected_text(&self) -> Option<String> {
        let (start, end) = self.selection.normalized()?;
        self.emulator.extract_text(start.line, start.col, end.line, end.col)
    }

    /// Clear the terminal selection.
    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_coord_default() {
        let sel = TerminalSelection::default();
        assert!(!sel.is_active());
        assert!(sel.normalized().is_none());
    }

    #[test]
    fn test_selection_set_and_clear() {
        let mut sel = TerminalSelection::default();
        sel.set_anchor(TerminalCoord { line: 0, col: 5 });
        assert!(sel.is_active());
        sel.set_endpoint(TerminalCoord { line: 2, col: 3 });
        assert!(sel.is_active());

        let (start, end) = sel.normalized().unwrap();
        assert_eq!(start.line, 0);
        assert_eq!(start.col, 5);
        assert_eq!(end.line, 2);
        assert_eq!(end.col, 3);

        sel.clear();
        assert!(!sel.is_active());
    }

    #[test]
    fn test_selection_backward_normalization() {
        let mut sel = TerminalSelection::default();
        // Backward drag: endpoint before anchor
        sel.set_anchor(TerminalCoord { line: 5, col: 10 });
        sel.set_endpoint(TerminalCoord { line: 2, col: 3 });

        let (start, end) = sel.normalized().unwrap();
        assert_eq!(start.line, 2);
        assert_eq!(start.col, 3);
        assert_eq!(end.line, 5);
        assert_eq!(end.col, 10);
    }

    #[test]
    fn test_selection_same_line_backward() {
        let mut sel = TerminalSelection::default();
        sel.set_anchor(TerminalCoord { line: 3, col: 15 });
        sel.set_endpoint(TerminalCoord { line: 3, col: 5 });

        let (start, end) = sel.normalized().unwrap();
        assert_eq!(start.line, 3);
        assert_eq!(start.col, 5);
        assert_eq!(end.line, 3);
        assert_eq!(end.col, 15);
    }

    #[test]
    fn test_extract_selected_text_no_selection() {
        let state = TerminalState::default();
        assert!(state.extract_selected_text().is_none());
    }

    #[test]
    fn test_extract_selected_text_single_line() {
        let mut state = TerminalState::default();
        // Write some content (scrollback_len=0, so line 0 = grid row 0)
        state.emulator.process(b"Hello World");
        let scrollback_len = state.emulator.scrollback_len();

        state.selection.set_anchor(TerminalCoord {
            line: scrollback_len,
            col: 0,
        });
        state.selection.set_endpoint(TerminalCoord {
            line: scrollback_len,
            col: 4,
        });

        let text = state.extract_selected_text().unwrap();
        assert_eq!(text, "Hello");
    }

    #[test]
    fn test_extract_selected_text_multi_line() {
        let mut state = TerminalState::default();
        state.emulator.process(b"Line 1\r\nLine 2\r\nLine 3");
        let scrollback_len = state.emulator.scrollback_len();

        state.selection.set_anchor(TerminalCoord {
            line: scrollback_len,
            col: 0,
        });
        state.selection.set_endpoint(TerminalCoord {
            line: scrollback_len + 2,
            col: 5,
        });

        let text = state.extract_selected_text().unwrap();
        // Should contain all three lines with trailing spaces trimmed
        assert!(text.contains("Line 1"));
        assert!(text.contains("Line 2"));
        assert!(text.contains("Line 3"));
    }
}
