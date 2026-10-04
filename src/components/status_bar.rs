use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use super::workspace_chrome::{display_text, prefix_cells, tail_cells, DocumentStatus};
use crate::theme::ThemeColors;
use unicode_width::UnicodeWidthStr;

/// Status bar widget that displays file path, info, key hints, or status messages.
pub struct StatusBarWidget<'a> {
    path_str: &'a str,
    file_info: &'a str,
    theme: &'a ThemeColors,
    status_message: Option<&'a str>,
    is_error: bool,
    clipboard_info: Option<&'a str>,
    watcher_status: Option<&'a str>,
    diagnostics: Option<(&'a str, Color)>,
    key_hints: Option<&'a str>,
    git_branch: Option<&'a str>,
    document: Option<DocumentStatus<'a>>,
    reserved_right: u16,
}

impl<'a> StatusBarWidget<'a> {
    pub fn new(path_str: &'a str, file_info: &'a str, theme: &'a ThemeColors) -> Self {
        Self {
            path_str,
            file_info,
            theme,
            status_message: None,
            is_error: false,
            clipboard_info: None,
            watcher_status: None,
            diagnostics: None,
            key_hints: None,
            git_branch: None,
            document: None,
            reserved_right: 0,
        }
    }

    pub fn status_message(mut self, msg: &'a str, is_error: bool) -> Self {
        self.status_message = Some(msg);
        self.is_error = is_error;
        self
    }

    pub fn clipboard_info(mut self, info: &'a str) -> Self {
        self.clipboard_info = Some(info);
        self
    }

    pub fn watcher_status(mut self, status: &'a str) -> Self {
        self.watcher_status = Some(status);
        self
    }

    /// Severity summary ("E:2 W:1") in the worst severity's color — the
    /// caller picks; absent entirely when the workspace is clean.
    pub fn diagnostics(mut self, label: &'a str, color: Color) -> Self {
        self.diagnostics = Some((label, color));
        self
    }

    /// Read-only Git branch/detached/unborn label. Rendered with a dedicated
    /// color but always as text, so it stays legible with no colors.
    pub fn git_branch(mut self, branch: &'a str) -> Self {
        self.git_branch = Some(branch);
        self
    }

    pub fn key_hints(mut self, hints: &'a str) -> Self {
        self.key_hints = Some(hints);
        self
    }

    /// Explicit active-document context, independent of explorer selection.
    pub fn document_status(mut self, document: DocumentStatus<'a>) -> Self {
        self.document = Some(document);
        self
    }

    /// Leave these rightmost cells untouched for a separately rendered menu.
    pub fn reserved_right(mut self, cells: u16) -> Self {
        self.reserved_right = cells;
        self
    }
}

impl Widget for StatusBarWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Reserve against the caller's original rectangle, then clip to buffer.
        let area = Rect::new(
            area.x,
            area.y,
            area.width.saturating_sub(self.reserved_right),
            area.height,
        )
        .intersection(buf.area);
        if area.is_empty() {
            return;
        }
        let width = area.width as usize;
        let base = Style::default()
            .fg(self.theme.status_fg)
            .bg(self.theme.status_bg);
        buf.set_stringn(area.x, area.y, " ".repeat(width), width, base);
        if let Some(msg) = self.status_message {
            let style = if self.is_error {
                base.bg(self.theme.error_fg)
            } else {
                base.fg(self.theme.success_fg)
            };
            buf.set_style(Rect::new(area.x, area.y, area.width, 1), style);
            buf.set_stringn(area.x, area.y, prefix_cells(msg, width), width, style);
            return;
        }

        let (path, markers, info) = match self.document {
            Some(doc) => (
                display_text(doc.path),
                doc.indicators.markers(),
                display_text(&format!(
                    "Ln {} Col {} {} {} {}",
                    doc.line, doc.column, doc.line_ending, doc.language, doc.encoding
                )),
            ),
            None => (
                display_text(self.path_str),
                String::new(),
                display_text(self.file_info),
            ),
        };
        // Filename + textual state and editor metadata outrank all hints. Long
        // ancestors are expendable, the document owner is not a tree selection.
        let title = std::path::Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|| path.as_str().into());
        let minimum_path =
            (title.width() + markers.width() + usize::from(path.width() > title.width()))
                .min((width / 2).max(markers.width() + 1))
                .min(width);
        let separator = usize::from(!path.is_empty() && !info.is_empty());
        // Branch state outranks the discardable info detail: reserve it right
        // after the minimum path so a real repository always shows its branch
        // even when the path and editor metadata would otherwise fill the bar.
        // When space is scarce it truncates rather than silently vanishing.
        let branch_text = self
            .git_branch
            .map(|branch| display_text(&format!(" {branch} ")));
        let branch_budget = branch_text
            .as_ref()
            .map(|branch| branch.width().min(width.saturating_sub(minimum_path)))
            .unwrap_or(0);
        let info_budget = info
            .width()
            .min(width.saturating_sub(minimum_path + branch_budget + separator));
        let info = prefix_cells(&info, info_budget);
        let path_budget =
            width.saturating_sub(info.width() + branch_budget + usize::from(!info.is_empty()));
        let marker_display = prefix_cells(&markers, path_budget);
        let path_display = format!(
            "{}{}",
            tail_cells(&path, path_budget.saturating_sub(marker_display.width())),
            marker_display
        );

        let branch_span = branch_text.map(|branch| {
            Span::styled(
                prefix_cells(&branch, branch_budget),
                base.fg(self.theme.git_branch_fg)
                    .add_modifier(Modifier::BOLD),
            )
        });
        let core_used = path_display.width()
            + info.width()
            + branch_budget
            + usize::from(!path_display.is_empty() && !info.is_empty());
        let mut remaining = width.saturating_sub(core_used);
        let mut extras = Vec::new();
        for (text, style) in [
            (
                self.clipboard_info,
                base.fg(self.theme.accent_fg).add_modifier(Modifier::BOLD),
            ),
            (
                self.watcher_status,
                base.fg(self.theme.warning_fg).add_modifier(Modifier::BOLD),
            ),
            (
                self.diagnostics.map(|(label, _)| label),
                base.fg(self
                    .diagnostics
                    .map_or(self.theme.error_fg, |(_, color)| color))
                    .add_modifier(Modifier::BOLD),
            ),
            (
                Some(self.key_hints.unwrap_or(if self.document.is_some() {
                    ""
                } else {
                    " y:cp x:cut p:paste Y:path o:open "
                })),
                base.fg(self.theme.dim_fg).add_modifier(Modifier::DIM),
            ),
        ] {
            if let Some(text) = text {
                let text = display_text(text);
                if !text.is_empty() && text.width() < remaining {
                    remaining -= text.width() + 1;
                    extras.push(Span::styled(format!(" {text}"), style));
                }
            }
        }
        let gap = remaining + usize::from(!path_display.is_empty() && !info.is_empty());
        let mut spans = vec![
            Span::styled(path_display, base),
            Span::styled(" ".repeat(gap), base),
            Span::styled(info, base.fg(self.theme.info_fg)),
        ];
        if let Some(branch) = branch_span {
            spans.push(branch);
        }
        spans.extend(extras);
        buf.set_line(area.x, area.y, &Line::from(spans), area.width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;
    use ratatui::style::Color;

    fn test_theme() -> ThemeColors {
        theme::dark_theme()
    }

    #[test]
    fn test_basic_widget_creation() {
        let tc = test_theme();
        let widget = StatusBarWidget::new("/home/user/file.txt", "1.2 KB | File | rw-r--r--", &tc);
        assert_eq!(widget.path_str, "/home/user/file.txt");
        assert_eq!(widget.file_info, "1.2 KB | File | rw-r--r--");
        assert!(widget.status_message.is_none());
        assert!(!widget.is_error);
    }

    #[test]
    fn test_status_message_success() {
        let tc = test_theme();
        let widget = StatusBarWidget::new("/path", "info", &tc)
            .status_message("File copied successfully", false);

        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content: String = (0..80)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(content.contains("File copied successfully"));

        // Check green foreground style on first cell (theme success color)
        let cell = buf.cell((0, 0)).unwrap();
        assert_eq!(cell.fg, Color::Rgb(166, 227, 161));
    }

    #[test]
    fn test_status_message_error() {
        let tc = test_theme();
        let widget =
            StatusBarWidget::new("/path", "info", &tc).status_message("Permission denied", true);

        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content: String = (0..80)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(content.contains("Permission denied"));

        // Check error style: theme error background, theme status fg
        let cell = buf.cell((0, 0)).unwrap();
        assert_eq!(cell.bg, Color::Rgb(243, 139, 168));
        assert_eq!(cell.fg, Color::Rgb(205, 214, 244));
    }

    #[test]
    fn test_normal_bar_rendering() {
        let tc = test_theme();
        let widget = StatusBarWidget::new("/home/user/project", "4.0 KB | Dir | rwxr-xr-x", &tc);

        let area = Rect::new(0, 0, 100, 1);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content: String = (0..100)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(content.contains("/home/user/project"));
        assert!(content.contains("4.0 KB | Dir | rwxr-xr-x"));
        assert!(content.contains("y:cp"));
        assert!(content.contains("o:open"));
    }

    #[test]
    fn test_zero_area_does_not_panic() {
        let tc = test_theme();
        let widget = StatusBarWidget::new("/path", "info", &tc);
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
    }

    #[test]
    fn test_clipboard_info_displayed() {
        let tc = test_theme();
        let widget = StatusBarWidget::new("/path", "info", &tc).clipboard_info("📋 2 items");

        let area = Rect::new(0, 0, 120, 1);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content: String = (0..120)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(content.contains("2 items"));
    }

    #[test]
    fn test_status_message_unicode_narrow_width_no_panic() {
        let tc = test_theme();
        let widget = StatusBarWidget::new("/path", "info", &tc)
            .status_message("Đường dẫn: 한글/emoji 👩‍💻", false);

        let area = Rect::new(0, 0, 12, 1);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content: String = (0..12)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(!content.is_empty());
    }

    #[test]
    fn test_normal_bar_unicode_path_narrow_width_no_panic() {
        let tc = test_theme();
        let widget = StatusBarWidget::new("/tmp/한글/emoji👩‍💻/file.txt", "∞ KB | File", &tc);

        let area = Rect::new(0, 0, 24, 1);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content: String = (0..24)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(!content.is_empty());
    }

    #[test]
    fn narrow_bar_keeps_current_filename_instead_of_long_shortcuts() {
        let tc = test_theme();
        let area = Rect::new(3, 2, 60, 1);
        let mut buf = Buffer::empty(area);
        StatusBarWidget::new("/workspace/deployment/config.yaml", "YAML UTF-8 LF", &tc)
            .key_hints(" e:edit many shortcuts that must not displace the current document ")
            .render(area, &mut buf);
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("config.yaml"), "{text}");
        assert!(text.contains("LF"), "{text}");
        assert!(!text.contains("e:edit"), "{text}");
    }

    #[test]
    fn document_context_wins_over_tree_and_hints_in_backend_snapshots() {
        use super::super::workspace_chrome::DocumentIndicators;
        use ratatui::{backend::TestBackend, Terminal};
        let doc = DocumentStatus {
            path: "/workspace/非常に長い/配置/e\u{301}👩‍💻/config.yaml",
            line: 12,
            column: 4,
            language: "YAML",
            encoding: "UTF-8",
            line_ending: "LF",
            indicators: DocumentIndicators {
                dirty: true,
                read_only: true,
                external_change: true,
                temporary: true,
            },
        };
        for theme in [theme::dark_theme(), theme::light_theme()] {
            for (width, height) in [(120, 40), (80, 24), (60, 20)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                let area = Rect::new(0, height - 1, width, 1);
                terminal.draw(|frame| frame.render_widget(
                    StatusBarWidget::new("/tree/other.rs", "tree info", &theme)
                        .document_status(doc).key_hints(" e:edit overly long legacy shortcut legend that must never steal document context "),
                    area)).unwrap();
                let buf = terminal.backend().buffer();
                let text: String = buf.content.iter().map(|c| c.symbol()).collect();
                for expected in [
                    "config.yaml",
                    "Ln 12 Col 4",
                    "YAML",
                    "UTF-8",
                    "LF",
                    "*!",
                    "[RO]",
                    "[preview]",
                ] {
                    assert!(
                        text.contains(expected),
                        "{width}: missing {expected}: {text}"
                    );
                }
                assert!(!text.contains("other.rs"));
                assert!(!text.contains("e:edit"));
                assert_eq!(buf[(0, height - 1)].bg, theme.status_bg);
            }
        }
    }

    #[test]
    fn menu_reservation_survives_status_messages_and_tiny_rectangles() {
        let theme = test_theme();
        for width in 0..30 {
            for error in [false, true] {
                let area = Rect::new(2, 3, width, 1);
                let mut buf = Buffer::empty(area);
                if width > 0 {
                    buf.set_string(2, 3, "X".repeat(width as usize), Style::default());
                }
                StatusBarWidget::new("config.yaml", "LF", &theme)
                    .status_message("界e\u{301}👩‍💻 saved", error)
                    .reserved_right(8)
                    .render(area, &mut buf);
                for x in area.right().saturating_sub(width.min(8))..area.right() {
                    assert_eq!(buf[(x, 3)].symbol(), "X");
                }
                if width >= 11 {
                    assert_eq!(buf[(2, 3)].symbol(), "界");
                    assert_eq!(buf[(4, 3)].symbol(), "e\u{301}");
                    assert_eq!(
                        buf[(2, 3)].bg,
                        if error {
                            theme.error_fg
                        } else {
                            theme.status_bg
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn empty_path_does_not_steal_last_metadata_cell() {
        let theme = test_theme();
        let area = Rect::new(0, 0, 8, 1);
        let mut buf = Buffer::empty(area);
        StatusBarWidget::new("", "UTF-8 LF", &theme)
            .key_hints("")
            .render(area, &mut buf);
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert_eq!(text, "UTF-8 LF");
    }

    #[test]
    fn normal_status_composes_menu_watcher_clipboard_and_document_without_legacy_hints() {
        use super::super::workspace_chrome::DocumentIndicators;
        let theme = test_theme();
        let area = Rect::new(3, 2, 100, 1);
        let mut buf = Buffer::empty(area);
        buf.set_string(95, 2, "[Menu]XX", Style::default());
        StatusBarWidget::new("/tree/unrelated.txt", "tree info", &theme)
            .document_status(DocumentStatus {
                path: "界e\u{301}👩‍💻.rs",
                line: 2,
                column: 3,
                language: "Rust",
                encoding: "UTF-8",
                line_ending: "CRLF",
                indicators: DocumentIndicators::default(),
            })
            .clipboard_info("2 items")
            .watcher_status("[watch]")
            .reserved_right(8)
            .render(area, &mut buf);
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        for expected in [
            "Ln 2 Col 3",
            "Rust",
            "UTF-8",
            "CRLF",
            "2 items",
            "[watch]",
            "[Menu]XX",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        assert!(!text.contains("y:cp"));
        assert!(!text.contains("unrelated"));
        assert_eq!(buf[(3, 2)].symbol(), "界");
        assert_eq!(buf[(5, 2)].symbol(), "e\u{301}");
        assert_eq!(buf[(6, 2)].symbol(), "👩‍💻");
        for width in 0..20 {
            let area = Rect::new(2, 3, width, 1);
            let mut buf = Buffer::empty(area);
            StatusBarWidget::new("/long-parent/config.yaml", "Ln 2 Col 3 LF", &theme)
                .reserved_right(2)
                .render(area, &mut buf);
            for x in area.right().saturating_sub(width.min(2))..area.right() {
                assert_eq!(buf[(x, 3)].symbol(), " ");
            }
        }
        let clipped = Rect::new(0, 0, 5, 1);
        let mut buf = Buffer::empty(clipped);
        StatusBarWidget::new("config.yaml", "LF", &theme).render(Rect::new(3, 0, 20, 1), &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), " ");
    }

    #[test]
    fn very_long_document_title_still_leaves_room_for_editor_context() {
        use super::super::workspace_chrome::DocumentIndicators;
        let theme = test_theme();
        let path = format!("/workspace/{}.yaml", "配置e\u{301}👩‍💻".repeat(30));
        let area = Rect::new(2, 3, 60, 1);
        let mut buf = Buffer::empty(area);
        StatusBarWidget::new("tree.txt", "", &theme)
            .document_status(DocumentStatus {
                path: &path,
                line: 12,
                column: 4,
                language: "YAML",
                encoding: "UTF-8",
                line_ending: "LF",
                indicators: DocumentIndicators {
                    dirty: true,
                    read_only: true,
                    ..DocumentIndicators::default()
                },
            })
            .render(area, &mut buf);
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        for expected in [".yaml", "*[RO]", "Ln 12 Col 4", "LF", "YAML", "UTF-8"] {
            assert!(text.contains(expected), "{text}");
        }
    }

    #[test]
    fn git_branch_renders_in_full_and_truncates_safely_when_narrow() {
        let theme = test_theme();
        // Full width: the branch name is present and legible. The path text is
        // deliberately free of "main" so the assertion can only be satisfied by
        // the rendered branch span.
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        StatusBarWidget::new("/workspace/src/lib.rs", "File", &theme)
            .git_branch("main")
            .render(area, &mut buf);
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("main"), "{text}");

        // Without the branch the same bar has no "main": the assertion above is
        // genuinely about the branch and not the path.
        let mut buf = Buffer::empty(area);
        StatusBarWidget::new("/workspace/src/lib.rs", "File", &theme).render(area, &mut buf);
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(!text.contains("main"), "{text}");

        // Narrow: the bar never overflows and the branch is truncated, not a
        // panic or an out-of-bounds cell.
        for width in 0..40u16 {
            let area = Rect::new(0, 0, width, 1);
            let mut buf = Buffer::empty(area);
            StatusBarWidget::new("/workspace/src/lib.rs", "File", &theme)
                .git_branch("feature/long-branch-name")
                .render(area, &mut buf);
            assert_eq!(buf.content.len(), usize::from(width));
        }
    }

    #[test]
    fn git_branch_survives_document_context_and_detached_label() {
        use super::super::workspace_chrome::DocumentIndicators;
        let theme = test_theme();
        let area = Rect::new(0, 0, 100, 1);
        let mut buf = Buffer::empty(area);
        StatusBarWidget::new("/ws/config.yaml", "File", &theme)
            .document_status(DocumentStatus {
                path: "/ws/config.yaml",
                line: 1,
                column: 1,
                language: "YAML",
                encoding: "UTF-8",
                line_ending: "LF",
                indicators: DocumentIndicators::default(),
            })
            .git_branch("HEAD (detached)")
            .render(area, &mut buf);
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("HEAD (detached)"), "{text}");
        assert!(text.contains("config.yaml"), "{text}");
    }
}
