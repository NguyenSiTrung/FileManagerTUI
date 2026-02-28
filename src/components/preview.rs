use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Widget},
};

use crate::app::PreviewState;
use crate::theme::ThemeColors;

/// Preview widget that renders file content in the preview panel.
#[allow(dead_code)]
pub struct PreviewWidget<'a> {
    preview_state: &'a PreviewState,
    theme: &'a ThemeColors,
    block: Option<Block<'a>>,
}

impl<'a> PreviewWidget<'a> {
    #[allow(dead_code)]
    pub fn new(preview_state: &'a PreviewState, theme: &'a ThemeColors) -> Self {
        Self {
            preview_state,
            theme,
            block: None,
        }
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
        let start = self.preview_state.scroll_offset;
        let end = (start + visible_height).min(self.preview_state.content_lines.len());

        for (i, line) in self.preview_state.content_lines[start..end]
            .iter()
            .enumerate()
        {
            let y = inner.y + i as u16;
            buf.set_line(inner.x, y, line, inner.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(row0.contains("line 2"));
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
}
