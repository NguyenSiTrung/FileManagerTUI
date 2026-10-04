//! Bounded overlay presenting language-feature results: completion picks,
//! hover text, locations (definition/references), and document symbols.
//!
//! The overlay carries the request's staleness tokens (`uri`, `revision`)
//! with it: the app re-checks them against the document before applying
//! any selection, so a late result can never touch a moved buffer.

use crate::lsp::features::{CompletionEntry, LocationEntry, SymbolEntry};
use crate::workspace::documents::DocumentId;
use ratatui::layout::Rect;

/// Number of rows the list renders at once (hard bound regardless of how
/// many items the server sent).
const LIST_HEIGHT: u16 = 12;

/// What the overlay is showing.
pub enum FeatureView {
    /// Completion candidates; Enter applies the selection atomically.
    Completion { items: Vec<CompletionEntry> },
    /// Read-only text (hover), already sanitized and bounded.
    Text { title: String, lines: Vec<String> },
    /// Definition/references results; Enter navigates.
    Locations {
        title: String,
        items: Vec<LocationEntry>,
    },
    /// Document symbols; Enter navigates within the same document.
    Symbols { items: Vec<SymbolEntry> },
}

/// One feature overlay: bounded list + detail + scroll bookkeeping for
/// mouse hit-testing (same contract as `CommandMenu`).
pub struct LanguageFeatures {
    /// Document the request was issued for — staleness anchor.
    pub document: DocumentId,
    /// URI the request addressed — a rename invalidates the results.
    pub uri: String,
    /// `content_revision` at request time — buffer-moved invalidation.
    pub revision: u64,
    pub view: FeatureView,
    /// Currently highlighted row across the whole view model.
    pub selected: usize,
    pub scroll: usize,
    /// Render-time geometry, refreshed every frame.
    pub area: Rect,
    pub rows: Vec<Rect>,
}

impl LanguageFeatures {
    pub fn new(document: DocumentId, uri: String, revision: u64, view: FeatureView) -> Self {
        Self {
            document,
            uri,
            revision,
            view,
            selected: 0,
            scroll: 0,
            area: Rect::default(),
            rows: Vec::new(),
        }
    }

    /// Mouse hit-test: which list row holds (column, row)?
    pub fn hit(&self, column: u16, row: u16) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| r.contains((column, row).into()))
            .map(|i| self.scroll + i)
    }

    /// Row count of the active view.
    pub fn len(&self) -> usize {
        match &self.view {
            FeatureView::Completion { items } => items.len(),
            FeatureView::Text { lines, .. } => lines.len(),
            FeatureView::Locations { items, .. } => items.len(),
            FeatureView::Symbols { items } => items.len(),
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        let len = self.len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1));
    }

    /// Title text for the border.
    pub fn title(&self) -> String {
        match &self.view {
            FeatureView::Completion { items } => format!(" Completion · {} ", items.len()),
            FeatureView::Text { title, .. } => format!(" {title} "),
            FeatureView::Locations { title, items } => {
                format!(" {title} · {} ", items.len())
            }
            FeatureView::Symbols { items } => format!(" Symbols · {} ", items.len()),
        }
    }

    /// Selected completion entry, if the view is a completion list.
    pub fn selected_completion(&self) -> Option<&CompletionEntry> {
        match &self.view {
            FeatureView::Completion { items } => items.get(self.selected),
            _ => None,
        }
    }

    /// Selected location (definition/references row).
    pub fn selected_location(&self) -> Option<&LocationEntry> {
        match &self.view {
            FeatureView::Locations { items, .. } => items.get(self.selected),
            _ => None,
        }
    }

    /// Selected document symbol.
    pub fn selected_symbol(&self) -> Option<&SymbolEntry> {
        match &self.view {
            FeatureView::Symbols { items } => items.get(self.selected),
            _ => None,
        }
    }

    /// Row text for the given index — pre-sanitized display strings.
    fn row_text(&self, index: usize) -> String {
        match &self.view {
            FeatureView::Completion { items } => items
                .get(index)
                .map(|item| {
                    let kind = item.kind.map(|k| format!("[{k}] ")).unwrap_or_default();
                    let detail = item.detail.as_deref().unwrap_or("");
                    if detail.is_empty() {
                        format!("{kind}{}", item.label)
                    } else {
                        format!("{kind}{} — {}", item.label, detail)
                    }
                })
                .unwrap_or_default(),
            FeatureView::Text { lines, .. } => lines.get(index).cloned().unwrap_or_default(),
            FeatureView::Locations { items, .. } => items
                .get(index)
                .map(|loc| {
                    format!(
                        "{}:{}:{}",
                        crate::lsp::features::sanitize_server_text(&loc.uri, 120),
                        loc.start_line + 1,
                        loc.start_character + 1
                    )
                })
                .unwrap_or_default(),
            FeatureView::Symbols { items } => items
                .get(index)
                .map(|sym| match &sym.container {
                    Some(container) => format!("{} :: {}", container, sym.name),
                    None => sym.name.clone(),
                })
                .unwrap_or_default(),
        }
    }

    /// Detail line for the footer — completion docs/details where present.
    fn detail_line(&self) -> String {
        if let FeatureView::Completion { items } = &self.view {
            items
                .get(self.selected)
                .and_then(|item| {
                    let mut flags = String::new();
                    if item.snippet {
                        flags.push_str(" [snippet — unsupported]");
                    }
                    if item.has_command {
                        flags.push_str(" [has command — not run]");
                    }
                    if item.deprecated {
                        flags.push_str(" [deprecated]");
                    }
                    item.documentation
                        .as_deref()
                        .map(|d| format!("{d}{flags}"))
                        .or_else(|| (!flags.is_empty()).then(|| flags.trim().to_string()))
                })
                .unwrap_or_default()
        } else {
            String::new()
        }
    }

    pub fn render(&mut self, _app: &crate::app::App, frame: &mut ratatui::Frame) {
        self.render_in_area(frame, frame.area());
    }

    pub(crate) fn render_in_area(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        use ratatui::{
            style::{Color, Style},
            widgets::{Block, Borders, Clear, Paragraph},
        };
        // Bounded surface centered like the command menu.
        let width = area.width.saturating_sub(8).clamp(20, 72);
        // Never exceed the frame: tiny terminals get a shrunken surface.
        let height = (LIST_HEIGHT + 4).min(area.height).max(3).min(area.height);
        let x = area.x + area.width.saturating_sub(width) / 2;
        let y = area.y + area.height.saturating_sub(height) / 4;
        self.area = Rect::new(x, y, width.min(area.width), height);
        self.rows.clear();
        if self.area.width == 0 || self.area.height == 0 {
            return;
        }
        frame.render_widget(Clear, self.area);
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let inner = block.inner(self.area);
        frame.render_widget(block, self.area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let list_height = inner.height.saturating_sub(1) as usize;
        let len = self.len();
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
            let marker = if index == self.selected { ">" } else { " " };
            let style = if index == self.selected {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default()
            };
            frame.render_widget(
                Paragraph::new(format!("{marker} {}", self.row_text(index))).style(style),
                row,
            );
            self.rows.push(row);
        }
        if inner.height >= 1 {
            let footer = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
            let hint = match &self.view {
                FeatureView::Completion { .. } => {
                    format!(
                        "{}/{} · Enter applies · Esc",
                        if len == 0 { 0 } else { self.selected + 1 },
                        len
                    )
                }
                FeatureView::Text { .. } => "Esc".to_string(),
                _ => format!(
                    "{}/{} · Enter jumps · Esc",
                    if len == 0 { 0 } else { self.selected + 1 },
                    len
                ),
            };
            let detail = self.detail_line();
            let text = if detail.is_empty() {
                hint
            } else {
                format!(
                    "{hint} · {}",
                    crate::lsp::features::sanitize_server_text(&detail, inner.width as usize)
                )
            };
            frame.render_widget(Paragraph::new(text), footer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::features::{CompletionEdit, LocationEntry, SymbolEntry};

    fn item(label: &str) -> CompletionEntry {
        CompletionEntry {
            label: label.to_string(),
            detail: None,
            kind: None,
            documentation: None,
            edit: CompletionEdit::Insert {
                text: label.to_string(),
            },
            additional_edits: vec![],
            snippet: false,
            has_command: false,
            deprecated: false,
        }
    }

    fn doc_id() -> DocumentId {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.rs");
        std::fs::write(&path, "x").unwrap();
        let mut store = crate::workspace::documents::DocumentStore::new();
        let id = store
            .open(&path, crate::workspace::documents::OpenDisposition::Pinned)
            .unwrap();
        std::mem::forget(dir);
        id
    }

    #[test]
    fn overlay_navigates_and_bounds_selection() {
        let mut f = LanguageFeatures::new(
            doc_id(),
            "file:///f".into(),
            0,
            FeatureView::Completion {
                items: vec![item("a"), item("b"), item("c")],
            },
        );
        assert_eq!(f.len(), 3);
        f.move_selection(1);
        assert_eq!(f.selected, 1);
        f.move_selection(5);
        assert_eq!(f.selected, 2); // clamped
        f.move_selection(isize::MIN);
        assert_eq!(f.selected, 0);
        f.move_selection(isize::MAX);
        assert_eq!(f.selected, 2);
        assert_eq!(f.selected_completion().unwrap().label, "c");
        assert_eq!(f.title(), " Completion · 3 ");
        assert!(f.hit(0, 0).is_none(), "nothing rendered yet");

        // Empty view: selection stays 0 and detail is empty.
        let mut empty = LanguageFeatures::new(
            doc_id(),
            "u".into(),
            0,
            FeatureView::Completion { items: vec![] },
        );
        empty.move_selection(1);
        assert_eq!(empty.selected, 0);
    }

    #[test]
    fn row_text_and_detail_cover_all_views() {
        let mut f = LanguageFeatures::new(
            doc_id(),
            "u".into(),
            0,
            FeatureView::Text {
                title: "Hover".to_string(),
                lines: vec!["l1".into(), "l2".into()],
            },
        );
        assert_eq!(f.title(), " Hover ");
        assert_eq!(f.row_text(1), "l2");
        f.view = FeatureView::Locations {
            title: "References".to_string(),
            items: vec![LocationEntry {
                uri: "file:///a.rs".into(),
                start_line: 9,
                start_character: 3,
                end_line: 9,
                end_character: 8,
            }],
        };
        assert_eq!(f.row_text(0), "file:///a.rs:10:4");
        assert!(f.selected_location().is_some());
        f.view = FeatureView::Symbols {
            items: vec![SymbolEntry {
                name: "inner".into(),
                kind: 6,
                container: Some("outer".into()),
                line: 2,
                character: 4,
            }],
        };
        assert_eq!(f.row_text(0), "outer :: inner");
        assert_eq!(f.selected_symbol().unwrap().name, "inner");
        // Flags surface in the detail line.
        let mut flagged = item("x");
        flagged.snippet = true;
        flagged.has_command = true;
        flagged.deprecated = true;
        flagged.documentation = Some("docs".into());
        flagged.kind = Some(3);
        flagged.detail = Some("fn x()".into());
        f.view = FeatureView::Completion {
            items: vec![flagged],
        };
        let detail = f.detail_line();
        assert!(detail.contains("snippet"), "{detail}");
        assert!(detail.contains("command"), "{detail}");
        assert!(detail.contains("deprecated"), "{detail}");
        assert!(f.row_text(0).contains("[3]"), "{}", f.row_text(0));
    }

    #[test]
    fn render_bounds_overlay_and_rows() {
        let dir = tempfile::tempdir().unwrap();
        let app = crate::app::App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        let mut f = LanguageFeatures::new(
            doc_id(),
            "u".into(),
            0,
            FeatureView::Completion {
                items: (0..30).map(|i| item(&format!("item{i}"))).collect(),
            },
        );
        use ratatui::{backend::TestBackend, Terminal};
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| f.render(&app, frame)).unwrap();
        assert!(f.area.width >= 20 && f.area.width <= 72);
        assert!(
            f.rows.len() <= LIST_HEIGHT as usize + 4,
            "{} rows",
            f.rows.len()
        );
        // A second draw after scrolling clamps nothing illegally.
        f.selected = 29;
        terminal.draw(|frame| f.render(&app, frame)).unwrap();
        assert!(f.scroll > 0);
        // Degenerate zero area must not panic.
        let mut terminal2 = Terminal::new(TestBackend::new(4, 4)).unwrap();
        terminal2.draw(|frame| f.render(&app, frame)).unwrap();

        // Text + locations + symbols views render too.
        for view in [
            FeatureView::Text {
                title: "t".into(),
                lines: vec!["x".into()],
            },
            FeatureView::Locations {
                title: "t".into(),
                items: vec![],
            },
            FeatureView::Symbols { items: vec![] },
        ] {
            let mut v = LanguageFeatures::new(doc_id(), "u".into(), 0, view);
            terminal.draw(|frame| v.render(&app, frame)).unwrap();
        }
    }
}
