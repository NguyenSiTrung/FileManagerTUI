//! Bounded document chrome; layout is also the mouse hit-test authority.
use super::workspace_chrome::{display_text, prefix_cells};
use crate::theme::ThemeColors;
use crate::workspace::documents::{DocumentId, DocumentStore};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    widgets::Widget,
};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone)]
pub struct DocumentSummary {
    pub id: DocumentId,
    pub path: std::path::PathBuf,
    pub label: String,
    pub active: bool,
    pub temporary: bool,
}
pub fn summaries(store: &DocumentStore) -> Vec<DocumentSummary> {
    summaries_with_read_only(store, |_| false)
}

/// Explicit read-only state supplied by the document owner, never polled here.
pub fn summaries_with_read_only(
    store: &DocumentStore,
    read_only: impl Fn(DocumentId) -> bool,
) -> Vec<DocumentSummary> {
    let docs: Vec<_> = store.iter().collect();
    docs.iter()
        .map(|d| {
            let title = d.title();
            let name = if docs.iter().filter(|other| other.title() == title).count() > 1 {
                let parent: Vec<_> = d.path().parent().unwrap().components().collect();
                let suffix = |path: &std::path::Path, count: usize| {
                    let parts: Vec<_> = path.parent().unwrap().components().collect();
                    parts[parts.len().saturating_sub(count)..]
                        .iter()
                        .collect::<std::path::PathBuf>()
                };
                let count = (1..=parent.len())
                    .find(|&n| {
                        docs.iter()
                            .filter(|other| other.title() == title && other.id() != d.id())
                            .all(|other| suffix(other.path(), n) != suffix(d.path(), n))
                    })
                    .unwrap_or(parent.len());
                format!("{title} — {}", suffix(d.path(), count).display())
            } else {
                title
            };
            DocumentSummary {
                id: d.id(),
                path: d.path().to_owned(),
                label: format!(
                    "{}{}{}{} {}",
                    name,
                    if d.editor.modified { "*" } else { "" },
                    if d.has_external_change() { "!" } else { "" },
                    if read_only(d.id()) { "[RO]" } else { "" },
                    if d.is_pinned() { "[pin]" } else { "[preview]" }
                ),
                active: store.active_id() == Some(d.id()),
                temporary: !d.is_pinned(),
            }
        })
        .collect()
}

/// Keyboard navigation visits the complete order, including overflowed tabs.
pub fn adjacent_document(
    tabs: &[DocumentSummary],
    current: Option<DocumentId>,
    backward: bool,
) -> Option<DocumentId> {
    if tabs.is_empty() {
        return None;
    }
    let index = current.and_then(|id| tabs.iter().position(|tab| tab.id == id));
    let next = match index {
        Some(index) if backward => (index + tabs.len() - 1) % tabs.len(),
        Some(index) => (index + 1) % tabs.len(),
        None if backward => tabs.len() - 1,
        None => 0,
    };
    Some(tabs[next].id)
}
#[derive(Debug, Clone)]
pub struct TabHit {
    pub id: DocumentId,
    pub area: Rect,
    pub label: String,
    pub active: bool,
    pub temporary: bool,
}
#[derive(Debug, Clone, Default)]
pub struct TabLayout {
    pub area: Rect,
    pub hits: Vec<TabHit>,
    pub hidden_before: bool,
    pub hidden_after: bool,
}
impl TabLayout {
    pub fn widget<'a>(&'a self, theme: &'a ThemeColors, plain: bool) -> TabWidget<'a> {
        TabWidget {
            layout: self,
            theme,
            plain,
        }
    }

    pub fn new(tabs: &[DocumentSummary], area: Rect) -> Self {
        let mut layout = Self {
            area,
            ..Self::default()
        };
        if area.width == 0 || area.height == 0 || tabs.is_empty() {
            return layout;
        }
        let active = tabs.iter().position(|t| t.active).unwrap_or(0);
        // Reserve overflow marks only when at least one cell remains for a tab.
        let marks = u16::from(area.width >= 3 && tabs.len() > 1);
        let available = area.width.saturating_sub(2 * marks);
        let width = |t: &DocumentSummary| {
            (display_text(&t.label).width().saturating_add(2).min(64) as u16)
                .min(available)
                .max(1)
        };
        let mut start = active;
        let mut used = width(&tabs[active]);
        while start > 0 && used.saturating_add(width(&tabs[start - 1])) <= available {
            start -= 1;
            used += width(&tabs[start]);
        }
        let mut x = area.x.saturating_add(marks);
        for t in &tabs[start..] {
            let w = width(t);
            if x.saturating_add(w) > area.right().saturating_sub(marks) {
                break;
            }
            let title = display_text(
                &t.path
                    .file_name()
                    .unwrap_or(t.path.as_os_str())
                    .to_string_lossy(),
            );
            layout.hits.push(TabHit {
                id: t.id,
                area: Rect::new(x, area.y, w, 1),
                // The owned basename identifies the summary boundary here;
                // TabHit intentionally keeps its compatible public shape.
                label: fit_label(&t.label, w as usize, Some(&title)),
                active: t.active,
                temporary: t.temporary,
            });
            x = x.saturating_add(w);
        }
        layout.hidden_before = start > 0;
        layout.hidden_after = start + layout.hits.len() < tabs.len();
        layout
    }
    pub fn hit(&self, x: u16, y: u16) -> Option<DocumentId> {
        self.hits
            .iter()
            .find(|h| h.area.contains((x, y).into()))
            .map(|h| h.id)
    }
}

pub struct TabWidget<'a> {
    layout: &'a TabLayout,
    theme: &'a ThemeColors,
    plain: bool,
}

impl Widget for TabWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.layout
            .render_chrome(area, buf, Some(self.theme), self.plain);
    }
}
impl Widget for &TabLayout {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.render_chrome(area, buf, None, false);
    }
}

/// Preserve distinguishing parents and state when a title needs clipping.
fn fit_label(label: &str, width: usize, owned_title: Option<&str>) -> String {
    let label = display_text(label);
    if label.width() <= width {
        return prefix_cells(&label, width);
    }
    let (mut body, pin) = if let Some(body) = label.strip_suffix(" [preview]") {
        (body, " [preview]")
    } else if let Some(body) = label.strip_suffix(" [pin]") {
        (body, " [pin]")
    } else {
        return prefix_cells(&label, width);
    };
    let mut flags = String::new();
    for marker in ["[RO]", "!", "*"] {
        if let Some(rest) = body.strip_suffix(marker) {
            body = rest;
            flags.insert_str(0, marker);
        }
    }
    // A separator is metadata only immediately after the complete owned
    // basename. Delimiters inside either filename or parent remain path text.
    let duplicate = owned_title.and_then(|title| {
        body.strip_prefix(title)
            .and_then(|rest| rest.strip_prefix(" — "))
            .map(|parent| (title, parent))
    });
    // Temporary/pinned is secondary to critical state and parent context.
    let minimum_context = duplicate.map_or(2, |(_, parent)| {
        // Do not retain a secondary pin marker at the cost of the first
        // distinguishing parent component when that component can fit.
        4 + parent
            .split(std::path::MAIN_SEPARATOR)
            .next()
            .unwrap_or(parent)
            .width()
            .max(1)
    });
    if flags.width() + pin.width() + minimum_context <= width {
        flags.push_str(pin);
    }
    let flags = prefix_cells(&flags, width);
    let budget = width.saturating_sub(flags.width());
    if budget == 0 {
        return flags;
    }
    if let Some((title, parent)) = duplicate {
        // The summary's suffix starts with the distinguishing parent fragment.
        // Reserve it before allocating cells to the possibly huge basename.
        let parent = prefix_cells(parent, budget.saturating_sub(4));
        if !parent.is_empty() {
            let title_budget = budget.saturating_sub(parent.width() + 4);
            return format!(
                "{}… — {}{}",
                prefix_cells(title, title_budget),
                parent,
                flags
            );
        }
    }
    format!("{}…{}", prefix_cells(body, budget - 1), flags)
}

impl TabLayout {
    fn render_chrome(
        &self,
        area: Rect,
        buf: &mut Buffer,
        theme: Option<&ThemeColors>,
        plain: bool,
    ) {
        let area = area.intersection(self.area).intersection(buf.area);
        if area.is_empty() || area.y != self.area.y {
            return;
        }
        let base = theme.map_or(Style::default(), |t| {
            Style::default().fg(t.status_fg).bg(t.status_bg)
        });
        buf.set_stringn(
            area.x,
            area.y,
            " ".repeat(area.width as usize),
            area.width as usize,
            base,
        );
        if self.hidden_before
            && self.area.width >= 3
            && area.contains((self.area.x, self.area.y).into())
        {
            buf.set_string(self.area.x, area.y, "<", base);
        }
        if self.hidden_after
            && self.area.width >= 3
            && area.contains((self.area.right() - 1, self.area.y).into())
        {
            buf.set_string(self.area.right() - 1, area.y, ">", base);
        }
        for hit in &self.hits {
            if hit.area.x < area.x || hit.area.x >= area.right() {
                continue;
            }
            let width = hit.area.width.min(area.right() - hit.area.x) as usize;
            let mut style = base;
            if hit.active {
                style = match theme {
                    Some(t) => style
                        .fg(t.tree_selected_fg)
                        .bg(t.tree_selected_bg)
                        .add_modifier(Modifier::BOLD),
                    None => style.add_modifier(Modifier::REVERSED | Modifier::BOLD),
                };
            }
            if hit.temporary {
                style = style.add_modifier(Modifier::ITALIC);
            }
            buf.set_stringn(hit.area.x, area.y, " ".repeat(width), width, style);
            let label = if hit.area.width == 1 {
                "*".to_string()
            } else {
                // Layout already fitted this label using the owned path.
                // A clipped render viewport must not re-parse display text.
                prefix_cells(&hit.label, width)
            };
            // Substitute only after clipping so every adapter recognizes the
            // same distinguishing suffix; both separators occupy one cell.
            let label = if plain {
                label.replace('—', "-")
            } else {
                label
            };
            buf.set_stringn(hit.area.x, area.y, label, width, style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::documents::OpenDisposition;
    fn store() -> (tempfile::TempDir, DocumentStore) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = DocumentStore::new();
        for name in ["one/same.txt", "two/same.txt", "三.txt"] {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "hello").unwrap();
            store.open(&path, OpenDisposition::Pinned).unwrap();
        }
        (dir, store)
    }
    #[test]
    fn duplicate_names_have_owned_path_labels_and_indicators() {
        let (_dir, mut store) = store();
        let id = store.iter().next().unwrap().id();
        store.get_mut(id).unwrap().editor.insert_char('x');
        store.mark_external_change(id).unwrap();
        let tabs = summaries(&store);
        assert_ne!(tabs[0].label, tabs[1].label);
        assert!(tabs[0].label.starts_with("same.txt"));
        assert!(tabs[0].label.contains("one"));
        assert!(tabs[1].label.contains("two"));
        assert!(tabs[0].label.contains('*'));
        assert!(tabs[0].label.contains('!'));
        assert!(tabs[0].label.contains("[pin]"));
    }
    #[test]
    fn overflow_active_visible_tiny_and_hits_are_bounded() {
        let (_dir, mut store) = store();
        let ids: Vec<_> = store.iter().map(|d| d.id()).collect();
        for id in ids {
            store.activate(id).unwrap();
            let tabs = summaries(&store);
            for width in 1..80 {
                let layout = TabLayout::new(&tabs, Rect::new(7, 9, width, 1));
                assert!(layout
                    .hits
                    .iter()
                    .any(|h| h.id == store.active_id().unwrap()));
                for h in &layout.hits {
                    assert!(h.area.width > 0);
                    assert!(h.area.right() <= layout.area.right());
                    assert_eq!(layout.hit(h.area.x, 9), Some(h.id));
                }
                assert_eq!(layout.hit(7, 10), None);
                let mut buf = Buffer::empty(layout.area);
                (&layout).render(layout.area, &mut buf);
                if width >= 3 && layout.hidden_before {
                    assert_eq!(buf[(7, 9)].symbol(), "<");
                }
                if width >= 3 && layout.hidden_after {
                    assert_eq!(buf[(layout.area.right() - 1, 9)].symbol(), ">");
                }
            }
            assert!(TabLayout::new(&tabs, Rect::new(0, 0, 0, 1)).hits.is_empty());
            assert!(TabLayout::new(&tabs, Rect::new(0, 0, 10, 0))
                .hits
                .is_empty());
            assert!(TabLayout::new(&[], Rect::new(0, 0, 10, 1)).hits.is_empty());
        }
    }
    #[test]
    fn tiny_unicode_active_tab_is_visibly_marked_and_empty_render_safe() {
        let (_dir, store) = store();
        let tabs = summaries(&store);
        let area = Rect::new(0, 0, 1, 1);
        let layout = TabLayout::new(&tabs, area);
        let mut buf = Buffer::empty(area);
        (&layout).render(area, &mut buf);
        assert_ne!(buf[(0, 0)].symbol(), " ");
        let empty = Rect::default();
        (&TabLayout::new(&tabs, empty)).render(empty, &mut Buffer::empty(empty));
    }

    #[test]
    fn temporary_preview_label_and_render() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a");
        std::fs::write(&p, "a").unwrap();
        let mut store = DocumentStore::new();
        store.open(&p, OpenDisposition::Preview).unwrap();
        let tabs = summaries(&store);
        assert!(tabs[0].label.contains("[preview]"));
        let area = Rect::new(0, 0, 60, 1);
        let layout = TabLayout::new(&tabs, area);
        let mut buf = Buffer::empty(area);
        (&layout).render(area, &mut buf);
        assert!(buf.content.iter().any(|cell| cell.symbol() == "a"));
    }

    #[test]
    fn render_outside_layout_does_not_paint_or_panic() {
        let (_dir, store) = store();
        let layout = TabLayout::new(&summaries(&store), Rect::new(7, 9, 30, 1));
        let area = Rect::new(0, 0, 5, 1);
        let mut buf = Buffer::empty(area);
        (&layout).render(area, &mut buf);
        assert!(buf.content.iter().all(|c| c.symbol() == " "));
    }

    #[test]
    fn explicit_read_only_and_keyboard_navigation_include_hidden_documents() {
        let (_dir, mut store) = store();
        let ids: Vec<_> = store.iter().map(|d| d.id()).collect();
        store.activate(ids[0]).unwrap();
        let tabs = summaries_with_read_only(&store, |id| id == ids[0]);
        assert!(tabs[0].label.contains("[RO]"));
        assert!(!tabs[1].label.contains("[RO]"));
        let layout = TabLayout::new(&tabs, Rect::new(2, 3, 10, 1));
        assert!(layout.hidden_after);
        assert_eq!(layout.hits.len(), 1);
        assert_eq!(adjacent_document(&tabs, Some(ids[0]), false), Some(ids[1]));
        assert_eq!(adjacent_document(&tabs, Some(ids[0]), true), Some(ids[2]));
        assert_eq!(adjacent_document(&tabs, Some(ids[2]), false), Some(ids[0]));
        assert_eq!(adjacent_document(&tabs, None, false), Some(ids[0]));
        assert_eq!(adjacent_document(&tabs, None, true), Some(ids[2]));
        assert_eq!(adjacent_document(&[], None, false), None);
    }

    #[test]
    fn themed_tabs_snapshot_active_inactive_and_plain_variants() {
        use ratatui::{backend::TestBackend, Terminal};
        let (_dir, mut store) = store();
        let ids: Vec<_> = store.iter().map(|d| d.id()).collect();
        store.activate(ids[0]).unwrap();
        store.get_mut(ids[0]).unwrap().editor.insert_char('x');
        store.mark_external_change(ids[0]).unwrap();
        let tabs = summaries_with_read_only(&store, |id| id == ids[0]);
        for theme in [crate::theme::dark_theme(), crate::theme::light_theme()] {
            for (width, height) in [(120, 40), (80, 24), (60, 20)] {
                for plain in [false, true] {
                    let area = Rect::new(3, 2, width - 6, 1);
                    let layout = TabLayout::new(&tabs, area);
                    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                    terminal
                        .draw(|frame| frame.render_widget(layout.widget(&theme, plain), area))
                        .unwrap();
                    let buf = terminal.backend().buffer();
                    let text: String = buf.content.iter().map(|c| c.symbol()).collect();
                    assert!(text.contains("same.txt"));
                    assert!(text.contains("*!"));
                    assert!(text.contains("[RO]"));
                    assert!(text.contains("one"));
                    assert_eq!(buf[(area.x, area.y)].bg, theme.status_bg);
                    let active = layout.hits.iter().find(|h| h.active).unwrap();
                    assert_eq!(buf[(active.area.x, area.y)].bg, theme.tree_selected_bg);
                    assert!(buf[(active.area.x, area.y)]
                        .modifier
                        .contains(Modifier::BOLD));
                    if let Some(inactive) = layout.hits.iter().find(|h| !h.active) {
                        assert_eq!(buf[(inactive.area.x, area.y)].fg, theme.status_fg);
                    }
                    if plain {
                        assert!(!text.contains('—'));
                    }
                    for hit in &layout.hits {
                        assert_eq!(layout.hit(hit.area.x, area.y), Some(hit.id));
                        assert_eq!(layout.hit(hit.area.right() - 1, area.y), Some(hit.id));
                    }
                    assert_eq!(layout.hit(area.x, area.y), None);
                }
            }
        }
    }

    #[test]
    fn long_unicode_tab_keeps_state_markers_and_grapheme_cells() {
        use ratatui::{backend::TestBackend, Terminal};
        let (_dir, store) = store();
        let id = store.iter().next().unwrap().id();
        let tabs = vec![DocumentSummary {
            id,
            path: "界e\u{301}👩‍💻/config.yaml".into(),
            label: format!("界e\u{301}👩‍💻{}*![RO] [preview]", "long".repeat(30)),
            active: true,
            temporary: true,
        }];
        let theme = crate::theme::dark_theme();
        let area = Rect::new(3, 2, 30, 1);
        let layout = TabLayout::new(&tabs, area);
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal
            .draw(|frame| frame.render_widget(layout.widget(&theme, true), area))
            .unwrap();
        let buf = terminal.backend().buffer();
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("*![RO] [preview]"), "{text}");
        assert_eq!(buf[(3, 2)].symbol(), "界");
        assert_eq!(buf[(5, 2)].symbol(), "e\u{301}");
        assert_eq!(buf[(6, 2)].symbol(), "👩‍💻");
        assert!(buf[(3, 2)].modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn truncation_prioritizes_changed_state_before_preview_and_pinned_labels() {
        for (label, width, expected) in [
            ("abcdefghijklmnopqrstuvwxyz*![RO] [preview]", 6, "*![RO]"),
            ("abcdefghijklmnopqrstuvwxyz*![RO] [preview]", 8, "a…*![RO]"),
            ("abcdefghijklmnopqrstuvwxyz*![RO] [preview]", 2, "*!"),
            ("abcdefghijklmnopqrstuvwxyz [pin]", 8, "a… [pin]"),
            ("custom界e\u{301}👩‍💻", 8, "custom界"),
            ("a [preview]", 20, "a [preview]"),
        ] {
            assert_eq!(fit_label(label, width, None), expected);
        }
        let (_dir, store) = store();
        let mut tabs = summaries(&store);
        for tab in &mut tabs {
            tab.active = false;
        }
        let layout = TabLayout::new(&tabs, Rect::new(3, 2, 80, 2));
        assert_eq!(layout.hits[0].id, tabs[0].id);
        let second_row = Rect::new(3, 3, 80, 1);
        let mut buf = Buffer::empty(second_row);
        (&layout).render(second_row, &mut buf);
        assert!(buf.content.iter().all(|c| c.symbol() == " "));
        let clip = Rect::new(8, 2, 5, 1);
        let mut buf = Buffer::empty(clip);
        (&layout).render(clip, &mut buf);
        assert!(buf.content.iter().all(|c| c.symbol() == " "));
    }

    #[test]
    fn duplicate_long_basenames_keep_distinguishing_parents_in_rendered_tabs() {
        use ratatui::{backend::TestBackend, Terminal};
        for basename in [
            format!("{}.yaml", "configuration".repeat(6)),
            format!("{}.yaml", "界e\u{301}👩‍💻".repeat(10)),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut store = DocumentStore::new();
            for parent in ["one/shared", "two/shared"] {
                let path = dir.path().join(parent).join(&basename);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, "hello").unwrap();
                store.open(&path, OpenDisposition::Pinned).unwrap();
            }
            for changed in [false, true] {
                if changed {
                    let ids: Vec<_> = store.iter().map(|doc| doc.id()).collect();
                    for id in ids {
                        store.get_mut(id).unwrap().editor.insert_char('x');
                        store.mark_external_change(id).unwrap();
                    }
                }
                let tabs = summaries_with_read_only(&store, |_| changed);
                assert_ne!(tabs[0].label, tabs[1].label);
                let area = Rect::new(2, 3, 140, 1);
                let layout = TabLayout::new(&tabs, area);
                assert_eq!(layout.hits.len(), 2);
                assert!(layout.hits.iter().all(|hit| hit.area.width == 64));
                for adapter in 0..3 {
                    let theme = crate::theme::dark_theme();
                    let mut terminal = Terminal::new(TestBackend::new(144, 20)).unwrap();
                    terminal
                        .draw(|frame| {
                            if adapter == 0 {
                                frame.render_widget(&layout, area);
                            } else {
                                frame.render_widget(layout.widget(&theme, adapter == 2), area);
                            }
                        })
                        .unwrap();
                    let buffer = terminal.backend().buffer();
                    let rendered: Vec<String> = layout
                        .hits
                        .iter()
                        .map(|hit| {
                            (hit.area.x..hit.area.right())
                                .map(|x| buffer[(x, area.y)].symbol())
                                .collect()
                        })
                        .collect();
                    assert_ne!(rendered[0], rendered[1], "{rendered:?}");
                    assert!(rendered[0].contains("one/shared"), "{rendered:?}");
                    assert!(rendered[1].contains("two/shared"), "{rendered:?}");
                    for text in &rendered {
                        assert!(
                            text.contains(if changed { "*![RO]" } else { "[pin]" }),
                            "{text}"
                        );
                    }
                    if adapter == 2 {
                        assert!(rendered.iter().all(|text| !text.contains('—')));
                    }
                    for hit in &layout.hits {
                        assert_eq!(layout.hit(hit.area.x, area.y), Some(hit.id));
                        assert_eq!(layout.hit(hit.area.right() - 1, area.y), Some(hit.id));
                    }
                }
                if changed {
                    for (tab, parent) in tabs.iter().zip(["one", "two"]) {
                        for width in 1..70 {
                            let area = Rect::new(2, 3, width, 1);
                            let layout = TabLayout::new(std::slice::from_ref(tab), area);
                            let mut buf = Buffer::empty(area);
                            let theme = crate::theme::light_theme();
                            layout.widget(&theme, true).render(area, &mut buf);
                            let text: String =
                                buf.content.iter().map(|cell| cell.symbol()).collect();
                            if width >= 13 {
                                assert!(text.contains(parent), "{width}: {text}");
                                assert!(text.contains("*![RO]"), "{width}: {text}");
                            }
                            assert_eq!(layout.hits.len(), 1);
                            assert!(layout.hits[0].area.right() <= area.right());
                            assert_eq!(layout.hit(area.x, area.y), Some(tab.id));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn owned_paths_disambiguate_delimiter_containing_filenames_and_parents() {
        use ratatui::{backend::TestBackend, Terminal};
        for basename in [
            format!("{}.yaml", "configuration".repeat(6)),
            format!("{}.yaml", "configuration — Documents".repeat(4)),
            format!("{}.yaml", "界e\u{301}👩‍💻 — Notes".repeat(6)),
        ] {
            for parents in [
                ["Acme — Documents", "Beta — Documents"],
                [
                    "Acme — Documents/shared — Notes",
                    "Beta — Documents/shared — Notes",
                ],
                ["Acme — Documents", "Beta"],
            ] {
                let dir = tempfile::tempdir().unwrap();
                let mut store = DocumentStore::new();
                for parent in parents {
                    let path = dir.path().join(parent).join(&basename);
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::fs::write(&path, "hello").unwrap();
                    store.open(&path, OpenDisposition::Pinned).unwrap();
                }
                for changed in [false, true] {
                    if changed {
                        let ids: Vec<_> = store.iter().map(|doc| doc.id()).collect();
                        for id in ids {
                            store.get_mut(id).unwrap().editor.insert_char('x');
                            store.mark_external_change(id).unwrap();
                        }
                    }
                    let tabs = summaries_with_read_only(&store, |_| changed);
                    assert_ne!(tabs[0].label, tabs[1].label);
                    assert!(tabs[0].label.starts_with(&basename));
                    assert!(tabs[1].label.starts_with(&basename));
                    let area = Rect::new(2, 3, 140, 1);
                    let layout = TabLayout::new(&tabs, area);
                    assert_eq!(layout.hits.len(), 2);
                    assert!(layout.hits.iter().all(|hit| hit.area.width == 64));
                    for adapter in 0..3 {
                        let theme = crate::theme::dark_theme();
                        let mut terminal = Terminal::new(TestBackend::new(144, 20)).unwrap();
                        terminal
                            .draw(|frame| {
                                if adapter == 0 {
                                    frame.render_widget(&layout, area);
                                } else {
                                    frame.render_widget(layout.widget(&theme, adapter == 2), area);
                                }
                            })
                            .unwrap();
                        let buffer = terminal.backend().buffer();
                        let rendered: Vec<String> = layout
                            .hits
                            .iter()
                            .map(|hit| {
                                (hit.area.x..hit.area.right())
                                    .map(|x| buffer[(x, area.y)].symbol())
                                    .collect()
                            })
                            .collect();
                        assert_ne!(rendered[0], rendered[1], "{rendered:?}");
                        for (text, parent) in rendered.iter().zip(parents) {
                            let expected = if adapter == 2 {
                                parent.replace('—', "-")
                            } else {
                                parent.into()
                            };
                            assert!(text.contains(&expected), "missing {expected}: {text}");
                            assert!(
                                text.contains(if changed { "*![RO]" } else { "[pin]" }),
                                "{text}"
                            );
                            assert!(!text.contains('\n'));
                        }
                        assert_eq!(buffer[(1, 3)].symbol(), " ");
                        assert_eq!(buffer[(142, 3)].symbol(), " ");
                        for hit in &layout.hits {
                            assert_eq!(layout.hit(hit.area.x, area.y), Some(hit.id));
                            assert_eq!(layout.hit(hit.area.right() - 1, area.y), Some(hit.id));
                        }
                    }
                }
            }
        }
    }
}
