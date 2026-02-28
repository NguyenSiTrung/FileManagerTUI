use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Widget},
};

use crate::app::SearchActionState;
use crate::theme::ThemeColors;

/// A single action entry in the search action menu.
struct ActionEntry {
    key: &'static str,
    label: &'static str,
}

/// Overlay widget for the search action menu.
pub struct SearchActionWidget<'a> {
    state: &'a SearchActionState,
    theme: &'a ThemeColors,
    #[allow(dead_code)]
    block: Option<Block<'a>>,
}

impl<'a> SearchActionWidget<'a> {
    pub fn new(state: &'a SearchActionState, theme: &'a ThemeColors) -> Self {
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

    /// Build the list of actions based on file type flags.
    fn build_actions(&self) -> Vec<ActionEntry> {
        let mut actions = vec![ActionEntry {
            key: "Enter",
            label: "Navigate (Go to)",
        }];

        if !self.state.is_directory {
            actions.push(ActionEntry {
                key: "p",
                label: "Preview",
            });
        }

        if !self.state.is_directory && !self.state.is_binary {
            actions.push(ActionEntry {
                key: "e",
                label: "Edit",
            });
        }

        actions.push(ActionEntry {
            key: "y",
            label: "Copy path",
        });
        actions.push(ActionEntry {
            key: "r",
            label: "Rename",
        });
        actions.push(ActionEntry {
            key: "d",
            label: "Delete",
        });
        actions.push(ActionEntry {
            key: "c",
            label: "Copy (clipboard)",
        });
        actions.push(ActionEntry {
            key: "x",
            label: "Cut (clipboard)",
        });
        actions.push(ActionEntry {
            key: "t",
            label: "Open in terminal",
        });

        actions
    }

    fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
        let x = area.x + area.width.saturating_sub(width) / 2;
        let y = area.y + area.height.saturating_sub(height) / 2;
        let w = width.min(area.width);
        let h = height.min(area.height);
        Rect::new(x, y, w, h)
    }
}

impl<'a> Widget for SearchActionWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 5 || area.width < 20 {
            return;
        }

        let actions = self.build_actions();

        // Calculate dimensions: header (2 lines) + separator (1) + actions + footer (1) + borders (2)
        let content_height = 2 + 1 + actions.len() as u16 + 1;
        let dialog_height = (content_height + 2).min(area.height);
        let dialog_width = 44u16.min(area.width);

        let rect = Self::centered_rect(dialog_width, dialog_height, area);

        Clear.render(rect, buf);

        let block = Block::default()
            .title(" Action Menu ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.dialog_border_fg))
            .padding(Padding::horizontal(1));

        let inner = block.inner(rect);
        block.render(rect, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let mut row = inner.y;

        // Row 0: File path header
        let name = self
            .state
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.state.display.clone());

        let type_indicator = if self.state.is_directory {
            "📁 "
        } else if self.state.is_binary {
            "📦 "
        } else {
            "📄 "
        };

        let header_line = Line::from(vec![
            Span::styled(type_indicator, Style::default().fg(self.theme.info_fg)),
            Span::styled(
                truncate_str(&name, inner.width as usize - 4),
                Style::default()
                    .fg(self.theme.status_fg)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        buf.set_line(inner.x, row, &header_line, inner.width);
        row += 1;

        // Row 1: relative path (dimmed)
        if row < inner.y + inner.height {
            let path_line = Line::from(Span::styled(
                truncate_str(&self.state.display, inner.width as usize),
                Style::default().fg(self.theme.dim_fg),
            ));
            buf.set_line(inner.x, row, &path_line, inner.width);
            row += 1;
        }

        // Separator
        if row < inner.y + inner.height {
            let sep = "─".repeat(inner.width as usize);
            let sep_line = Line::from(Span::styled(sep, Style::default().fg(self.theme.dim_fg)));
            buf.set_line(inner.x, row, &sep_line, inner.width);
            row += 1;
        }

        // Action entries
        let key_style = Style::default()
            .fg(self.theme.warning_fg)
            .add_modifier(Modifier::BOLD);
        let label_style = Style::default().fg(self.theme.status_fg);

        for action in &actions {
            if row >= inner.y + inner.height - 1 {
                break;
            }

            let key_display = format!("[{}]", action.key);
            let padding = " ".repeat(8usize.saturating_sub(key_display.len()));

            let action_line = Line::from(vec![
                Span::styled(key_display, key_style),
                Span::raw(padding),
                Span::styled(action.label, label_style),
            ]);
            buf.set_line(inner.x, row, &action_line, inner.width);
            row += 1;
        }

        // Hint at bottom
        if inner.height > 3 {
            let hint_line = Line::from(Span::styled(
                "[Esc] Back to search",
                Style::default()
                    .fg(self.theme.dim_fg)
                    .add_modifier(Modifier::DIM),
            ));
            buf.set_line(inner.x, inner.y + inner.height - 1, &hint_line, inner.width);
        }
    }
}

/// Truncate a string to fit within `max_len` characters, adding "…" if needed.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len.saturating_sub(1)).collect();
        format!("{}…", truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;
    use std::path::PathBuf;

    fn test_theme() -> ThemeColors {
        theme::dark_theme()
    }

    fn make_state(is_directory: bool, is_binary: bool) -> SearchActionState {
        SearchActionState {
            path: PathBuf::from("/home/user/test.txt"),
            display: "test.txt".to_string(),
            is_directory,
            is_binary,
        }
    }

    #[test]
    fn test_file_actions_include_all() {
        let state = make_state(false, false);
        let tc = test_theme();
        let widget = SearchActionWidget::new(&state, &tc);
        let actions = widget.build_actions();
        let labels: Vec<&str> = actions.iter().map(|a| a.label).collect();
        assert!(labels.contains(&"Navigate (Go to)"));
        assert!(labels.contains(&"Preview"));
        assert!(labels.contains(&"Edit"));
        assert!(labels.contains(&"Copy path"));
        assert!(labels.contains(&"Rename"));
        assert!(labels.contains(&"Delete"));
        assert!(labels.contains(&"Copy (clipboard)"));
        assert!(labels.contains(&"Cut (clipboard)"));
        assert!(labels.contains(&"Open in terminal"));
    }

    #[test]
    fn test_directory_hides_edit_and_preview() {
        let state = make_state(true, false);
        let tc = test_theme();
        let widget = SearchActionWidget::new(&state, &tc);
        let actions = widget.build_actions();
        let labels: Vec<&str> = actions.iter().map(|a| a.label).collect();
        assert!(!labels.contains(&"Preview"));
        assert!(!labels.contains(&"Edit"));
        // Navigate should still be there
        assert!(labels.contains(&"Navigate (Go to)"));
    }

    #[test]
    fn test_binary_hides_edit() {
        let state = make_state(false, true);
        let tc = test_theme();
        let widget = SearchActionWidget::new(&state, &tc);
        let actions = widget.build_actions();
        let labels: Vec<&str> = actions.iter().map(|a| a.label).collect();
        assert!(!labels.contains(&"Edit"));
        // Preview should still be there for binary
        assert!(labels.contains(&"Preview"));
    }

    #[test]
    fn test_render_does_not_panic() {
        let state = make_state(false, false);
        let tc = test_theme();
        let widget = SearchActionWidget::new(&state, &tc);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let mut content = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                content.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            content.push('\n');
        }
        assert!(content.contains("Action Menu"));
        assert!(content.contains("test.txt"));
    }

    #[test]
    fn test_small_area_no_panic() {
        let state = make_state(false, false);
        let tc = test_theme();
        let widget = SearchActionWidget::new(&state, &tc);
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
    }
}
