//! Embedded terminal panel: PTY process management, terminal emulation, and state.

pub mod emulator;
pub mod pty;

use ratatui::text::Line;

use crate::theme::ThemeColors;

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
}
