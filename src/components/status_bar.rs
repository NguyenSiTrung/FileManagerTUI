use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

/// Status bar widget that displays file path, info, key hints, or status messages.
pub struct StatusBarWidget<'a> {
    path_str: &'a str,
    file_info: &'a str,
    status_message: Option<&'a str>,
    is_error: bool,
}

impl<'a> StatusBarWidget<'a> {
    pub fn new(path_str: &'a str, file_info: &'a str) -> Self {
        Self {
            path_str,
            file_info,
            status_message: None,
            is_error: false,
        }
    }

    pub fn status_message(mut self, msg: &'a str, is_error: bool) -> Self {
        self.status_message = Some(msg);
        self.is_error = is_error;
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
                Style::default().bg(Color::Red).fg(Color::White)
            } else {
                Style::default().fg(Color::Green)
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

        let path_style = Style::default().fg(Color::White);
        let info_style = Style::default().fg(Color::Cyan);
        let hints_style = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM);

        let mut spans = vec![
            Span::styled(path_display, path_style),
            Span::raw(" ".repeat(gap)),
            Span::styled(info_display, info_style),
        ];

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

    #[test]
    fn test_basic_widget_creation() {
        let widget = StatusBarWidget::new("/home/user/file.txt", "1.2 KB | File | rw-r--r--");
        assert_eq!(widget.path_str, "/home/user/file.txt");
        assert_eq!(widget.file_info, "1.2 KB | File | rw-r--r--");
        assert!(widget.status_message.is_none());
        assert!(!widget.is_error);
    }

    #[test]
    fn test_status_message_success() {
        let widget =
            StatusBarWidget::new("/path", "info").status_message("File copied successfully", false);

        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content: String = (0..80)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(content.contains("File copied successfully"));

        // Check green foreground style on first cell
        let cell = buf.cell((0, 0)).unwrap();
        assert_eq!(cell.fg, Color::Green);
    }

    #[test]
    fn test_status_message_error() {
        let widget =
            StatusBarWidget::new("/path", "info").status_message("Permission denied", true);

        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content: String = (0..80)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(content.contains("Permission denied"));

        // Check error style: red background, white foreground
        let cell = buf.cell((0, 0)).unwrap();
        assert_eq!(cell.bg, Color::Red);
        assert_eq!(cell.fg, Color::White);
    }

    #[test]
    fn test_normal_bar_rendering() {
        let widget = StatusBarWidget::new("/home/user/project", "4.0 KB | Dir | rwxr-xr-x");

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
        let widget = StatusBarWidget::new("/path", "info");
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
    }
}
