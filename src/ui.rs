use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::Style,
    widgets::{Block, Borders},
    Frame,
};

use crate::app::{App, AppMode, FocusedPanel};
use crate::components::dialog::DialogWidget;
use crate::components::editor::EditorWidget;
use crate::components::help::HelpOverlay;
use crate::components::preview::PreviewWidget;
use crate::components::search::SearchWidget;
use crate::components::search_action::SearchActionWidget;
use crate::components::status_bar::StatusBarWidget;
use crate::components::terminal::TerminalWidget;
use crate::components::tree::TreeWidget;
use crate::fs::tree::NodeType;

/// Render the application UI.
pub fn render(app: &mut App, frame: &mut Frame) {
    // Update preview when selection changes
    app.update_preview();

    let area = frame.area();
    let theme = app.theme_colors.clone();

    // Determine vertical layout:
    // If terminal is visible: [main_area, terminal_area, status_bar]
    // If terminal is hidden:  [main_area, status_bar]
    let terminal_visible = app.terminal_state.visible;
    let term_height_pct = app.terminal_state.height_percent;

    let chunks = if terminal_visible {
        // Calculate terminal height in rows from percentage
        let screen_height = area.height;
        let term_rows = ((screen_height as u32 * term_height_pct as u32) / 100).max(3) as u16;
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),            // main (tree + preview)
                Constraint::Length(term_rows), // terminal panel
                Constraint::Length(1),         // status bar
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(1)])
            .split(area)
    };

    let main_area = chunks[0];
    let (terminal_area_rect, status_area) = if terminal_visible {
        (chunks[1], chunks[2])
    } else {
        (ratatui::layout::Rect::default(), chunks[1])
    };

    // Store terminal area for mouse mapping and resize
    app.terminal_area = terminal_area_rect;

    // Split main area: tree (40%) + preview (60%)
    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(main_area);

    let tree_area = panels[0];
    let preview_area = panels[1];

    // Store areas for mouse click mapping
    app.tree_area = tree_area;
    app.preview_area = preview_area;
    app.clamp_preview_scroll();

    // Determine border styles based on focus (using theme colors)
    let focused_border = Style::default().fg(theme.border_focused_fg);
    let unfocused_border = Style::default().fg(theme.border_fg);
    // S3 mode: tint the tree border with S3-specific color for visual context
    let tree_focused_border = if app.is_s3_mode() {
        Style::default().fg(theme.s3_border_fg)
    } else {
        focused_border
    };

    let (tree_border_style, preview_border_style, terminal_border_style) = match app.focused_panel {
        FocusedPanel::Tree => (tree_focused_border, unfocused_border, unfocused_border),
        FocusedPanel::Preview => (unfocused_border, focused_border, unfocused_border),
        FocusedPanel::Terminal => (unfocused_border, unfocused_border, focused_border),
    };

    // Update scroll offset to keep selected item visible,
    // but only if the viewport wasn't explicitly scrolled by mouse/scrollbar.
    let visible_height = tree_area.height.saturating_sub(2) as usize; // account for border
    app.tree_visible_height = visible_height;
    if !app.tree_viewport_locked {
        app.tree_state.update_scroll(visible_height);
    }

    let tree_block = Block::default()
        .title(format!(" {} ", app.tree_state.root.name))
        .borders(Borders::ALL)
        .border_style(tree_border_style);

    let tree_widget = TreeWidget::new(&app.tree_state, &theme, app.config.use_icons())
        .s3_mode(app.is_s3_mode())
        .block(tree_block.clone());
    // Store scrollbar column for mouse hit testing
    app.scrollbar_column = tree_widget.scrollbar_x(tree_area);
    let tree_widget = TreeWidget::new(&app.tree_state, &theme, app.config.use_icons())
        .s3_mode(app.is_s3_mode())
        .block(tree_block);
    frame.render_widget(tree_widget, tree_area);

    // Render preview panel (or editor if in edit mode)
    if app.mode == AppMode::Edit && app.editor_state.is_some() {
        // Edit mode: render editor widget
        let dirty = app.editor_state.as_ref().is_some_and(|e| e.modified);
        let editor_title = match &app.preview_state.current_path {
            Some(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Editor".to_string());
                if dirty {
                    format!(" {} ● [EDIT] ", name)
                } else {
                    format!(" {} [EDIT] ", name)
                }
            }
            None => " Editor ".to_string(),
        };

        let editor_block = Block::default()
            .title(editor_title)
            .borders(Borders::ALL)
            .border_style(preview_border_style);

        // Update visible_height before rendering
        if let Some(ref mut editor) = app.editor_state {
            let inner_height = preview_area.height.saturating_sub(2) as usize;
            editor.visible_height = inner_height;
            editor.ensure_cursor_visible();
        }

        if let Some(ref editor) = app.editor_state {
            let editor_widget =
                EditorWidget::new(editor, &theme, &app.syntax_set, &app.syntax_theme)
                    .block(editor_block);
            frame.render_widget(editor_widget, preview_area);
        }
    } else {
        // Normal preview mode
        let preview_title = match &app.preview_state.current_path {
            Some(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Preview".to_string());
                if path.extension().and_then(|e| e.to_str()) == Some("ipynb") {
                    let cell_count = app
                        .preview_state
                        .content_lines
                        .iter()
                        .filter(|l| {
                            l.spans
                                .first()
                                .map(|s| s.content.starts_with('━'))
                                .unwrap_or(false)
                        })
                        .count();
                    format!(" Notebook: {} cells ", cell_count)
                } else {
                    format!(" {} ", name)
                }
            }
            None => " Preview ".to_string(),
        };

        let preview_block = Block::default()
            .title(preview_title)
            .borders(Borders::ALL)
            .border_style(preview_border_style);

        let preview_widget = PreviewWidget::new(&app.preview_state, &theme)
            .selection(&app.preview_selection)
            .block(preview_block);
        frame.render_widget(preview_widget, preview_area);
    }

    // Render terminal panel if visible
    if terminal_visible {
        let terminal_title = if app.terminal_state.exited {
            " Terminal [exited] ".to_string()
        } else {
            " Terminal ".to_string()
        };

        let terminal_block = Block::default()
            .title(terminal_title)
            .borders(Borders::ALL)
            .border_style(terminal_border_style);

        let show_cursor = app.focused_panel == FocusedPanel::Terminal;
        let terminal_widget =
            TerminalWidget::new(&app.terminal_state, &theme, show_cursor).block(terminal_block);
        frame.render_widget(terminal_widget, terminal_area_rect);

        // Resize emulator to match the inner area if needed
        let inner_rows = terminal_area_rect.height.saturating_sub(2) as usize;
        let inner_cols = terminal_area_rect.width.saturating_sub(2) as usize;
        if inner_rows > 0
            && inner_cols > 0
            && (app.terminal_state.emulator.visible_rows() != inner_rows
                || app.terminal_state.emulator.visible_cols() != inner_cols)
        {
            app.terminal_state.emulator.resize(inner_rows, inner_cols);
            // Tell PTY about the new size
            if let Some(ref pty) = app.terminal_state.pty {
                let _ = pty.resize(inner_rows as u16, inner_cols as u16);
            }
        }
    }

    // Clear expired status messages
    app.clear_expired_status();

    // Build status bar
    let selected_item = app.tree_state.flat_items.get(app.tree_state.selected_index);

    let path_str = selected_item
        .map(|item| item.path.to_string_lossy().to_string())
        .unwrap_or_default();

    let file_info = selected_item
        .map(|item| match item.node_type {
            NodeType::Directory => {
                if let Some(count) = item.child_count {
                    if let Some(remaining) = item.load_more_remaining {
                        format!("Dir ({}/{} loaded)", count - remaining, count)
                    } else {
                        format!("Dir ({} items)", count)
                    }
                } else {
                    "Dir".to_string()
                }
            }
            NodeType::File => "File".to_string(),
            NodeType::Symlink => "Symlink".to_string(),
            NodeType::LoadMore => {
                if let Some(remaining) = item.load_more_remaining {
                    format!("Load more... (~{} remaining)", remaining)
                } else {
                    "Load more...".to_string()
                }
            }
            NodeType::Loading => "Loading...".to_string(),
        })
        .unwrap_or_default();

    let mut status_widget = StatusBarWidget::new(&path_str, &file_info, &theme);

    // Context-aware key hints based on focused panel
    let key_hints_str = match app.focused_panel {
        FocusedPanel::Tree => {
            if app.is_s3_mode() {
                " ↵:expand Y:uri y:cp s:sort ?:help ".to_string()
            } else if app.clipboard.is_empty() {
                " y:cp x:cut Y:path T:term o:open a:new r:ren d:del ".to_string()
            } else {
                " y:cp x:cut p:paste Y:path T:term o:open r:ren d:del ".to_string()
            }
        }
        FocusedPanel::Preview => {
            if app.preview_state.is_shallow_preview {
                " e:edit j/k:scroll D:deep ?:help ".to_string()
            } else {
                " e:edit j/k:scroll g/G:top/bot ?:help ".to_string()
            }
        }
        FocusedPanel::Terminal => " Esc:back C-c:copy ?:help ".to_string(),
    };
    status_widget = status_widget.key_hints(&key_hints_str);

    // Show clipboard info if clipboard has content
    let clipboard_info_str;
    if !app.clipboard.is_empty() {
        use crate::fs::clipboard::ClipboardOp;
        let icon = match app.clipboard.operation {
            Some(ClipboardOp::Copy) => "📋",
            Some(ClipboardOp::Cut) => "✂",
            None => "",
        };
        clipboard_info_str = format!(
            "{} {} item{}",
            icon,
            app.clipboard.len(),
            if app.clipboard.len() == 1 { "" } else { "s" }
        );
        status_widget = status_widget.clipboard_info(&clipboard_info_str);
    }

    // Show watcher status indicator
    let watcher_indicator = if app.is_s3_mode() {
        "☁ S3".to_string()
    } else if app.watcher_active {
        "👁 Auto".to_string()
    } else {
        "⏸ Manual".to_string()
    };
    status_widget = status_widget.watcher_status(&watcher_indicator);

    // Show filter query in status bar when filtering
    let filter_display;
    if app.mode == AppMode::Filter || app.tree_state.is_filtering {
        filter_display = format!("Filter: {}_", app.tree_state.filter_query);
        status_widget = status_widget.status_message(&filter_display, false);
    } else if let Some((ref msg, _)) = app.status_message {
        let is_error = msg.starts_with("Error");
        status_widget = status_widget.status_message(msg, is_error);
    }
    frame.render_widget(status_widget, status_area);

    // Render dialog overlay on top if in dialog mode
    if matches!(app.mode, AppMode::Dialog(_)) {
        let dialog_widget = DialogWidget::new(&app.mode, &app.dialog_state, &theme);
        frame.render_widget(dialog_widget, area);
    }

    // Render search overlay on top if in search mode
    if app.mode == AppMode::Search {
        let search_widget = SearchWidget::new(&app.search_state, &theme);
        frame.render_widget(search_widget, area);
    }

    // Render search action overlay on top if in search action mode
    if app.mode == AppMode::SearchAction {
        if let Some(ref state) = app.search_action_state {
            let action_widget = SearchActionWidget::new(state, &theme);
            frame.render_widget(action_widget, area);
        }
    }

    // Render help overlay on top if in help mode
    if app.mode == AppMode::Help {
        // Update settings scroll if on settings tab
        if app.help_state.active_tab == crate::components::help::HelpTab::Settings {
            // Estimate visible content height from area (80% of screen height, minus borders, tab bar, separator)
            let overlay_height = (area.height as f32 * 0.80).min(50.0) as usize;
            let content_height = overlay_height.saturating_sub(4); // border(2) + tab bar(1) + separator(1)
            if let Some(ref mut settings) = app.help_state.settings_state {
                let total_lines = {
                    use crate::components::settings::SettingsWidget;
                    let widget = SettingsWidget::new(settings, &theme);
                    widget.total_lines()
                };
                settings.update_scroll(content_height, total_lines);
            }
        }
        let help_widget = HelpOverlay::new(&theme, &app.help_state);
        frame.render_widget(help_widget, area);
    }

    // Render copy overlay — mouse capture is disabled in this mode so the
    // browser/xterm.js handles text selection + Ctrl+C natively.
    if app.mode == AppMode::CopyOverlay {
        if let Some(ref text) = app.copy_overlay_text {
            use ratatui::layout::Alignment;
            use ratatui::text::{Line, Span};
            use ratatui::widgets::{Clear, Paragraph, Wrap};

            // Center the overlay
            let text_width = (text.len() as u16 + 6).min(area.width.saturating_sub(4));
            let overlay_width = text_width.max(46);
            let overlay_height = 7u16;
            let x = area.x + (area.width.saturating_sub(overlay_width)) / 2;
            let y = area.y + (area.height.saturating_sub(overlay_height)) / 2;
            let overlay_area = ratatui::layout::Rect::new(x, y, overlay_width, overlay_height);

            frame.render_widget(Clear, overlay_area);

            let lines = vec![
                Line::raw(""),
                Line::from(Span::styled(
                    text.clone(),
                    Style::default()
                        .fg(ratatui::style::Color::Yellow)
                        .add_modifier(ratatui::style::Modifier::BOLD),
                )),
                Line::raw(""),
                Line::from(Span::styled(
                    "Select above → Ctrl+C to copy",
                    Style::default().fg(ratatui::style::Color::DarkGray),
                )),
                Line::from(Span::styled(
                    "Press Enter/Esc to return",
                    Style::default().fg(ratatui::style::Color::DarkGray),
                )),
            ];

            let popup = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" 📋 Copy Path ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(theme.border_focused_fg)),
                )
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false });

            frame.render_widget(popup, overlay_area);
        }
    }
}
