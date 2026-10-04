//! Project content-search overlay: literal path/line results and status.
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Widget},
};

use crate::background::RequestGeneration;
use crate::search::SearchHit;
use crate::theme::ThemeColors;

/// Bounded, generation-tagged content-search model. Only the worker writes
/// hits; the UI thread never traverses the filesystem to refresh this state.
#[derive(Debug, Default)]
pub struct ContentSearchState {
    pub query: String,
    pub cursor_position: usize,
    pub hits: Vec<SearchHit>,
    pub selected_index: usize,
    pub scanning: bool,
    pub complete: bool,
    pub capped: bool,
    pub scanned_files: usize,
    pub unreadable: usize,
    /// Immutable identity of the request that owns the current hits.
    pub generation: Option<RequestGeneration>,
}

impl ContentSearchState {
    pub fn retire(&mut self) {
        self.hits.clear();
        self.selected_index = 0;
        self.scanning = false;
        self.complete = false;
        self.capped = false;
        self.scanned_files = 0;
        self.unreadable = 0;
        self.generation = None;
    }

    /// Human-readable status. Never claims a complete scan when a cap, an
    /// unreadable entry, a refused continuation or a failed delivery stopped it.
    pub fn status_text(&self, query_empty: bool) -> String {
        if query_empty {
            return "Type to search file contents...".to_string();
        }
        let count = self.hits.len();
        let plural = if count == 1 { "" } else { "es" };
        if self.scanning {
            return format!("{count} match{plural} (scanning {})", self.scanned_files);
        }
        if self.capped || self.unreadable > 0 {
            let reason = if self.capped {
                if self.unreadable > 0 {
                    "cap + unreadable"
                } else {
                    "cap/deadline"
                }
            } else {
                "unreadable entries"
            };
            return format!("{count} match{plural} (incomplete: {reason})");
        }
        if self.complete {
            return format!("{count} match{plural} (complete)");
        }
        // Not scanning, not capped, not complete: the request never ran or was
        // refused. Say so instead of presenting a bare count as final.
        format!("{count} match{plural} (incomplete)")
    }

    pub fn selected_hit(&self) -> Option<&SearchHit> {
        self.hits.get(self.selected_index)
    }

    pub fn select_next(&mut self) {
        if !self.hits.is_empty() && self.selected_index < self.hits.len() - 1 {
            self.selected_index += 1;
        }
    }

    pub fn select_previous(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(1);
    }
}

/// Content-search results widget.
pub struct ContentSearchWidget<'a> {
    state: &'a ContentSearchState,
    theme: &'a ThemeColors,
    block: Option<Block<'a>>,
}

impl<'a> ContentSearchWidget<'a> {
    pub fn new(state: &'a ContentSearchState, theme: &'a ThemeColors) -> Self {
        Self {
            state,
            theme,
            block: None,
        }
    }

    #[allow(dead_code)]
    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
        let x = area.x + area.width.saturating_sub(width) / 2;
        let y = area.y + area.height.saturating_sub(height) / 2;
        Rect::new(x, y, width.min(area.width), height.min(area.height))
    }
}

impl<'a> Widget for ContentSearchWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 5 || area.width < 20 {
            return;
        }

        let dialog_width = (area.width * 70 / 100).clamp(30, 100);
        let dialog_height = (area.height * 70 / 100).clamp(8, 30);
        let rect = Self::centered_rect(dialog_width, dialog_height, area);
        Clear.render(rect, buf);

        let block = Block::default()
            .title(" Content Search (literal) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.dialog_border_fg))
            .padding(Padding::horizontal(1));
        let inner = block.inner(rect);
        block.render(rect, buf);
        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let query = &self.state.query;
        let cursor_pos = self.state.cursor_position.min(query.len());
        let (before, cursor_char, after) = if cursor_pos < query.len() {
            match query.get(cursor_pos..cursor_pos + 1) {
                Some(ch) => (
                    query.get(..cursor_pos).unwrap_or(""),
                    ch,
                    query.get(cursor_pos + 1..).unwrap_or(""),
                ),
                None => (query.as_str(), " ", ""),
            }
        } else {
            (query.as_str(), " ", "")
        };

        let input_style = Style::default().fg(self.theme.status_fg);
        let prompt_style = Style::default()
            .fg(self.theme.info_fg)
            .add_modifier(Modifier::BOLD);
        let cursor_style = Style::default()
            .bg(self.theme.status_fg)
            .fg(self.theme.dialog_bg)
            .add_modifier(Modifier::BOLD);
        let input_line = Line::from(vec![
            Span::styled("> ", prompt_style),
            Span::styled(before, input_style),
            Span::styled(cursor_char, cursor_style),
            Span::styled(after, input_style),
        ]);
        buf.set_line(inner.x, inner.y, &input_line, inner.width);

        if inner.height > 1 {
            let status = self.state.status_text(query.is_empty());
            let sep = Line::from(Span::styled(
                format!("─── {status} "),
                Style::default().fg(self.theme.dim_fg),
            ));
            buf.set_line(inner.x, inner.y + 1, &sep, inner.width);
        }

        let results_start = 2u16;
        let visible = inner.height.saturating_sub(results_start) as usize;
        let scroll = if self.state.selected_index >= visible {
            self.state.selected_index - visible + 1
        } else {
            0
        };
        let max_width = inner.width.saturating_sub(2) as usize;

        for (row_offset, (index, hit)) in self
            .state
            .hits
            .iter()
            .enumerate()
            .skip(scroll)
            .take(visible)
            .enumerate()
        {
            let row = inner.y + results_start + row_offset as u16;
            if row >= inner.y + inner.height {
                break;
            }
            let selected = index == self.state.selected_index;
            let marker = if selected {
                Span::styled(
                    "▸ ",
                    Style::default()
                        .fg(self.theme.info_fg)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw("  ")
            };
            let label_style = if selected {
                Style::default()
                    .fg(self.theme.status_fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(self.theme.dim_fg)
            };
            let excerpt_style = Style::default().fg(self.theme.warning_fg);
            let location = format!("{}:{}  ", hit.path.display(), hit.line);
            let excerpt: String = hit.excerpt.chars().take(max_width).collect();
            let line = Line::from(vec![
                marker,
                Span::styled(truncate(&location, max_width), label_style),
                Span::styled(excerpt, excerpt_style),
            ]);
            buf.set_line(inner.x, row, &line, inner.width);
        }

        if inner.height > 3 {
            let hint = "[Enter] Open hit  [Tab] Filename mode  [Esc] Close  [↑↓] Navigate";
            buf.set_line(
                inner.x,
                inner.y + inner.height - 1,
                &Line::from(Span::styled(
                    hint,
                    Style::default()
                        .fg(self.theme.dim_fg)
                        .add_modifier(Modifier::DIM),
                )),
                inner.width,
            );
        }
    }
}

fn truncate(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        input.to_string()
    } else {
        let truncated: String = input.chars().take(max.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;
    use std::path::PathBuf;

    fn hit(name: &str, line: u64) -> SearchHit {
        SearchHit {
            path: PathBuf::from(name),
            line,
            byte: 0,
            column: 0,
            excerpt: "training: true".into(),
        }
    }

    fn buffer_to_string(buf: &Buffer, area: Rect) -> String {
        let mut output = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                output.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            output.push('\n');
        }
        output
    }

    #[test]
    fn content_search_widget_renders_hits_and_status() {
        let mut state = ContentSearchState {
            query: "training".into(),
            cursor_position: 8,
            ..Default::default()
        };
        state.hits.push(hit("config.yaml", 1));
        state.scanning = false;
        state.complete = true;
        let colors = theme::dark_theme();
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        ContentSearchWidget::new(&state, &colors).render(area, &mut buf);
        let text = buffer_to_string(&buf, area);
        assert!(text.contains("Content Search"));
        assert!(text.contains("config.yaml:1"));
        assert!(text.contains("training: true"));
        assert!(text.contains("complete"));
    }

    #[test]
    fn content_search_status_never_claims_complete_when_capped() {
        let mut state = ContentSearchState {
            query: "x".into(),
            ..Default::default()
        };
        state.hits.push(hit("a.txt", 1));
        state.capped = true;
        let status = state.status_text(false);
        assert!(status.contains("incomplete"), "{status}");
        assert!(!status.contains("(complete)"));
    }

    #[test]
    fn content_search_status_reports_scanning() {
        let state = ContentSearchState {
            query: "x".into(),
            scanning: true,
            scanned_files: 7,
            ..Default::default()
        };
        assert!(state.status_text(false).contains("scanning 7"));
    }

    #[test]
    fn content_search_status_marks_refused_and_partial_requests_incomplete() {
        // Not scanning, not capped, not complete: a request that never ran (a
        // refused admission) must not present a bare count as final.
        let mut state = ContentSearchState {
            query: "x".into(),
            scanning: false,
            complete: false,
            capped: false,
            unreadable: 0,
            ..Default::default()
        };
        let status = state.status_text(false);
        assert!(status.contains("incomplete"), "{status}");
        assert!(!status.contains("(complete)"), "{status}");

        // A partial hit set from a failed delivery reports the same marker.
        state.hits.push(hit("a.txt", 1));
        let status = state.status_text(false);
        assert!(status.contains("1 match (incomplete)"), "{status}");
    }

    #[test]
    fn content_search_widget_small_area_is_safe() {
        let state = ContentSearchState::default();
        let colors = theme::dark_theme();
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        ContentSearchWidget::new(&state, &colors).render(area, &mut buf);
    }

    #[test]
    fn content_search_status_covers_all_terminal_states() {
        let mut state = ContentSearchState::default();
        assert!(state.status_text(true).contains("Type to search"));
        state.query = "x".into();
        state.capped = true;
        state.unreadable = 2;
        assert!(state.status_text(false).contains("cap + unreadable"));
        state.capped = false;
        state.unreadable = 1;
        assert!(state.status_text(false).contains("unreadable entries"));
        state.unreadable = 0;
        assert!(state.status_text(false).contains("0 match"));
    }

    #[test]
    fn content_search_selection_and_accessors_are_bounded() {
        let mut state = ContentSearchState::default();
        assert!(state.selected_hit().is_none());
        state.hits = vec![hit("a", 1), hit("b", 2)];
        assert_eq!(state.selected_hit().unwrap().path, PathBuf::from("a"));
        state.select_next();
        assert_eq!(state.selected_index, 1);
        state.select_next();
        assert_eq!(state.selected_index, 1);
        state.select_previous();
        state.select_previous();
        assert_eq!(state.selected_index, 0);
        let colors = theme::dark_theme();
        let widget = ContentSearchWidget::new(&state, &colors).block(Block::default());
        let area = Rect::new(0, 0, 60, 12);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
    }

    #[test]
    fn content_search_widget_renders_scrolled_selection_and_long_labels() {
        let state = ContentSearchState {
            query: "q".into(),
            cursor_position: 1,
            hits: (0..6)
                .map(|i| hit(&format!("dir/{i}/{}", "x".repeat(120)), i + 1))
                .collect(),
            selected_index: 5,
            ..Default::default()
        };
        let colors = theme::dark_theme();
        let area = Rect::new(0, 0, 80, 12);
        let mut buf = Buffer::empty(area);
        ContentSearchWidget::new(&state, &colors).render(area, &mut buf);
    }

    #[test]
    fn content_search_retire_resets_status() {
        let mut state = ContentSearchState {
            hits: vec![hit("a", 1)],
            scanned_files: 9,
            unreadable: 3,
            generation: None,
            ..Default::default()
        };
        state.retire();
        assert!(state.hits.is_empty());
        assert_eq!(state.scanned_files, 0);
        assert_eq!(state.unreadable, 0);
        assert!(!state.scanning);
    }
}
