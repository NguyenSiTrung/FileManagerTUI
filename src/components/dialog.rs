use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Widget},
};

use crate::app::{AppMode, DialogKind, DialogState};

/// Dialog widget that renders a centered modal overlay.
pub struct DialogWidget<'a> {
    mode: &'a AppMode,
    dialog_state: &'a DialogState,
}

impl<'a> DialogWidget<'a> {
    pub fn new(mode: &'a AppMode, dialog_state: &'a DialogState) -> Self {
        Self { mode, dialog_state }
    }

    /// Calculate a centered rectangle within the given area.
    fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
        let x = area.x + area.width.saturating_sub(width) / 2;
        let y = area.y + area.height.saturating_sub(height) / 2;
        let w = width.min(area.width);
        let h = height.min(area.height);
        Rect::new(x, y, w, h)
    }
}

impl<'a> Widget for DialogWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let kind = match &self.mode {
            AppMode::Dialog(kind) => kind,
            _ => return,
        };

        match kind {
            DialogKind::CreateFile => {
                render_input_dialog("Create New File", self.dialog_state, area, buf);
            }
            DialogKind::CreateDirectory => {
                render_input_dialog("Create New Directory", self.dialog_state, area, buf);
            }
            DialogKind::Rename { .. } => {
                render_input_dialog("Rename", self.dialog_state, area, buf);
            }
            DialogKind::DeleteConfirm { targets } => {
                render_confirm_dialog(targets, area, buf);
            }
            DialogKind::Error { message } => {
                render_error_dialog(message, area, buf);
            }
            DialogKind::Progress {
                message,
                current,
                total,
            } => {
                render_progress_dialog(message, *current, *total, area, buf);
            }
        }
    }
}

fn render_input_dialog(title: &str, state: &DialogState, area: Rect, buf: &mut Buffer) {
    let dialog_width = 50.min(area.width.saturating_sub(4));
    let dialog_height = 5;
    let rect = DialogWidget::centered_rect(dialog_width, dialog_height, area);

    Clear.render(rect, buf);

    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .padding(Padding::horizontal(1));

    let inner = block.inner(rect);
    block.render(rect, buf);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Render input line with cursor
    let input = &state.input;
    let cursor_pos = state.cursor_position;
    let max_width = inner.width as usize;

    let (before, cursor_char, after) = if cursor_pos < input.len() {
        let ch = &input[cursor_pos..cursor_pos + 1];
        (&input[..cursor_pos], ch, &input[cursor_pos + 1..])
    } else {
        (input.as_str(), " ", "")
    };

    // Truncate from left if input is too long
    let total_len = before.len() + 1 + after.len();
    let before_display = if total_len > max_width && before.len() > max_width.saturating_sub(2) {
        let skip = before.len().saturating_sub(max_width.saturating_sub(2));
        &before[skip..]
    } else {
        before
    };

    let input_style = Style::default().fg(Color::White);
    let cursor_style = Style::default()
        .bg(Color::White)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);

    let spans = vec![
        Span::styled(before_display, input_style),
        Span::styled(cursor_char, cursor_style),
        Span::styled(after, input_style),
    ];

    let line = Line::from(spans);
    buf.set_line(inner.x, inner.y + inner.height / 2, &line, inner.width);

    // Render hint at bottom
    let hint = "[Enter] Confirm  [Esc] Cancel";
    let hint_style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM);
    let hint_line = Line::from(Span::styled(hint, hint_style));
    if inner.height > 1 {
        buf.set_line(inner.x, inner.y + inner.height - 1, &hint_line, inner.width);
    }
}

fn render_confirm_dialog(targets: &[std::path::PathBuf], area: Rect, buf: &mut Buffer) {
    let max_name_len = targets
        .iter()
        .filter_map(|p| p.file_name())
        .map(|n| n.to_string_lossy().len())
        .max()
        .unwrap_or(10);

    let dialog_width = (max_name_len as u16 + 10)
        .max(40)
        .min(area.width.saturating_sub(4));
    let dialog_height = (targets.len() as u16 + 6).min(area.height.saturating_sub(2));
    let rect = DialogWidget::centered_rect(dialog_width, dialog_height, area);

    Clear.render(rect, buf);

    let block = Block::default()
        .title(" Delete Confirmation ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .padding(Padding::horizontal(1));

    let inner = block.inner(rect);
    block.render(rect, buf);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // "Delete the following?" header
    let header = Line::from(Span::styled(
        "Delete the following?",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    ));
    buf.set_line(inner.x, inner.y, &header, inner.width);

    // List targets
    let max_items = (inner.height.saturating_sub(3)) as usize;
    for (i, target) in targets.iter().take(max_items).enumerate() {
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| target.to_string_lossy().to_string());
        let line = Line::from(Span::styled(
            format!("  • {}", name),
            Style::default().fg(Color::White),
        ));
        buf.set_line(inner.x, inner.y + 2 + i as u16, &line, inner.width);
    }

    // Render hint at bottom
    let hint = "[y] Yes  [n/Esc] Cancel";
    let hint_style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM);
    let hint_line = Line::from(Span::styled(hint, hint_style));
    buf.set_line(inner.x, inner.y + inner.height - 1, &hint_line, inner.width);
}

fn render_error_dialog(message: &str, area: Rect, buf: &mut Buffer) {
    let dialog_width = (message.len() as u16 + 6)
        .max(30)
        .min(area.width.saturating_sub(4));
    let dialog_height = 5;
    let rect = DialogWidget::centered_rect(dialog_width, dialog_height, area);

    Clear.render(rect, buf);

    let block = Block::default()
        .title(" Error ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .padding(Padding::horizontal(1));

    let inner = block.inner(rect);
    block.render(rect, buf);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Error message
    let msg_line = Line::from(Span::styled(message, Style::default().fg(Color::Red)));
    buf.set_line(inner.x, inner.y + inner.height / 2, &msg_line, inner.width);

    // Hint
    let hint = "[Enter/Esc] Dismiss";
    let hint_style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM);
    let hint_line = Line::from(Span::styled(hint, hint_style));
    if inner.height > 1 {
        buf.set_line(inner.x, inner.y + inner.height - 1, &hint_line, inner.width);
    }
}

fn render_progress_dialog(
    current_file: &str,
    current: usize,
    total: usize,
    area: Rect,
    buf: &mut Buffer,
) {
    let dialog_width = 50.min(area.width.saturating_sub(4));
    let dialog_height = 6;
    let rect = DialogWidget::centered_rect(dialog_width, dialog_height, area);

    Clear.render(rect, buf);

    let title = format!(" Processing {}/{} ", current, total);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .padding(Padding::horizontal(1));

    let inner = block.inner(rect);
    block.render(rect, buf);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Current file being processed
    let file_line = Line::from(Span::styled(
        current_file.to_string(),
        Style::default().fg(Color::White),
    ));
    buf.set_line(inner.x, inner.y, &file_line, inner.width);

    // Simple progress bar
    if inner.height > 1 && total > 0 {
        let bar_width = inner.width as usize;
        let filled = (current * bar_width) / total;
        let bar: String = "█".repeat(filled) + &"░".repeat(bar_width.saturating_sub(filled));
        let bar_line = Line::from(Span::styled(bar, Style::default().fg(Color::Cyan)));
        buf.set_line(inner.x, inner.y + 1, &bar_line, inner.width);
    }

    // Hint at bottom
    let hint = "[Esc] Cancel";
    let hint_style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM);
    let hint_line = Line::from(Span::styled(hint, hint_style));
    if inner.height > 2 {
        buf.set_line(inner.x, inner.y + inner.height - 1, &hint_line, inner.width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_input_dialog_renders() {
        let mode = AppMode::Dialog(DialogKind::CreateFile);
        let state = DialogState {
            input: "test.txt".to_string(),
            cursor_position: 8,
        };
        let widget = DialogWidget::new(&mode, &state);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        // Check that the dialog title appears
        let content = buffer_to_string(&buf, area);
        assert!(content.contains("Create New File"));
        assert!(content.contains("test.txt"));
    }

    #[test]
    fn test_rename_dialog_renders() {
        let mode = AppMode::Dialog(DialogKind::Rename {
            original: PathBuf::from("/tmp/old_name.txt"),
        });
        let state = DialogState {
            input: "old_name.txt".to_string(),
            cursor_position: 12,
        };
        let widget = DialogWidget::new(&mode, &state);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content = buffer_to_string(&buf, area);
        assert!(content.contains("Rename"));
        assert!(content.contains("old_name.txt"));
    }

    #[test]
    fn test_confirm_dialog_renders() {
        let targets = vec![
            PathBuf::from("/tmp/file1.txt"),
            PathBuf::from("/tmp/file2.txt"),
        ];
        let mode = AppMode::Dialog(DialogKind::DeleteConfirm {
            targets: targets.clone(),
        });
        let state = DialogState::default();
        let widget = DialogWidget::new(&mode, &state);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content = buffer_to_string(&buf, area);
        assert!(content.contains("Delete"));
        assert!(content.contains("file1.txt"));
        assert!(content.contains("file2.txt"));
    }

    #[test]
    fn test_error_dialog_renders() {
        let mode = AppMode::Dialog(DialogKind::Error {
            message: "Permission denied".to_string(),
        });
        let state = DialogState::default();
        let widget = DialogWidget::new(&mode, &state);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content = buffer_to_string(&buf, area);
        assert!(content.contains("Error"));
        assert!(content.contains("Permission denied"));
    }

    #[test]
    fn test_no_dialog_mode_noop() {
        let mode = AppMode::Normal;
        let state = DialogState::default();
        let widget = DialogWidget::new(&mode, &state);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        // Buffer should be empty (all spaces)
        let content = buffer_to_string(&buf, area);
        assert!(content.trim().is_empty());
    }

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
}
