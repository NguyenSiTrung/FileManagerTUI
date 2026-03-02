use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Widget},
};

use crate::theme::ThemeColors;

use super::settings::SettingsState;

/// Active tab within the Help overlay.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum HelpTab {
    #[default]
    Keybindings,
    Settings,
}

impl HelpTab {
    /// Toggle to the other tab.
    pub fn toggle(self) -> Self {
        match self {
            HelpTab::Keybindings => HelpTab::Settings,
            HelpTab::Settings => HelpTab::Keybindings,
        }
    }
}

/// State for the help overlay.
#[derive(Debug, Default)]
pub struct HelpState {
    /// Scroll offset for the help content.
    pub scroll_offset: usize,
    /// Active tab (Keybindings or Settings).
    pub active_tab: HelpTab,
    /// Settings state (lazily initialized on first tab switch).
    pub settings_state: Option<SettingsState>,
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
        description: "Expand dir / Load more entries",
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
        description: "Accept filter / Open action menu",
    },
];

const SEARCH_ACTION_KEYS: &[KeyEntry] = &[
    KeyEntry {
        key: "Enter",
        description: "Navigate (Go to file in tree)",
    },
    KeyEntry {
        key: "p",
        description: "Preview (navigate + focus preview)",
    },
    KeyEntry {
        key: "e",
        description: "Edit (open inline editor)",
    },
    KeyEntry {
        key: "y",
        description: "Copy path to status bar",
    },
    KeyEntry {
        key: "r",
        description: "Rename file",
    },
    KeyEntry {
        key: "d",
        description: "Delete file",
    },
    KeyEntry {
        key: "c",
        description: "Copy to clipboard",
    },
    KeyEntry {
        key: "x",
        description: "Cut to clipboard",
    },
    KeyEntry {
        key: "t",
        description: "Open parent dir in terminal",
    },
    KeyEntry {
        key: "Esc",
        description: "Back to search results",
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
        key: "Mouse drag",
        description: "Select preview text",
    },
    KeyEntry {
        key: "Ctrl+Shift+C / Ctrl+C",
        description: "Copy selected preview text",
    },
    KeyEntry {
        key: "Right click",
        description: "Copy selected preview text",
    },
    KeyEntry {
        key: "+ / -",
        description: "Adjust head/tail lines",
    },
    KeyEntry {
        key: "e",
        description: "Enter edit mode",
    },
    KeyEntry {
        key: "D",
        description: "Trigger deep scan (dirs only)",
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
        key: "t / Ctrl+T",
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
        description: "Clear selection / Leave terminal (→ tree)",
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
    KeyEntry {
        key: "Mouse drag",
        description: "Select terminal text",
    },
    KeyEntry {
        key: "Ctrl+Shift+C",
        description: "Copy selected text to clipboard",
    },
    KeyEntry {
        key: "Ctrl+Insert",
        description: "Copy selected text to clipboard",
    },
    KeyEntry {
        key: "Right click",
        description: "Copy current terminal selection",
    },
    KeyEntry {
        key: "Scroll wheel",
        description: "Scroll terminal history",
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
        name: "Search Action Menu",
        entries: SEARCH_ACTION_KEYS,
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

/// Help overlay widget showing keybindings or settings.
pub struct HelpOverlay<'a> {
    theme: &'a ThemeColors,
    state: &'a HelpState,
}

impl<'a> HelpOverlay<'a> {
    pub fn new(theme: &'a ThemeColors, state: &'a HelpState) -> Self {
        Self { theme, state }
    }

    /// Build the tab bar line.
    fn build_tab_bar(&self) -> Line<'static> {
        let kb_style = if self.state.active_tab == HelpTab::Keybindings {
            Style::default()
                .fg(self.theme.accent_fg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(self.theme.dim_fg)
        };
        let settings_style = if self.state.active_tab == HelpTab::Settings {
            Style::default()
                .fg(self.theme.accent_fg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(self.theme.dim_fg)
        };

        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(" Keybindings ", kb_style),
            Span::styled("  │  ", Style::default().fg(self.theme.dim_fg)),
            Span::styled(" Settings ", settings_style),
            Span::styled("  ", Style::default()),
        ])
    }

    /// Build keybinding content lines.
    fn build_keybinding_lines(&self) -> Vec<Line<'static>> {
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
            " Press ? or Esc to close  |  Tab/←/→ to switch tabs ",
            Style::default().fg(self.theme.dim_fg),
        )]));

        lines
    }

    /// Build all the lines for the current tab.
    fn build_content_lines(&self) -> Vec<Line<'static>> {
        match self.state.active_tab {
            HelpTab::Keybindings => self.build_keybinding_lines(),
            HelpTab::Settings => {
                if let Some(ref settings) = self.state.settings_state {
                    use super::settings::SettingsWidget;
                    let widget = SettingsWidget::new(settings, self.theme);
                    widget.build_content_lines()
                } else {
                    vec![Line::from(vec![Span::styled(
                        "  Settings not loaded. Switch tabs to initialize.",
                        Style::default().fg(self.theme.dim_fg),
                    )])]
                }
            }
        }
    }

    /// Get total number of keybinding content lines (for scroll bounds).
    pub fn keybinding_total_lines() -> usize {
        let mut count = 2; // title + blank
        for category in CATEGORIES {
            count += 1; // header
            count += category.entries.len();
            count += 1; // blank separator
        }
        count += 1; // footer
        count
    }

    /// Get total lines for the current tab.
    pub fn total_lines_for_tab(&self) -> usize {
        match self.state.active_tab {
            HelpTab::Keybindings => Self::keybinding_total_lines(),
            HelpTab::Settings => {
                if let Some(ref settings) = self.state.settings_state {
                    use super::settings::SettingsWidget;
                    let widget = SettingsWidget::new(settings, self.theme);
                    widget.total_lines()
                } else {
                    1
                }
            }
        }
    }
}

impl<'a> Widget for HelpOverlay<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Center the overlay — 70% width, 80% height
        let overlay_width = (area.width as f32 * 0.70).min(90.0) as u16;
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

        // Render tab bar at the top of the inner area
        let tab_line = self.build_tab_bar();
        if inner.height > 0 {
            buf.set_line(inner.x, inner.y, &tab_line, inner.width);
        }

        // Separator line below tab bar
        if inner.height > 1 {
            let sep = Line::from(Span::styled(
                "─".repeat(inner.width as usize),
                Style::default().fg(self.theme.dim_fg),
            ));
            buf.set_line(inner.x, inner.y + 1, &sep, inner.width);
        }

        // Content area below the tab bar + separator (2 lines reserved)
        let content_y = inner.y + 2;
        let content_height = inner.height.saturating_sub(2) as usize;

        // Build and render content lines
        let content_lines = self.build_content_lines();
        // Use the correct scroll offset for the active tab:
        // - Keybindings: uses HelpState.scroll_offset
        // - Settings: uses SettingsState.scroll_offset (auto-managed by update_scroll)
        let scroll = match self.state.active_tab {
            HelpTab::Settings => self
                .state
                .settings_state
                .as_ref()
                .map(|s| s.scroll_offset)
                .unwrap_or(0),
            HelpTab::Keybindings => self.state.scroll_offset,
        };

        for (i, line) in content_lines
            .iter()
            .skip(scroll)
            .take(content_height)
            .enumerate()
        {
            let line_y = content_y + i as u16;
            if line_y >= inner.y + inner.height {
                break;
            }
            buf.set_line(inner.x + 1, line_y, line, inner.width.saturating_sub(2));
        }

        // Draw scroll indicator if content overflows
        if content_lines.len() > content_height {
            let indicator = match self.state.active_tab {
                HelpTab::Settings => {
                    // Show selected entry index / total entries
                    if let Some(ref settings) = self.state.settings_state {
                        let current = settings.selected_index + 1;
                        let total = settings.entries.len();
                        format!(" {}/{} ", current, total)
                    } else {
                        String::new()
                    }
                }
                HelpTab::Keybindings => {
                    // Show scroll position as percentage
                    let total = content_lines.len();
                    let visible_end = (scroll + content_height).min(total);
                    let pct = if total > 0 {
                        (visible_end * 100) / total
                    } else {
                        100
                    };
                    format!(" {}% ", pct)
                }
            };
            if !indicator.is_empty() {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keybinding_total_lines_is_nonzero() {
        assert!(HelpOverlay::keybinding_total_lines() > 0);
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
    fn keybinding_content_lines_match_total() {
        let theme = crate::theme::dark_theme();
        let state = HelpState::default();
        let overlay = HelpOverlay::new(&theme, &state);
        let lines = overlay.build_keybinding_lines();
        assert_eq!(lines.len(), HelpOverlay::keybinding_total_lines());
    }

    #[test]
    fn default_tab_is_keybindings() {
        let state = HelpState::default();
        assert_eq!(state.active_tab, HelpTab::Keybindings);
    }

    #[test]
    fn tab_toggle_cycles() {
        assert_eq!(HelpTab::Keybindings.toggle(), HelpTab::Settings);
        assert_eq!(HelpTab::Settings.toggle(), HelpTab::Keybindings);
    }

    #[test]
    fn total_lines_for_keybinding_tab() {
        let theme = crate::theme::dark_theme();
        let state = HelpState::default();
        let overlay = HelpOverlay::new(&theme, &state);
        assert_eq!(
            overlay.total_lines_for_tab(),
            HelpOverlay::keybinding_total_lines()
        );
    }

    #[test]
    fn total_lines_for_settings_tab() {
        let theme = crate::theme::dark_theme();
        let config = crate::config::AppConfig::default();
        let mut state = HelpState::default();
        state.active_tab = HelpTab::Settings;
        state.settings_state = Some(super::super::settings::SettingsState::from_config(&config));
        let overlay = HelpOverlay::new(&theme, &state);
        assert!(overlay.total_lines_for_tab() > 0);
    }
}
