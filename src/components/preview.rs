use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Widget},
};

use crate::app::PreviewState;
use crate::terminal::TerminalSelection;
use crate::theme::ThemeColors;

/// Preview widget that renders file content in the preview panel.
#[allow(dead_code)]
pub struct PreviewWidget<'a> {
    preview_state: &'a PreviewState,
    selection: Option<&'a TerminalSelection>,
    theme: &'a ThemeColors,
    block: Option<Block<'a>>,
}

impl<'a> PreviewWidget<'a> {
    #[allow(dead_code)]
    pub fn new(preview_state: &'a PreviewState, theme: &'a ThemeColors) -> Self {
        Self {
            preview_state,
            selection: None,
            theme,
            block: None,
        }
    }

    pub fn selection(mut self, selection: &'a TerminalSelection) -> Self {
        self.selection = Some(selection);
        self
    }

    #[allow(dead_code)]
    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = block.into();
        self
    }
}

impl<'a> Widget for PreviewWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Render block (border) first, get inner area
        let inner = if let Some(block) = self.block {
            let inner = block.inner(area);
            block.render(area, buf);
            inner
        } else {
            area
        };

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        if self.preview_state.content_lines.is_empty() {
            // Show placeholder text
            let msg = "No preview";
            let line = Line::from(Span::styled(msg, Style::default().fg(self.theme.dim_fg)));
            buf.set_line(inner.x, inner.y, &line, inner.width);
            return;
        }

        // Render visible lines starting from scroll_offset
        let visible_height = inner.height as usize;
        let max_start = self
            .preview_state
            .content_lines
            .len()
            .saturating_sub(visible_height);
        let start = self.preview_state.scroll_offset.min(max_start);
        let end = (start + visible_height).min(self.preview_state.content_lines.len());

        for (i, line) in self.preview_state.content_lines[start..end]
            .iter()
            .enumerate()
        {
            let y = inner.y + i as u16;
            buf.set_line(inner.x, y, line, inner.width);
        }

        let Some(selection) = self.selection else {
            return;
        };
        let Some((sel_start, sel_end)) = selection.normalized() else {
            return;
        };
        if start >= end || inner.width == 0 {
            return;
        }

        let first_visible = start;
        let last_visible = end - 1;
        if sel_end.line < first_visible || sel_start.line > last_visible {
            return;
        }

        let highlight_style = Style::default()
            .bg(self.theme.editor_selection_bg)
            .fg(self.theme.preview_fg);
        let first_line = sel_start.line.max(first_visible);
        let last_line = sel_end.line.min(last_visible);
        let max_col = inner.width.saturating_sub(1) as usize;

        for abs_line in first_line..=last_line {
            let row = inner.y + (abs_line - first_visible) as u16;
            let start_col = if abs_line == sel_start.line {
                sel_start.col
            } else {
                0
            };
            let end_col = if abs_line == sel_end.line {
                sel_end.col
            } else {
                max_col
            };
            if start_col > end_col || start_col > max_col {
                continue;
            }

            for col in start_col.min(max_col)..=end_col.min(max_col) {
                if let Some(cell) = buf.cell_mut((inner.x + col as u16, row)) {
                    cell.set_style(highlight_style);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::{TerminalCoord, TerminalSelection};
    use crate::theme;
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Borders};

    fn test_theme() -> ThemeColors {
        theme::dark_theme()
    }

    #[test]
    fn test_empty_preview_shows_placeholder() {
        let state = PreviewState::default();
        let tc = test_theme();
        let widget = PreviewWidget::new(&state, &tc)
            .block(Block::default().borders(Borders::ALL).title(" Preview "));
        let area = Rect::new(0, 0, 30, 5);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        // The inner area should contain "No preview"
        let content: String = (0..30)
            .map(|x| {
                buf.cell((x, 1))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        assert!(content.contains("No preview"));
    }

    #[test]
    fn test_preview_with_content() {
        let mut state = PreviewState::default();
        state.content_lines = vec![
            Line::from("line 1"),
            Line::from("line 2"),
            Line::from("line 3"),
        ];
        state.total_lines = 3;
        let tc = test_theme();
        let widget = PreviewWidget::new(&state, &tc);
        let area = Rect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        let row0: String = (0..20)
            .map(|x| {
                buf.cell((x, 0))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        assert!(row0.contains("line 1"));
    }

    #[test]
    fn test_preview_scroll_offset() {
        let mut state = PreviewState::default();
        state.content_lines = vec![
            Line::from("line 1"),
            Line::from("line 2"),
            Line::from("line 3"),
        ];
        state.total_lines = 3;
        state.scroll_offset = 1;
        let tc = test_theme();
        let widget = PreviewWidget::new(&state, &tc);
        let area = Rect::new(0, 0, 20, 2);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        let row0: String = (0..20)
            .map(|x| {
                buf.cell((x, 0))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        assert!(row0.contains("line 2"));
    }

    #[test]
    fn test_preview_scroll_offset_clamps_to_bottom_start() {
        let mut state = PreviewState::default();
        state.content_lines = vec![
            Line::from("line 1"),
            Line::from("line 2"),
            Line::from("line 3"),
            Line::from("line 4"),
            Line::from("line 5"),
        ];
        state.total_lines = 5;
        state.scroll_offset = 99;
        let tc = test_theme();
        let widget = PreviewWidget::new(&state, &tc);
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        let row0: String = (0..20)
            .map(|x| {
                buf.cell((x, 0))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        assert!(row0.contains("line 3"));
    }

    #[test]
    fn test_zero_area_no_panic() {
        let state = PreviewState::default();
        let tc = test_theme();
        let widget = PreviewWidget::new(&state, &tc);
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
    }

    #[test]
    fn test_preview_selection_highlight() {
        let mut state = PreviewState::default();
        state.content_lines = vec![Line::from("hello world"), Line::from("line two")];
        state.total_lines = 2;

        let mut selection = TerminalSelection::default();
        selection.set_anchor(TerminalCoord { line: 0, col: 6 });
        selection.set_endpoint(TerminalCoord { line: 0, col: 10 });

        let tc = test_theme();
        let widget = PreviewWidget::new(&state, &tc).selection(&selection);
        let area = Rect::new(0, 0, 20, 4);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let selected = buf.cell((6, 0)).unwrap();
        assert_eq!(selected.bg, tc.editor_selection_bg);

        let unselected = buf.cell((0, 0)).unwrap();
        assert_ne!(unselected.bg, tc.editor_selection_bg);
    }
}
