use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Widget},
};

use crate::app::SearchState;

/// Fuzzy finder overlay widget (Ctrl+P).
pub struct SearchWidget<'a> {
    state: &'a SearchState,
    block: Option<Block<'a>>,
}

impl<'a> SearchWidget<'a> {
    pub fn new(state: &'a SearchState) -> Self {
        Self { state, block: None }
    }

    #[allow(dead_code)]
    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
        let x = area.x + area.width.saturating_sub(width) / 2;
        let y = area.y + area.height.saturating_sub(height) / 2;
        let w = width.min(area.width);
        let h = height.min(area.height);
        Rect::new(x, y, w, h)
    }
}

impl<'a> Widget for SearchWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 5 || area.width < 20 {
            return;
        }

        // Size: 60% width, 60% height, capped
        let dialog_width = (area.width * 60 / 100).clamp(30, 80);
        let dialog_height = (area.height * 60 / 100).clamp(8, 30);
        let rect = Self::centered_rect(dialog_width, dialog_height, area);

        Clear.render(rect, buf);

        let block = Block::default()
            .title(" Fuzzy Finder (Ctrl+P) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .padding(Padding::horizontal(1));

        let inner = block.inner(rect);
        block.render(rect, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let max_width = inner.width as usize;

        // Row 0: Search input with cursor
        let query = &self.state.query;
        let cursor_pos = self.state.cursor_position;

        let (before, cursor_char, after) = if cursor_pos < query.len() {
            let ch = &query[cursor_pos..cursor_pos + 1];
            (&query[..cursor_pos], ch, &query[cursor_pos + 1..])
        } else {
            (query.as_str(), " ", "")
        };

        let input_style = Style::default().fg(Color::White);
        let cursor_style = Style::default()
            .bg(Color::White)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD);
        let prompt_style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);

        let input_spans = vec![
            Span::styled("> ", prompt_style),
            Span::styled(before, input_style),
            Span::styled(cursor_char, cursor_style),
            Span::styled(after, input_style),
        ];
        let input_line = Line::from(input_spans);
        buf.set_line(inner.x, inner.y, &input_line, inner.width);

        // Row 1: Separator + result count
        if inner.height > 1 {
            let count_str = if self.state.query.is_empty() {
                "Type to search...".to_string()
            } else {
                format!(
                    "{} result{}",
                    self.state.results.len(),
                    if self.state.results.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                )
            };
            let sep_line = Line::from(Span::styled(
                format!("─── {} ", count_str),
                Style::default().fg(Color::DarkGray),
            ));
            buf.set_line(inner.x, inner.y + 1, &sep_line, inner.width);
        }

        // Row 2+: Results list
        let results_start = 2u16;
        let visible_results = (inner.height.saturating_sub(results_start)) as usize;

        // Calculate scroll offset for results to keep selected visible
        let scroll = if self.state.selected_index >= visible_results {
            self.state.selected_index - visible_results + 1
        } else {
            0
        };

        for (i, result) in self
            .state
            .results
            .iter()
            .skip(scroll)
            .take(visible_results)
            .enumerate()
        {
            let row = inner.y + results_start + i as u16;
            if row >= inner.y + inner.height {
                break;
            }

            let is_selected = (i + scroll) == self.state.selected_index;

            // Build display with highlighted match characters
            let display = &result.display;
            let match_set: std::collections::HashSet<usize> =
                result.match_indices.iter().copied().collect();

            let mut spans = Vec::new();

            // Selection indicator
            if is_selected {
                spans.push(Span::styled(
                    "▸ ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::raw("  "));
            }

            // Path with highlighted match chars
            let base_style = if is_selected {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::Gray)
            };
            let highlight_style = Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD);

            // Build spans char by char, grouping consecutive same-style chars
            let mut current_text = String::new();
            let mut current_highlighted = false;

            for (ci, ch) in display.chars().enumerate() {
                let is_match = match_set.contains(&ci);
                if is_match != current_highlighted && !current_text.is_empty() {
                    let style = if current_highlighted {
                        highlight_style
                    } else {
                        base_style
                    };
                    spans.push(Span::styled(current_text.clone(), style));
                    current_text.clear();
                }
                current_highlighted = is_match;
                current_text.push(ch);

                // Truncate if too wide
                let used: usize = spans.iter().map(|s| s.content.len()).sum();
                if used + current_text.len() >= max_width.saturating_sub(2) {
                    break;
                }
            }
            if !current_text.is_empty() {
                let style = if current_highlighted {
                    highlight_style
                } else {
                    base_style
                };
                spans.push(Span::styled(current_text, style));
            }

            let line = Line::from(spans);
            buf.set_line(inner.x, row, &line, inner.width);
        }

        // Hint at bottom
        if inner.height > 3 {
            let hint = "[Enter] Open  [Esc] Close  [↑↓] Navigate";
            let hint_style = Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM);
            let hint_line = Line::from(Span::styled(hint, hint_style));
            buf.set_line(inner.x, inner.y + inner.height - 1, &hint_line, inner.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SearchResult;
    use std::path::PathBuf;

    fn buffer_to_string(buf: &Buffer, area: Rect) -> String {
        let mut s = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                s.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn test_empty_search_renders() {
        let state = SearchState::default();
        let widget = SearchWidget::new(&state);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content = buffer_to_string(&buf, area);
        assert!(content.contains("Fuzzy Finder"));
        assert!(content.contains("Type to search"));
    }

    #[test]
    fn test_search_with_results_renders() {
        let mut state = SearchState::default();
        state.query = "test".to_string();
        state.cursor_position = 4;
        state.results = vec![
            SearchResult {
                path: PathBuf::from("/tmp/test.txt"),
                display: "test.txt".to_string(),
                score: 100,
                match_indices: vec![0, 1, 2, 3],
            },
            SearchResult {
                path: PathBuf::from("/tmp/foo/test.rs"),
                display: "foo/test.rs".to_string(),
                score: 90,
                match_indices: vec![4, 5, 6, 7],
            },
        ];

        let widget = SearchWidget::new(&state);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content = buffer_to_string(&buf, area);
        assert!(content.contains("test.txt"));
        assert!(content.contains("foo/test.rs"));
        assert!(content.contains("2 results"));
    }

    #[test]
    fn test_search_selection_indicator() {
        let mut state = SearchState::default();
        state.query = "t".to_string();
        state.cursor_position = 1;
        state.selected_index = 1;
        state.results = vec![
            SearchResult {
                path: PathBuf::from("/a.txt"),
                display: "a.txt".to_string(),
                score: 50,
                match_indices: vec![2],
            },
            SearchResult {
                path: PathBuf::from("/b.txt"),
                display: "b.txt".to_string(),
                score: 40,
                match_indices: vec![2],
            },
        ];

        let widget = SearchWidget::new(&state);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content = buffer_to_string(&buf, area);
        assert!(content.contains("▸"));
    }

    #[test]
    fn test_small_area_no_panic() {
        let state = SearchState::default();
        let widget = SearchWidget::new(&state);
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
    }
}
