//! Pure workspace breadcrumbs and document-aware presentation.
use std::path::{Path, PathBuf};

use ratatui::{buffer::Buffer, layout::Rect, style::Style, widgets::Widget};

use crate::theme::ThemeColors;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// In-memory presentation signals supplied by the document owner, never by I/O.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DocumentIndicators {
    pub dirty: bool,
    pub read_only: bool,
    pub external_change: bool,
    pub temporary: bool,
}

impl DocumentIndicators {
    pub fn markers(self) -> String {
        format!(
            "{}{}{}{}",
            if self.dirty { "*" } else { "" },
            if self.external_change { "!" } else { "" },
            if self.read_only { "[RO]" } else { "" },
            if self.temporary { "[preview]" } else { "" },
        )
    }
}

/// Cursor coordinates are explicitly one-based display coordinates supplied by
/// the editor. Language, encoding and ending describe the loaded buffer.
#[derive(Debug, Clone, Copy)]
pub struct DocumentStatus<'a> {
    pub path: &'a str,
    pub line: usize,
    pub column: usize,
    pub language: &'a str,
    pub encoding: &'a str,
    pub line_ending: &'a str,
    pub indicators: DocumentIndicators,
}

pub(crate) fn display_text(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

pub(crate) fn prefix_cells(text: &str, width: usize) -> String {
    let text = display_text(text);
    let mut result = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        let cells = grapheme.width();
        if used + cells > width || width == 0 {
            break;
        }
        if cells > 0 || !result.is_empty() {
            result.push_str(grapheme);
            used += cells;
        }
    }
    result
}

/// Keep the basename/end of a long path; ellipsis itself occupies one cell.
pub(crate) fn tail_cells(text: &str, width: usize) -> String {
    let text = display_text(text);
    if text.width() <= width {
        return prefix_cells(&text, width);
    }
    if width == 0 {
        return String::new();
    }
    let mut used = 1;
    let mut tail = Vec::new();
    for grapheme in text.graphemes(true).rev() {
        let cells = grapheme.width();
        if used + cells > width {
            break;
        }
        tail.push(grapheme);
        used += cells;
    }
    format!("…{}", tail.into_iter().rev().collect::<String>())
}

#[derive(Debug, Clone)]
pub struct BreadcrumbHit {
    pub path: PathBuf,
    pub label: String,
    pub area: Rect,
}

/// One-row geometry; its segment rectangles are the navigation hit authority.
#[derive(Debug, Clone, Default)]
pub struct BreadcrumbLayout {
    pub area: Rect,
    pub hits: Vec<BreadcrumbHit>,
    pub hidden_before: bool,
    separator: &'static str,
}

impl BreadcrumbLayout {
    /// Plain mode uses ASCII separators and never depends on an icon font.
    pub fn new(path: &Path, area: Rect, plain: bool) -> Self {
        let mut layout = Self {
            area,
            separator: if plain { " / " } else { " › " },
            ..Self::default()
        };
        if area.is_empty() {
            return layout;
        }
        let mut prefix = PathBuf::new();
        let segments: Vec<_> = path
            .components()
            .map(|component| {
                prefix.push(component);
                let raw = component.as_os_str().to_string_lossy();
                let label = prefix_cells(&raw, display_text(&raw).width());
                // Normalize invisible components before selecting the suffix:
                // the fallback is a real cell, not a post-packing decoration.
                let label = if label.is_empty() || raw.chars().all(char::is_control) {
                    "*".into()
                } else {
                    label
                };
                (prefix.clone(), label)
            })
            .collect();
        if segments.is_empty() {
            return layout;
        }
        let mut start = segments.len() - 1;
        let mut used = segments[start].1.width();
        while start > 0 {
            let candidate = used + layout.separator.width() + segments[start - 1].1.width();
            let marker = usize::from(start > 1) * 2;
            if candidate + marker > area.width as usize {
                break;
            }
            used = candidate;
            start -= 1;
        }
        layout.hidden_before = start > 0;
        let mark = if layout.hidden_before && area.width >= 3 {
            2
        } else {
            0
        };
        let mut x = area.x + mark;
        for (index, (path, label)) in segments[start..].iter().enumerate() {
            if index > 0 {
                x += layout.separator.width() as u16;
            }
            let remaining = area.right().saturating_sub(x);
            let label = tail_cells(label, remaining as usize);
            let width = label.width() as u16;
            layout.hits.push(BreadcrumbHit {
                path: path.clone(),
                label,
                area: Rect::new(x, area.y, width, 1),
            });
            x += width;
        }
        layout
    }

    pub fn hit(&self, x: u16, y: u16) -> Option<&Path> {
        self.hits
            .iter()
            .find(|h| h.area.contains((x, y).into()))
            .map(|h| h.path.as_path())
    }

    pub fn widget<'a>(&'a self, theme: &'a ThemeColors) -> BreadcrumbWidget<'a> {
        BreadcrumbWidget {
            layout: self,
            theme,
        }
    }
}

pub struct BreadcrumbWidget<'a> {
    layout: &'a BreadcrumbLayout,
    theme: &'a ThemeColors,
}

impl Widget for BreadcrumbWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = area.intersection(self.layout.area).intersection(buf.area);
        if area.is_empty() || area.y != self.layout.area.y {
            return;
        }
        let style = Style::default()
            .fg(self.theme.status_fg)
            .bg(self.theme.status_bg);
        buf.set_stringn(
            area.x,
            area.y,
            " ".repeat(area.width as usize),
            area.width as usize,
            style,
        );
        if self.layout.hidden_before && self.layout.area.width >= 3 && area.x == self.layout.area.x
        {
            buf.set_stringn(area.x, area.y, "… ", area.width as usize, style);
        }
        for (index, hit) in self.layout.hits.iter().enumerate() {
            if index > 0 {
                let x = hit
                    .area
                    .x
                    .saturating_sub(self.layout.separator.width() as u16);
                if x >= area.x && hit.area.x <= area.right() {
                    buf.set_stringn(
                        x,
                        area.y,
                        self.layout.separator,
                        (area.right() - x) as usize,
                        style,
                    );
                }
            }
            // Never shift geometry when the render viewport differs from layout.
            if hit.area.x >= area.x && hit.area.x < area.right() {
                let label = prefix_cells(
                    &hit.label,
                    (hit.area.right().min(area.right()) - hit.area.x) as usize,
                );
                buf.set_stringn(
                    hit.area.x,
                    area.y,
                    label,
                    area.right().saturating_sub(hit.area.x) as usize,
                    style,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn grapheme_clipping_preserves_cjk_combining_and_emoji_cells() {
        assert_eq!(prefix_cells("界e\u{301}👩‍💻z", 3), "界e\u{301}");
        assert_eq!(prefix_cells("👩‍💻z", 1), "");
        assert_eq!(tail_cells("prefix/界e\u{301}👩‍💻z", 6), "…e\u{301}👩‍💻z");
        assert_eq!(prefix_cells("a\nb\tc", 5), "a b c");
        assert_eq!(prefix_cells("abc", 0), "");
        assert_eq!(tail_cells("abc", 1), "…");
    }

    #[test]
    fn markers_are_explicit_without_color_or_icons() {
        assert_eq!(DocumentIndicators::default().markers(), "");
        assert_eq!(
            DocumentIndicators {
                dirty: true,
                read_only: true,
                external_change: true,
                temporary: true,
            }
            .markers(),
            "*![RO][preview]"
        );
    }

    #[test]
    fn breadcrumb_keeps_filename_and_bounded_navigation_for_every_size() {
        let theme = crate::theme::dark_theme();
        for (width, height) in [(120, 40), (80, 24), (60, 20)] {
            for plain in [true, false] {
                let area = Rect::new(3, 2, width - 6, 1);
                let layout = BreadcrumbLayout::new(
                    Path::new("/workspace/very-long-parent/配置/e\u{301}👩‍💻/config.yaml"),
                    area,
                    plain,
                );
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| frame.render_widget(layout.widget(&theme), area))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
                assert!(text.contains("config.yaml"), "{text}");
                assert!(text.contains(if plain { "/" } else { "›" }));
                for hit in &layout.hits {
                    assert!(area.contains((hit.area.x, hit.area.y).into()));
                    assert!(hit.area.right() <= area.right());
                    assert_eq!(layout.hit(hit.area.x, hit.area.y), Some(hit.path.as_path()));
                }
                assert_eq!(layout.hit(area.x, area.y + 1), None);
                assert_eq!(buffer[(area.x, area.y)].bg, theme.status_bg);
            }
        }
        for width in 0..20 {
            let area = Rect::new(2, 3, width, 1);
            let layout = BreadcrumbLayout::new(Path::new("/long-parent/config.yaml"), area, true);
            if width > 0 {
                assert_eq!(
                    layout.hits.last().unwrap().path,
                    Path::new("/long-parent/config.yaml")
                );
            } else {
                assert!(layout.hits.is_empty());
            }
            let mut buffer = Buffer::empty(area);
            layout.widget(&theme).render(area, &mut buffer);
        }
    }

    #[test]
    fn breadcrumb_snapshot_uses_display_cells_and_excludes_separator_hits() {
        let theme = crate::theme::light_theme();
        let area = Rect::new(2, 3, 40, 1);
        let layout = BreadcrumbLayout::new(Path::new("界/e\u{301}👩‍💻/config.yaml"), area, true);
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal
            .draw(|frame| frame.render_widget(layout.widget(&theme), area))
            .unwrap();
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(2, 3)].symbol(), "界");
        assert_eq!(buf[(7, 3)].symbol(), "e\u{301}");
        assert_eq!(buf[(8, 3)].symbol(), "👩‍💻");
        assert_eq!(buf[(13, 3)].symbol(), "c");
        assert_eq!(layout.hit(4, 3), None);
        assert_eq!(layout.hit(10, 3), None);
        assert_eq!(
            layout.hit(13, 3),
            Some(Path::new("界/e\u{301}👩‍💻/config.yaml"))
        );
        assert_eq!(layout.hit(24, 3), None);
        assert!(!layout.hidden_before);
    }

    #[test]
    fn empty_root_and_clipped_breadcrumb_rectangles_remain_bounded() {
        let theme = crate::theme::dark_theme();
        for area in [
            Rect::default(),
            Rect::new(2, 3, 10, 0),
            Rect::new(2, 3, 1, 1),
        ] {
            let layout = BreadcrumbLayout::new(Path::new(""), area, true);
            assert!(layout.hits.is_empty());
            layout.widget(&theme).render(area, &mut Buffer::empty(area));
        }
        let area = Rect::new(2, 3, 1, 1);
        let root = BreadcrumbLayout::new(Path::new("/"), area, true);
        assert_eq!(root.hit(2, 3), Some(Path::new("/")));
        let mut buf = Buffer::empty(area);
        root.widget(&theme).render(area, &mut buf);
        assert_eq!(buf[(2, 3)].symbol(), "/");
        let layout = BreadcrumbLayout::new(
            Path::new("/long-parent/config.yaml"),
            Rect::new(2, 3, 15, 2),
            false,
        );
        let away = Rect::new(0, 0, 1, 1);
        let mut buf = Buffer::empty(away);
        layout.widget(&theme).render(away, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), " ");
        let second_row = Rect::new(2, 4, 15, 1);
        let mut buf = Buffer::empty(second_row);
        layout.widget(&theme).render(second_row, &mut buf);
        assert!(buf.content.iter().all(|c| c.symbol() == " "));
        let clip = Rect::new(4, 3, 4, 1);
        let mut buf = Buffer::empty(clip);
        layout.widget(&theme).render(clip, &mut buf);
        assert_eq!(buf[(4, 3)].symbol(), "c");
        assert_eq!(buf[(7, 3)].symbol(), "f");
        assert_eq!(tail_cells("界", 0), "");
        assert_eq!(tail_cells("界", 2), "界");
        assert_eq!(prefix_cells("\u{301}x", 1), "x");
    }

    #[test]
    fn combining_only_breadcrumb_components_are_budgeted_before_packing() {
        let path = Path::new("\u{301}/\u{301}");
        let area = Rect::new(2, 3, 3, 1);
        let layout = BreadcrumbLayout::new(path, area, true);
        assert_eq!(layout.hits.len(), 1, "{:?}", layout.hits);
        assert_eq!(layout.hits[0].path, path);
        assert_eq!(layout.hits[0].area, Rect::new(4, 3, 1, 1));
        assert_eq!(layout.hit(4, 3), Some(path));
        assert_eq!(layout.hit(2, 3), None);
        let theme = crate::theme::dark_theme();
        let mut terminal = Terminal::new(TestBackend::new(8, 6)).unwrap();
        terminal
            .draw(|frame| frame.render_widget(layout.widget(&theme), area))
            .unwrap();
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(2, 3)].symbol(), "…");
        assert_eq!(buf[(4, 3)].symbol(), "*");
    }

    #[test]
    fn invisible_breadcrumb_variants_keep_final_component_visible_and_hits_bounded() {
        let theme = crate::theme::dark_theme();
        for path in [
            "\u{301}",
            "\u{301}/\u{301}",
            "\u{301}/\u{301}/\u{301}",
            "\n",
            "\n/\t",
            "\n/\t/\r",
            "\u{301}/\n/\u{301}",
            "\n/\u{301}/\t",
        ] {
            for plain in [true, false] {
                for width in 0..16 {
                    let area = Rect::new(2, 3, width, 1);
                    let layout = BreadcrumbLayout::new(Path::new(path), area, plain);
                    let mut terminal = Terminal::new(TestBackend::new(20, 8)).unwrap();
                    terminal
                        .draw(|frame| frame.render_widget(layout.widget(&theme), area))
                        .unwrap();
                    let buf = terminal.backend().buffer();
                    if width == 0 {
                        assert!(layout.hits.is_empty());
                        continue;
                    }
                    let last = layout.hits.last().unwrap();
                    assert_eq!(last.path, Path::new(path), "{path:?}, {width}");
                    assert_eq!(last.label, "*", "{path:?}, {width}");
                    assert_eq!(buf[(last.area.x, last.area.y)].symbol(), "*");
                    for hit in &layout.hits {
                        assert!(hit.area.width > 0, "{path:?}, {width}: {hit:?}");
                        assert!(
                            area.contains((hit.area.x, hit.area.y).into()),
                            "{path:?}, {width}: {hit:?}"
                        );
                        assert!(
                            hit.area.right() <= area.right(),
                            "{path:?}, {width}: {hit:?}"
                        );
                        assert_eq!(layout.hit(hit.area.x, hit.area.y), Some(hit.path.as_path()));
                    }
                    assert_eq!(layout.hit(area.right(), area.y), None);
                    assert_eq!(layout.hit(last.area.x, area.y + 1), None);
                }
            }
        }
    }
}
