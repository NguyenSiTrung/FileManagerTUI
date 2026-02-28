//! Terminal panel widget for rendering the embedded terminal emulator output.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Widget};

use crate::terminal::TerminalState;
use crate::theme::ThemeColors;

/// Widget that renders the terminal emulator output.
pub struct TerminalWidget<'a> {
    state: &'a TerminalState,
    theme: &'a ThemeColors,
    block: Option<Block<'a>>,
    show_cursor: bool,
}

impl<'a> TerminalWidget<'a> {
    pub fn new(state: &'a TerminalState, theme: &'a ThemeColors, show_cursor: bool) -> Self {
        Self {
            state,
            theme,
            block: None,
            show_cursor,
        }
    }

    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }
}

impl<'a> Widget for TerminalWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Render block border if present
        let inner = if let Some(ref block) = self.block {
            let inner = block.inner(area);
            block.clone().render(area, buf);
            inner
        } else {
            area
        };

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        if self.state.exited {
            // Show exited message in center
            let msg = "[Process exited - press Ctrl+T to restart]";
            let y = inner.y + inner.height / 2;
            let x = inner.x + inner.width.saturating_sub(msg.len() as u16) / 2;
            let style = Style::default()
                .fg(self.theme.dim_fg)
                .add_modifier(Modifier::DIM);
            buf.set_string(x, y, msg, style);
            return;
        }

        // Get rendered lines from emulator
        let lines = self.state.render_lines(self.theme);
        let (cursor_row, cursor_col) = if self.state.pty.is_some() {
            self.state.emulator.cursor_position()
        } else {
            (0, 0)
        };

        // Render each line
        for (row_idx, line) in lines.iter().enumerate() {
            if row_idx >= inner.height as usize {
                break;
            }
            let y = inner.y + row_idx as u16;

            for (col_idx, span) in line.spans.iter().enumerate() {
                if col_idx >= inner.width as usize {
                    break;
                }
                let x = inner.x + col_idx as u16;
                buf.set_string(x, y, &span.content, span.style);
            }
        }

        // Render cursor if focused
        if self.show_cursor {
            let cursor_y = inner.y + cursor_row as u16;
            let cursor_x = inner.x + cursor_col as u16;
            if cursor_x < inner.x + inner.width && cursor_y < inner.y + inner.height {
                // Invert the cell at cursor position
                let cell = buf.cell_mut((cursor_x, cursor_y));
                if let Some(cell) = cell {
                    let cursor_fg = self.theme.border_focused_fg;
                    cell.set_style(
                        Style::default()
                            .fg(Color::Black)
                            .bg(cursor_fg)
                            .add_modifier(Modifier::BOLD),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::TerminalState;
    use crate::theme;

    #[test]
    fn test_terminal_widget_renders() {
        let mut state = TerminalState::default();
        state.emulator.process(b"Hello World");
        let theme = theme::dark_theme();

        let widget = TerminalWidget::new(&state, &theme, false);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        // First row should contain "Hello World"
        let content: String = (0..11)
            .map(|x| {
                buf.cell((x, 0))
                    .map(|c| c.symbol().chars().next().unwrap_or(' '))
                    .unwrap_or(' ')
            })
            .collect();
        assert_eq!(content, "Hello World");
    }

    #[test]
    fn test_terminal_widget_exited() {
        let mut state = TerminalState::default();
        state.exited = true;
        let theme = theme::dark_theme();

        let widget = TerminalWidget::new(&state, &theme, false);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        // Should render the exited message somewhere in the middle
    }
}
