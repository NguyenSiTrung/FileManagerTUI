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
    /// True while a mouse drag selection is in progress.
    pub dragging: bool,
}

impl TerminalSelection {
    /// Clear the selection entirely.
    pub fn clear(&mut self) {
        self.anchor = None;
        self.endpoint = None;
        self.dragging = false;
    }

    /// Set the anchor (start of selection).
    pub fn set_anchor(&mut self, coord: TerminalCoord) {
        self.anchor = Some(coord);
        self.endpoint = Some(coord);
        self.dragging = false;
    }

    /// Start a mouse drag selection from the given anchor.
    pub fn begin_drag(&mut self, coord: TerminalCoord) {
        self.set_anchor(coord);
        self.dragging = true;
    }

    /// Update the moving endpoint.
    pub fn set_endpoint(&mut self, coord: TerminalCoord) {
        self.endpoint = Some(coord);
    }

    /// Finish an active drag gesture.
    pub fn end_drag(&mut self) {
        self.dragging = false;
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
            scroll_offset: 0,
            exited: false,
            selection: TerminalSelection::default(),
        }
    }
}

impl std::fmt::Debug for TerminalState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalState")
            .field("scroll_offset", &self.scroll_offset)
            .field("exited", &self.exited)
            .field("pty_active", &self.pty.is_some())
            .field("selection_active", &self.selection.is_active())
            .finish()
    }
}

impl TerminalState {
    /// Apply bounded history capacity without spawning or restarting a shell.
    pub fn set_scrollback_limit(&mut self, limit: usize) {
        if self.emulator.scrollback_limit() == limit.min(emulator::MAX_SCROLLBACK_LINES) {
            self.scroll_offset = self.scroll_offset.min(self.emulator.scrollback_len());
            return;
        }
        let removed = self.emulator.set_scrollback_limit(limit);
        self.scroll_offset = self.scroll_offset.min(self.emulator.scrollback_len());
        if removed > 0 {
            if self.selection.anchor.is_some_and(|p| p.line < removed)
                || self.selection.endpoint.is_some_and(|p| p.line < removed)
            {
                self.selection.clear();
            } else {
                for point in [&mut self.selection.anchor, &mut self.selection.endpoint]
                    .into_iter()
                    .flatten()
                {
                    point.line -= removed;
                }
            }
        }
    }

    /// Get rendered lines from the emulator for display,
    /// accounting for the current scroll offset.
    pub fn render_lines(&self, _theme: &ThemeColors) -> Vec<Line<'static>> {
        self.emulator.render_lines_at_offset(self.scroll_offset)
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
        self.emulator
            .extract_text(start.line, start.col, end.line, end.col)
    }

    /// Clear the terminal selection.
    #[allow(dead_code)]
    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task3_history_limit_zero_shrink_grow_rebases_only_surviving_selection() {
        let mut state = TerminalState {
            emulator: emulator::TerminalEmulator::new(2, 8),
            ..Default::default()
        };
        state
            .emulator
            .process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
        let history = state.emulator.scrollback_len();
        assert!(history > 1);
        let cursor = state.emulator.cursor_position();
        let grid: Vec<_> = state
            .emulator
            .render_lines()
            .iter()
            .map(crate::text::line_text)
            .collect();
        state.selection.begin_drag(TerminalCoord {
            line: history,
            col: 0,
        });
        state.selection.set_endpoint(TerminalCoord {
            line: history + 1,
            col: 3,
        });
        let selected = state.extract_selected_text();
        state.scroll_offset = history;
        state.exited = true;
        state.set_scrollback_limit(1);
        assert_eq!(state.emulator.scrollback_limit(), 1);
        assert_eq!(state.selection.anchor.unwrap().line, 1);
        assert_eq!(state.extract_selected_text(), selected);
        assert_eq!(state.scroll_offset, 1);
        assert_eq!(state.emulator.cursor_position(), cursor);
        assert_eq!(
            state
                .emulator
                .render_lines()
                .iter()
                .map(crate::text::line_text)
                .collect::<Vec<_>>(),
            grid
        );
        assert!(state.exited && state.pty.is_none());
        state.set_scrollback_limit(usize::MAX);
        assert_eq!(
            state.emulator.scrollback_limit(),
            emulator::MAX_SCROLLBACK_LINES
        );
        assert_eq!(state.emulator.scrollback_len(), 1);
        state
            .selection
            .set_anchor(TerminalCoord { line: 0, col: 0 });
        state.set_scrollback_limit(0);
        assert!(!state.selection.is_active());
        state.emulator.process(&b"next\r\n".repeat(10));
        assert_eq!(state.emulator.scrollback_len(), 0);
        state.set_scrollback_limit(2);
        state.emulator.process(&b"new\r\n".repeat(10));
        assert_eq!(state.emulator.scrollback_len(), 2);
        state.set_scrollback_limit(2);
        assert_eq!(state.emulator.scrollback_len(), 2);
    }

    #[test]
    fn task3_shrink_clears_selection_when_only_endpoint_is_removed() {
        let mut state = TerminalState {
            emulator: emulator::TerminalEmulator::new(2, 8),
            ..Default::default()
        };
        state.emulator.process(&b"text\r\n".repeat(8));
        state.selection.set_anchor(TerminalCoord {
            line: state.emulator.total_lines() - 1,
            col: 1,
        });
        state
            .selection
            .set_endpoint(TerminalCoord { line: 0, col: 0 });
        state.set_scrollback_limit(1);
        assert!(state.selection.anchor.is_none() && state.selection.endpoint.is_none());
    }

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
