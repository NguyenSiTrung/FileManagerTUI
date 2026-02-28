use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Widget},
};

use crate::theme::ThemeColors;

/// State for the help overlay.
#[derive(Debug, Default)]
pub struct HelpState {
    /// Scroll offset for the help content.
    pub scroll_offset: usize,
}

/// A single keybinding entry for display.
struct KeyEntry {
    key: &'static str,
    description: &'static str,
}

/// A category of keybindings.
struct KeyCategory {
    name: &'static str,
    entries: &'static [KeyEntry],
}

const NAVIGATION_KEYS: &[KeyEntry] = &[
    KeyEntry {
        key: "j / ↓",
        description: "Move down",
    },
    KeyEntry {
        key: "k / ↑",
        description: "Move up",
    },
    KeyEntry {
        key: "g / Home",
        description: "Jump to first item",
    },
    KeyEntry {
        key: "G / End",
        description: "Jump to last item",
    },
    KeyEntry {
        key: "Enter / l / →",
        description: "Expand directory",
    },
    KeyEntry {
        key: "Backspace / h / ←",
        description: "Collapse directory",
    },
    KeyEntry {
        key: "Tab",
        description: "Cycle panel focus (forward)",
    },
    KeyEntry {
        key: "Ctrl+←/→",
        description: "Focus left/right panel",
    },
    KeyEntry {
        key: "Ctrl+↑/↓",
        description: "Focus up/down (terminal)",
    },
    KeyEntry {
        key: ".",
        description: "Toggle hidden files",
    },
    KeyEntry {
        key: "Space",
        description: "Toggle multi-select",
    },
    KeyEntry {
        key: "Esc",
        description: "Clear multi-selection",
    },
    KeyEntry {
        key: "s",
        description: "Cycle sort (name → size → modified)",
    },
    KeyEntry {
        key: "S",
        description: "Toggle dirs first",
    },
];

const FILE_OPS_KEYS: &[KeyEntry] = &[
    KeyEntry {
        key: "a",
        description: "Create new file",
    },
    KeyEntry {
        key: "A",
        description: "Create new directory",
    },
    KeyEntry {
        key: "r",
        description: "Rename item",
    },
    KeyEntry {
        key: "d",
        description: "Delete item",
    },
    KeyEntry {
        key: "y",
        description: "Copy to clipboard",
    },
    KeyEntry {
        key: "x",
        description: "Cut to clipboard",
    },
    KeyEntry {
        key: "p",
        description: "Paste from clipboard",
    },
    KeyEntry {
        key: "Ctrl+Z",
        description: "Undo last operation",
    },
];

const SEARCH_FILTER_KEYS: &[KeyEntry] = &[
    KeyEntry {
        key: "Ctrl+P",
        description: "Open fuzzy finder",
    },
    KeyEntry {
        key: "/",
        description: "Start inline filter",
    },
    KeyEntry {
        key: "Esc",
        description: "Cancel / clear filter",
    },
    KeyEntry {
        key: "Enter",
        description: "Accept filter",
    },
];

const PREVIEW_KEYS: &[KeyEntry] = &[
    KeyEntry {
        key: "j / ↓",
        description: "Scroll down",
    },
    KeyEntry {
        key: "k / ↑",
        description: "Scroll up",
    },
    KeyEntry {
        key: "g / Home",
        description: "Jump to top",
    },
    KeyEntry {
        key: "G / End",
        description: "Jump to bottom",
    },
    KeyEntry {
        key: "Ctrl+D",
        description: "Half page down",
    },
    KeyEntry {
        key: "Ctrl+U",
        description: "Half page up",
    },
    KeyEntry {
        key: "Ctrl+W",
        description: "Toggle line wrap",
    },
    KeyEntry {
        key: "+ / -",
        description: "Adjust head/tail lines",
    },
    KeyEntry {
        key: "e",
        description: "Enter edit mode",
    },
];

const EDITOR_KEYS: &[KeyEntry] = &[
    KeyEntry {
        key: "Esc",
        description: "Exit edit mode (prompt if unsaved)",
    },
    KeyEntry {
        key: "Ctrl+S",
        description: "Save file",
    },
    KeyEntry {
        key: "Arrows",
        description: "Move cursor",
    },
    KeyEntry {
        key: "Home / End",
        description: "Start / end of line",
    },
    KeyEntry {
        key: "Ctrl+Home/End",
        description: "Top / bottom of file",
    },
    KeyEntry {
        key: "PgUp / PgDn",
        description: "Page up / page down",
    },
    KeyEntry {
        key: "Shift+Arrows",
        description: "Select text (char/line)",
    },
    KeyEntry {
        key: "Shift+Home/End",
        description: "Select to line start/end",
    },
    KeyEntry {
        key: "Shift+Ctrl+Home/End",
        description: "Select to file start/end",
    },
    KeyEntry {
        key: "Shift+PgUp/PgDn",
        description: "Select page up/down",
    },
    KeyEntry {
        key: "Ctrl+A",
        description: "Select all",
    },
    KeyEntry {
        key: "Tab / Shift+Tab",
        description: "Indent / dedent",
    },
    KeyEntry {
        key: "Ctrl+Z",
        description: "Undo",
    },
    KeyEntry {
        key: "Ctrl+Y",
        description: "Redo",
    },
    KeyEntry {
        key: "Ctrl+C",
        description: "Copy (selection or line)",
    },
    KeyEntry {
        key: "Ctrl+X",
        description: "Cut (selection or line)",
    },
    KeyEntry {
        key: "Ctrl+V",
        description: "Paste",
    },
    KeyEntry {
        key: "Ctrl+F",
        description: "Find",
    },
    KeyEntry {
        key: "Ctrl+H",
        description: "Find & Replace",
    },
    KeyEntry {
        key: "Ctrl+A (in replace)",
        description: "Replace all",
    },
    KeyEntry {
        key: "Mouse click",
        description: "Position cursor / click+drag to select",
    },
    KeyEntry {
        key: "Scroll wheel",
        description: "Scroll editor viewport",
    },
];

const TERMINAL_KEYS: &[KeyEntry] = &[
    KeyEntry {
        key: "Ctrl+T",
        description: "Toggle terminal panel",
    },
    KeyEntry {
        key: "Ctrl+Shift+↑",
        description: "Resize terminal smaller",
    },
    KeyEntry {
        key: "Ctrl+Shift+↓",
        description: "Resize terminal larger",
    },
    KeyEntry {
        key: "Esc",
        description: "Leave terminal (focus → tree)",
    },
    KeyEntry {
        key: "Tab",
        description: "Shell autocompletion (sent to PTY)",
    },
    KeyEntry {
        key: "Shift+↑/↓",
        description: "Scroll terminal history",
    },
    KeyEntry {
        key: "Shift+PgUp/PgDn",
        description: "Fast scroll terminal history",
    },
];

const GENERAL_KEYS: &[KeyEntry] = &[
    KeyEntry {
        key: "?",
        description: "Toggle this help overlay",
    },
    KeyEntry {
        key: "q",
        description: "Quit",
    },
    KeyEntry {
        key: "Ctrl+C",
        description: "Quit",
    },
    KeyEntry {
        key: "F5",
        description: "Manual refresh",
    },
    KeyEntry {
        key: "Ctrl+R",
        description: "Toggle file watcher",
    },
];

const CATEGORIES: &[KeyCategory] = &[
    KeyCategory {
        name: "Navigation (Tree Panel)",
        entries: NAVIGATION_KEYS,
    },
    KeyCategory {
        name: "File Operations",
        entries: FILE_OPS_KEYS,
    },
    KeyCategory {
        name: "Search & Filter",
        entries: SEARCH_FILTER_KEYS,
    },
    KeyCategory {
        name: "Preview Panel",
        entries: PREVIEW_KEYS,
    },
    KeyCategory {
        name: "Editor Mode (Preview)",
        entries: EDITOR_KEYS,
    },
    KeyCategory {
        name: "Terminal Panel",
        entries: TERMINAL_KEYS,
    },
    KeyCategory {
        name: "General",
        entries: GENERAL_KEYS,
    },
];

/// Help overlay widget showing all keybindings.
pub struct HelpOverlay<'a> {
    theme: &'a ThemeColors,
    scroll_offset: usize,
}

impl<'a> HelpOverlay<'a> {
    pub fn new(theme: &'a ThemeColors, scroll_offset: usize) -> Self {
        Self {
            theme,
            scroll_offset,
        }
    }

    /// Build all the lines for the help content.
    fn build_content_lines(&self) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        // Title
        lines.push(Line::from(vec![Span::styled(
            " Keybinding Reference ",
            Style::default()
                .fg(self.theme.accent_fg)
                .add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(""));

        for category in CATEGORIES {
            // Category header
            lines.push(Line::from(vec![
                Span::styled(
                    format!("── {} ", category.name),
                    Style::default()
                        .fg(self.theme.accent_fg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("─".repeat(40), Style::default().fg(self.theme.dim_fg)),
            ]));

            for entry in category.entries {
                let key_width = 24;
                let key_padded = format!("  {:<width$}", entry.key, width = key_width);
                lines.push(Line::from(vec![
                    Span::styled(
                        key_padded,
                        Style::default()
                            .fg(self.theme.warning_fg)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        entry.description.to_string(),
                        Style::default().fg(self.theme.tree_file_fg),
                    ),
                ]));
            }

            lines.push(Line::from(""));
        }

        // Footer
        lines.push(Line::from(vec![Span::styled(
            " Press ? or Esc to close ",
            Style::default().fg(self.theme.dim_fg),
        )]));

        lines
    }

    /// Get total number of content lines (for scroll bounds).
    pub fn total_lines() -> usize {
        let mut count = 2; // title + blank
        for category in CATEGORIES {
            count += 1; // header
            count += category.entries.len();
            count += 1; // blank separator
        }
        count += 1; // footer
        count
    }
}

impl<'a> Widget for HelpOverlay<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Center the overlay — 70% width, 80% height
        let overlay_width = (area.width as f32 * 0.70).min(80.0) as u16;
        let overlay_height = (area.height as f32 * 0.80).min(50.0) as u16;

        let x = area.x + (area.width.saturating_sub(overlay_width)) / 2;
        let y = area.y + (area.height.saturating_sub(overlay_height)) / 2;
        let overlay_area = Rect::new(x, y, overlay_width, overlay_height);

        // Clear the background
        Clear.render(overlay_area, buf);

        // Draw the block
        let block = Block::default()
            .title(" Help ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.border_focused_fg))
            .style(Style::default().bg(self.theme.dialog_bg));

        let inner = block.inner(overlay_area);
        block.render(overlay_area, buf);

        // Build and render content lines
        let content_lines = self.build_content_lines();
        let visible_height = inner.height as usize;

        let scroll = self.scroll_offset;

        for (i, line) in content_lines
            .iter()
            .skip(scroll)
            .take(visible_height)
            .enumerate()
        {
            let line_y = inner.y + i as u16;
            if line_y >= inner.y + inner.height {
                break;
            }
            buf.set_line(inner.x + 1, line_y, line, inner.width.saturating_sub(2));
        }

        // Draw scroll indicator if content overflows
        if content_lines.len() > visible_height {
            let total = content_lines.len();
            let indicator = format!(" {}/{} ", (scroll + 1).min(total), total);
            let ind_span = Span::styled(indicator, Style::default().fg(self.theme.dim_fg));
            let ind_x = overlay_area.x
                + overlay_area
                    .width
                    .saturating_sub(ind_span.width() as u16 + 1);
            let ind_y = overlay_area.y + overlay_area.height - 1;
            buf.set_span(ind_x, ind_y, &ind_span, ind_span.width() as u16);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_lines_is_nonzero() {
        assert!(HelpOverlay::total_lines() > 0);
    }

    #[test]
    fn all_categories_have_entries() {
        for cat in CATEGORIES {
            assert!(
                !cat.entries.is_empty(),
                "Category '{}' has no entries",
                cat.name
            );
        }
    }

    #[test]
    fn content_lines_match_total() {
        let theme = crate::theme::dark_theme();
        let overlay = HelpOverlay::new(&theme, 0);
        let lines = overlay.build_content_lines();
        assert_eq!(lines.len(), HelpOverlay::total_lines());
    }
}
