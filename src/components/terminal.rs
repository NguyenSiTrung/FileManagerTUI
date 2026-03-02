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
            let msg = "[Process exited - press t or Ctrl+T to restart]";
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

        // Compute selection range in absolute coordinates
        let sel_range = self.state.selection.normalized();

        // Compute the first absolute line index currently visible on screen.
        // scroll_offset=0 → showing the last `visible_rows` lines (live view).
        let total = self.state.emulator.total_lines();
        let visible = self.state.emulator.visible_rows();
        let first_abs = total.saturating_sub(visible + self.state.scroll_offset);

        // Selection highlight style — uses editor_selection_bg for consistency
        let sel_style = Style::default()
            .bg(self.theme.editor_selection_bg)
            .fg(self.theme.tree_fg);

        // Render each line
        for (row_idx, line) in lines.iter().enumerate() {
            if row_idx >= inner.height as usize {
                break;
            }
            let y = inner.y + row_idx as u16;
            let abs_line = first_abs + row_idx;

            for (col_idx, span) in line.spans.iter().enumerate() {
                if col_idx >= inner.width as usize {
                    break;
                }
                let x = inner.x + col_idx as u16;

                // Determine if this cell is in the selection range
                let in_selection = if let Some((start, end)) = sel_range {
                    if abs_line > start.line && abs_line < end.line {
                        true // Entire line is selected
                    } else if abs_line == start.line && abs_line == end.line {
                        col_idx >= start.col && col_idx <= end.col
                    } else if abs_line == start.line {
                        col_idx >= start.col
                    } else if abs_line == end.line {
                        col_idx <= end.col
                    } else {
                        false
                    }
                } else {
                    false
                };

                if in_selection {
                    buf.set_string(x, y, &span.content, sel_style);
                } else {
                    buf.set_string(x, y, &span.content, span.style);
                }
            }
        }

        // Render cursor if focused (cursor is not occluded by selection)
        if self.show_cursor && self.state.scroll_offset == 0 {
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
    use crate::terminal::{TerminalCoord, TerminalState};
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

    #[test]
    fn test_terminal_widget_selection_highlight() {
        let mut state = TerminalState::default();
        state.emulator.process(b"Hello World");
        let sb = state.emulator.scrollback_len();

        // Select "World" (cols 6-10)
        state.selection.set_anchor(TerminalCoord {
            line: sb,
            col: 6,
        });
        state.selection.set_endpoint(TerminalCoord {
            line: sb,
            col: 10,
        });

        let theme = theme::dark_theme();
        let widget = TerminalWidget::new(&state, &theme, false);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        // Cell at col 6 (W) should have selection bg
        let cell_w = buf.cell((6, 0)).unwrap();
        assert_eq!(cell_w.bg, theme.editor_selection_bg);

        // Cell at col 0 (H) should NOT have selection bg
        let cell_h = buf.cell((0, 0)).unwrap();
        assert_ne!(cell_h.bg, theme.editor_selection_bg);
    }

    #[test]
    fn test_terminal_widget_no_selection_no_highlight() {
        let mut state = TerminalState::default();
        state.emulator.process(b"Hello");
        let theme = theme::dark_theme();

        let widget = TerminalWidget::new(&state, &theme, false);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        // No cells should have selection bg
        let cell = buf.cell((0, 0)).unwrap();
        assert_ne!(cell.bg, theme.editor_selection_bg);
    }
}
