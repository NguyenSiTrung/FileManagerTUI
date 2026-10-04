//! Navigable diagnostics panel: a bounded, severity-sorted flat list of
//! every server's published diagnostics, rebuilt when the store's
//! revision counter moves. Enter navigates to the row's position;
//! next/previous commands live on `App` for the active document only.

use crate::diagnostics::{Diagnostics, Row, Severity};
use ratatui::layout::Rect;

/// Rows rendered at once (hard bound, same contract as LanguageFeatures).
const LIST_HEIGHT: u16 = 12;

/// The panel is a *view* over the store — `rows` is a snapshot taken at
/// `rebuild`, keyed to the store revision it was built from, so a publish
/// landing while the panel is open refreshes it without a per-frame
/// borrow of the whole store.
pub struct DiagnosticsPanel {
    pub rows: Vec<Row>,
    pub selected: usize,
    pub scroll: usize,
    /// Store revision this snapshot was built from.
    pub built_revision: u64,
    /// Render-time geometry for mouse hit-testing.
    pub area: Rect,
    pub row_rects: Vec<Rect>,
}

impl DiagnosticsPanel {
    /// Snapshot the store's flat bounded rows.
    pub fn rebuild(&mut self, store: &Diagnostics) {
        self.rows = store.rows();
        self.built_revision = store.revision();
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.len() == 0 {
            self.selected = 0;
            return;
        }
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.len().saturating_sub(1));
    }

    pub fn selected(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    /// Mouse hit-test: which list row holds (column, row)?
    pub fn hit(&self, column: u16, row: u16) -> Option<usize> {
        self.row_rects
            .iter()
            .position(|r| r.contains((column, row).into()))
            .map(|i| self.scroll + i)
    }

    /// Border title with the workspace-wide severity summary.
    pub fn title(&self, store: &Diagnostics) -> String {
        let summary = store.total_summary();
        let label = summary.label();
        if label.is_empty() {
            " Diagnostics ".to_string()
        } else {
            format!(" Diagnostics · {label} ")
        }
    }

    pub fn render(
        &mut self,
        store: &Diagnostics,
        theme: &crate::theme::ThemeColors,
        frame: &mut ratatui::Frame,
    ) {
        self.render_in_area(store, theme, frame, frame.area());
    }

    pub(crate) fn render_in_area(
        &mut self,
        store: &Diagnostics,
        theme: &crate::theme::ThemeColors,
        frame: &mut ratatui::Frame,
        area: Rect,
    ) {
        use ratatui::{
            style::{Color, Style},
            widgets::{Block, Borders, Clear, Paragraph},
        };
        let width = area.width.saturating_sub(4).clamp(28, 96);
        let height = (LIST_HEIGHT + 3).min(area.height).max(3).min(area.height);
        let x = area.x + area.width.saturating_sub(width) / 2;
        let y = area.y + area.height.saturating_sub(height) / 4;
        self.area = Rect::new(x, y, width.min(area.width), height);
        self.row_rects.clear();
        if self.area.width == 0 || self.area.height == 0 {
            return;
        }
        frame.render_widget(Clear, self.area);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(self.title(store));
        let inner = block.inner(self.area);
        frame.render_widget(block, self.area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let list_height = inner.height.saturating_sub(1) as usize;
        let len = self.rows.len();
        self.selected = self.selected.min(len.saturating_sub(1));
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        if self.selected >= self.scroll.saturating_add(list_height) {
            self.scroll = self.selected.saturating_add(1).saturating_sub(list_height);
        }
        self.scroll = self.scroll.min(len.saturating_sub(list_height));
        for index in self.scroll..len.min(self.scroll + list_height) {
            let row = Rect::new(
                inner.x,
                inner.y + (index - self.scroll) as u16,
                inner.width,
                1,
            );
            self.row_rects.push(row);
            let entry = &self.rows[index];
            let selected = index == self.selected;
            let (marker, fg) = severity_paint(entry.severity, theme);
            let text = row_text(entry);
            let style = if selected {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default().fg(fg)
            };
            let prefix = if selected { ">" } else { " " };
            frame.render_widget(
                Paragraph::new(format!("{prefix} {marker} {text}")).style(style),
                row,
            );
        }
        if self.rows.is_empty() {
            frame.render_widget(Paragraph::new(" No diagnostics"), inner);
        }
        // Footer: count + hint, clipped into the last inner row.
        if inner.height >= 1 {
            let footer = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
            let hint = format!(" {} · Enter: jump · Esc: close", self.rows.len());
            frame.render_widget(
                Paragraph::new(hint).style(Style::default().fg(theme.editor_gutter_sep)),
                footer,
            );
        }
    }
}

impl Default for DiagnosticsPanel {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            built_revision: u64::MAX,
            area: Rect::default(),
            row_rects: Vec::new(),
        }
    }
}

/// `[E]`/`[W]`/… marker + the severity's paint color.
fn severity_paint(
    severity: Severity,
    theme: &crate::theme::ThemeColors,
) -> (&'static str, ratatui::style::Color) {
    match severity {
        Severity::Error => ("E", theme.error_fg),
        Severity::Warning => ("W", theme.warning_fg),
        Severity::Information => ("I", theme.editor_line_nr_current),
        Severity::Hint => ("H", theme.editor_line_nr),
    }
}

/// `file.rs:12:4 — message (source@server)`: one bounded display line.
fn row_text(row: &Row) -> String {
    let path = crate::lsp::features::path_for_uri(&row.uri)
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| row.uri.clone());
    let mut text = format!(
        "{path}:{}:{} — {}",
        row.line + 1,
        row.character + 1,
        row.message
    );
    if let Some(source) = &row.source {
        text.push_str(&format!(" ({source}@{})", row.language));
    } else {
        text.push_str(&format!(" ({})", row.language));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{Diagnostic, Publish};

    fn store_with(entries: &[(&str, u32, Severity, &str)]) -> Diagnostics {
        let mut store = Diagnostics::new();
        // One publish per URI — a publish replaces the whole entry.
        let mut uris: Vec<&str> = entries.iter().map(|(uri, ..)| *uri).collect();
        uris.sort_unstable();
        uris.dedup();
        for uri in uris {
            store.apply(
                Publish {
                    language: "rust".to_string(),
                    generation: 0,
                    uri: uri.to_string(),
                    version: None,
                    diagnostics: entries
                        .iter()
                        .filter(|(u, ..)| *u == uri)
                        .map(|(_, line, severity, message)| Diagnostic {
                            start_line: *line,
                            start_character: 1,
                            end_line: *line,
                            end_character: 2,
                            severity: *severity,
                            code: None,
                            source: Some("clippy".to_string()),
                            message: message.to_string(),
                        })
                        .collect(),
                },
                None,
            );
        }
        store
    }

    #[test]
    fn panel_navigates_and_bounds_selection() {
        let store = store_with(&[
            ("file:///a.rs", 0, Severity::Error, "a"),
            ("file:///a.rs", 2, Severity::Warning, "b"),
            ("file:///b.rs", 1, Severity::Hint, "c"),
        ]);
        let mut panel = DiagnosticsPanel::default();
        panel.rebuild(&store);
        assert_eq!(panel.len(), 3);
        panel.move_selection(10);
        assert_eq!(panel.selected, 2);
        panel.move_selection(isize::MAX);
        assert_eq!(panel.selected, 2);
        panel.move_selection(isize::MIN);
        assert_eq!(panel.selected, 0);
        panel.move_selection(-5);
        assert_eq!(panel.selected, 0);
        assert_eq!(panel.selected().unwrap().message, "a");
        // Empty panel: selection stays put and selected() is None.
        let empty = Diagnostics::new();
        let mut panel = DiagnosticsPanel::default();
        panel.rebuild(&empty);
        panel.move_selection(1);
        assert!(panel.selected().is_none());
    }

    #[test]
    fn rebuild_follows_store_revision_and_clamps_selection() {
        let mut store = Diagnostics::new();
        let mut panel = DiagnosticsPanel::default();
        panel.rebuild(&store);
        assert_eq!(panel.built_revision, store.revision());
        store.apply(
            Publish {
                language: "rust".to_string(),
                generation: 0,
                uri: "file:///a.rs".to_string(),
                version: None,
                diagnostics: vec![Diagnostic {
                    start_line: 0,
                    start_character: 0,
                    end_line: 0,
                    end_character: 1,
                    severity: Severity::Error,
                    code: None,
                    source: None,
                    message: "x".to_string(),
                }],
            },
            None,
        );
        assert_ne!(panel.built_revision, store.revision());
        panel.rebuild(&store);
        assert_eq!(panel.built_revision, store.revision());
        assert_eq!(panel.len(), 1);
        panel.selected = 9;
        panel.rebuild(&store);
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn title_shows_severity_summary() {
        let store = store_with(&[
            ("file:///a.rs", 0, Severity::Error, "e"),
            ("file:///a.rs", 1, Severity::Error, "e2"),
            ("file:///b.rs", 0, Severity::Warning, "w"),
            ("file:///c.rs", 0, Severity::Hint, "h"),
        ]);
        let panel = DiagnosticsPanel::default();
        assert_eq!(panel.title(&store), " Diagnostics · E:2 W:1 H:1 ");
        let clean = Diagnostics::new();
        assert_eq!(panel.title(&clean), " Diagnostics ");
    }

    #[test]
    fn render_bounds_rows_and_survives_tiny_frames() {
        use ratatui::{backend::TestBackend, Terminal};
        let store = store_with(&[
            ("file:///a.rs", 0, Severity::Error, "first"),
            ("file:///a.rs", 1, Severity::Warning, "second"),
            ("file:///b.rs", 3, Severity::Hint, "third"),
        ]);
        let mut panel = DiagnosticsPanel::default();
        panel.rebuild(&store);
        // Tiny frame: no panic, bounded surface.
        let backend = TestBackend::new(10, 4);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let mut panel = panel;
                panel.render(&store, &crate::theme::dark_theme(), f)
            })
            .unwrap();
        // Normal frame: rows + severity markers + summary title render.
        let store2 = store_with(&[
            ("file:///x.rs", 0, Severity::Error, "boom"),
            ("file:///y.rs", 5, Severity::Warning, "careful"),
        ]);
        let mut panel = DiagnosticsPanel::default();
        panel.rebuild(&store2);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| panel.render(&store2, &crate::theme::dark_theme(), f))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Diagnostics · E:1 W:1"));
        assert!(text.contains("boom"));
        assert!(text.contains("careful"));
        assert!(text.contains("E"));
        assert!(text.contains("W"));
        // Selected row recorded for hit-testing.
        assert!(!panel.row_rects.is_empty());
        let hit = panel.hit(panel.row_rects[0].x + 1, panel.row_rects[0].y);
        assert_eq!(hit, Some(0));
        // A second draw with a different selection still bounds the rows.
        let mut tiny = DiagnosticsPanel::default();
        tiny.rebuild(&store2);
        tiny.move_selection(10);
        let backend = TestBackend::new(30, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| tiny.render(&store2, &crate::theme::dark_theme(), f))
            .unwrap();
        assert!(tiny.selected <= tiny.len().saturating_sub(1));
    }
}
