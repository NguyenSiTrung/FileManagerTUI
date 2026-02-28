use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::theme::ThemeColors;

/// Status bar widget that displays file path, info, key hints, or status messages.
pub struct StatusBarWidget<'a> {
    path_str: &'a str,
    file_info: &'a str,
    theme: &'a ThemeColors,
    status_message: Option<&'a str>,
    is_error: bool,
    clipboard_info: Option<&'a str>,
    watcher_status: Option<&'a str>,
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
}

impl<'a> Widget for StatusBarWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let width = area.width as usize;

        if let Some(msg) = self.status_message {
            let style = if self.is_error {
                Style::default()
                    .bg(self.theme.error_fg)
                    .fg(self.theme.status_fg)
            } else {
                Style::default().fg(self.theme.success_fg)
            };

            // Pad or truncate message to fill full width
            let display: String = if msg.len() >= width {
                msg[..width].to_string()
            } else {
                format!("{:<width$}", msg, width = width)
            };

            let line = Line::from(Span::styled(display, style));
            buf.set_line(area.x, area.y, &line, area.width);
            return;
        }

        // Normal bar: [path] [file_info] [key_hints]
        let key_hints = " a:new  A:dir  r:ren  d:del ";
        let hints_len = key_hints.len();

        // Reserve space for hints on the right
        let remaining = width.saturating_sub(hints_len);

        // Split remaining between path (left) and file_info (center-right)
        let info_len = self.file_info.len();
        let path_budget = remaining.saturating_sub(info_len).saturating_sub(1); // 1 for separator space

        let path_display = if self.path_str.len() > path_budget {
            if path_budget > 3 {
                format!(
                    "...{}",
                    &self.path_str[self.path_str.len() - (path_budget - 3)..]
                )
            } else {
                self.path_str[..path_budget].to_string()
            }
        } else {
            self.path_str.to_string()
        };

        let info_display = if self.file_info.len() > remaining.saturating_sub(path_display.len()) {
            let budget = remaining.saturating_sub(path_display.len());
            if budget > 0 {
                self.file_info[..budget].to_string()
            } else {
                String::new()
            }
        } else {
            self.file_info.to_string()
        };

        // Calculate gap between path and info to push info toward center-right
        let gap = remaining
            .saturating_sub(path_display.len())
            .saturating_sub(info_display.len());

        let path_style = Style::default().fg(self.theme.status_fg);
        let info_style = Style::default().fg(self.theme.info_fg);
        let hints_style = Style::default()
            .fg(self.theme.dim_fg)
            .add_modifier(Modifier::DIM);

        let mut spans = vec![
            Span::styled(path_display, path_style),
            Span::raw(" ".repeat(gap)),
            Span::styled(info_display, info_style),
        ];

        // Add clipboard info if present
        let clipboard_display = self.clipboard_info.unwrap_or("");
        let clipboard_len = clipboard_display.len();
        if clipboard_len > 0 {
            let clipboard_style = Style::default()
                .fg(self.theme.accent_fg)
                .add_modifier(Modifier::BOLD);
            spans.push(Span::raw(" "));
            spans.push(Span::styled(clipboard_display.to_string(), clipboard_style));
        }

        // Add watcher status indicator if present
        if let Some(watcher_str) = self.watcher_status {
            let watcher_style = Style::default()
                .fg(self.theme.warning_fg)
                .add_modifier(Modifier::BOLD);
            spans.push(Span::raw(" "));
            spans.push(Span::styled(watcher_str.to_string(), watcher_style));
        }

        // Pad to fill remaining width if needed, then add hints
        let used: usize = spans.iter().map(|s| s.content.len()).sum();
        let pad = width.saturating_sub(used).saturating_sub(hints_len);
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        spans.push(Span::styled(key_hints, hints_style));

        let line = Line::from(spans);
        buf.set_line(area.x, area.y, &line, area.width);
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
        assert!(content.contains("a:new"));
        assert!(content.contains("d:del"));
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
}
