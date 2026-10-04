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

        let width = inner.width as usize;
        let height = inner.height as usize;
        let start = self.preview_state.scroll_offset.min(
            self.preview_state
                .visual_row_count(width)
                .saturating_sub(height),
        );
        let selection = self.selection.and_then(TerminalSelection::normalized);
        let mut visual = 0;
        let mut screen = 0;
        for (line_idx, line) in self.preview_state.content_lines.iter().enumerate() {
            if screen >= height {
                return;
            }
            if !self.preview_state.line_wrap && line_idx < start {
                visual += 1;
                continue;
            }
            let content = crate::text::line_text(line);
            for mapped in
                crate::text::visual_rows(&content, width, self.preview_state.line_wrap, false)
            {
                if visual < start {
                    visual += 1;
                    continue;
                }
                if screen >= height {
                    return;
                }
                let left = if self.preview_state.line_wrap {
                    mapped.start
                } else {
                    self.preview_state.horizontal_offset
                };
                let right = if self.preview_state.line_wrap {
                    mapped.end
                } else {
                    left.saturating_add(width)
                };
                let display_row = if self.preview_state.line_wrap {
                    mapped
                } else {
                    crate::text::VisualRow::window(&content, left, right)
                };
                crate::text::paint_display_row(
                    &content,
                    display_row,
                    Rect::new(inner.x, inner.y + screen as u16, inner.width, 1),
                    buf,
                    line.style,
                    |g| crate::text::line_style_at_byte(line, g.byte),
                );
                // Display-cell selections are inclusive and may cover only part of a tab/wide glyph.
                if let Some((a, b)) = selection {
                    if line_idx >= a.line && line_idx <= b.line {
                        let from = if line_idx == a.line { a.col } else { 0 };
                        let to = if line_idx == b.line {
                            b.col.saturating_add(1)
                        } else {
                            usize::MAX
                        };
                        for col in from.max(left)..to.min(right).min(left.saturating_add(width)) {
                            buf[(inner.x + (col - left) as u16, inner.y + screen as u16)]
                                .set_bg(self.theme.editor_selection_bg);
                        }
                    }
                }
                screen += 1;
                visual += 1;
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
    fn preview_expands_logical_tab_stops() {
        let state = PreviewState {
            content_lines: vec![Line::from("a\t中")],
            ..Default::default()
        };
        let area = Rect::new(0, 0, 8, 1);
        let mut buf = Buffer::empty(area);
        PreviewWidget::new(&state, &test_theme()).render(area, &mut buf);
        assert_eq!(buf[(4, 0)].symbol(), "中");
        assert_eq!(buf[(1, 0)].symbol(), " ");
    }

    #[test]
    fn preview_horizontal_clipping_preserves_span_styles() {
        use ratatui::style::{Color, Modifier};
        let state = PreviewState {
            horizontal_offset: 5,
            content_lines: vec![Line::from(vec![
                Span::raw("a\t中"),
                Span::styled(
                    "e\u{301}👩‍💻Z",
                    Style::default()
                        .fg(Color::Red)
                        .add_modifier(Modifier::ITALIC),
                ),
            ])],
            ..Default::default()
        };
        let area = Rect::new(0, 0, 5, 1);
        let mut buf = Buffer::empty(area);
        PreviewWidget::new(&state, &test_theme()).render(area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), " ");
        assert_eq!(buf[(1, 0)].symbol(), "e\u{301}");
        assert_eq!(buf[(2, 0)].symbol(), "👩‍💻");
        assert_eq!(buf[(3, 0)].symbol(), " ");
        assert_eq!(buf[(4, 0)].symbol(), "Z");
        assert_eq!(buf[(2, 0)].fg, Color::Red);
        assert!(buf[(2, 0)].modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn wrapped_selection_uses_logical_display_cells_across_rows() {
        let state = PreviewState {
            line_wrap: true,
            content_lines: vec![Line::from("abc中\tZ")],
            ..Default::default()
        };
        let mut selection = TerminalSelection::default();
        selection.set_anchor(TerminalCoord { line: 0, col: 3 });
        selection.set_endpoint(TerminalCoord { line: 0, col: 8 });
        let area = Rect::new(0, 0, 4, 3);
        let mut buf = Buffer::empty(area);
        let theme = test_theme();
        PreviewWidget::new(&state, &theme)
            .selection(&selection)
            .render(area, &mut buf);
        assert_eq!(buf[(0, 1)].symbol(), "中");
        assert_eq!(buf[(1, 2)].symbol(), "Z");
        assert_ne!(buf[(3, 0)].bg, theme.editor_selection_bg);
        for (x, y) in [(0, 1), (1, 1), (2, 1), (3, 1), (0, 2), (1, 2)] {
            assert_eq!(buf[(x, y)].bg, theme.editor_selection_bg);
        }
    }

    #[test]
    fn zero_narrow_and_empty_wrapped_preview_is_safe() {
        let state = PreviewState {
            line_wrap: true,
            content_lines: vec![Line::from("中"), Line::from("")],
            scroll_offset: 99,
            ..Default::default()
        };
        for width in 0..3 {
            for height in 0..3 {
                let area = Rect::new(0, 0, width, height);
                let mut buf = Buffer::empty(area);
                PreviewWidget::new(&state, &test_theme()).render(area, &mut buf);
                if width == 1 && height == 2 {
                    assert_eq!(buf[(0, 0)].symbol(), " ");
                }
            }
        }
    }

    #[test]
    fn preview_wrap_preserves_style_and_logical_tabs() {
        let state = PreviewState {
            line_wrap: true,
            content_lines: vec![Line::from(Span::styled(
                "abc中\tZ",
                Style::default().fg(ratatui::style::Color::Red),
            ))],
            ..Default::default()
        };
        let area = Rect::new(0, 0, 4, 3);
        let mut buf = Buffer::empty(area);
        PreviewWidget::new(&state, &test_theme()).render(area, &mut buf);
        assert_eq!(buf[(0, 1)].symbol(), "中");
        assert_eq!(buf[(0, 1)].fg, ratatui::style::Color::Red);
        assert_eq!(buf[(1, 2)].symbol(), "Z");
    }

    #[test]
    fn display_selection_only_highlights_inclusive_cells() {
        let state = PreviewState {
            content_lines: vec![Line::from("\t中X")],
            ..Default::default()
        };
        let mut selection = TerminalSelection::default();
        selection.set_anchor(TerminalCoord { line: 0, col: 2 });
        selection.set_endpoint(TerminalCoord { line: 0, col: 4 });
        let area = Rect::new(0, 0, 8, 1);
        let mut buf = Buffer::empty(area);
        let theme = test_theme();
        PreviewWidget::new(&state, &theme)
            .selection(&selection)
            .render(area, &mut buf);
        assert_ne!(buf[(1, 0)].bg, theme.editor_selection_bg);
        assert_eq!(buf[(2, 0)].bg, theme.editor_selection_bg);
        assert_eq!(buf[(4, 0)].bg, theme.editor_selection_bg);
        assert_ne!(buf[(5, 0)].bg, theme.editor_selection_bg);
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
        let state = PreviewState {
            content_lines: vec![
                Line::from("line 1"),
                Line::from("line 2"),
                Line::from("line 3"),
            ],
            total_lines: 3,
            ..Default::default()
        };
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
        let state = PreviewState {
            content_lines: vec![
                Line::from("line 1"),
                Line::from("line 2"),
                Line::from("line 3"),
            ],
            total_lines: 3,
            scroll_offset: 1,
            ..Default::default()
        };
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
        let state = PreviewState {
            content_lines: vec![
                Line::from("line 1"),
                Line::from("line 2"),
                Line::from("line 3"),
                Line::from("line 4"),
                Line::from("line 5"),
            ],
            total_lines: 5,
            scroll_offset: 99,
            ..Default::default()
        };
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
        let state = PreviewState {
            content_lines: vec![Line::from("hello world"), Line::from("line two")],
            total_lines: 2,
            ..Default::default()
        };

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
