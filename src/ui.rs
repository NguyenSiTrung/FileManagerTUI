use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::Style,
    widgets::{Block, Borders},
    Frame,
};

use crate::app::{App, AppMode, FocusedPanel};
use crate::components::dialog::DialogWidget;
use crate::components::preview::PreviewWidget;
use crate::components::search::SearchWidget;
use crate::components::status_bar::StatusBarWidget;
use crate::components::tree::TreeWidget;
use crate::fs::tree::NodeType;

/// Render the application UI.
pub fn render(app: &mut App, frame: &mut Frame) {
    // Update preview when selection changes
    app.update_preview();

    let area = frame.area();
    let theme = app.theme_colors.clone();

    // Split into main area + status bar (1 line)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);

    let main_area = chunks[0];
    let status_area = chunks[1];

    // Split main area: tree (40%) + preview (60%)
    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(main_area);

    let tree_area = panels[0];
    let preview_area = panels[1];

    // Determine border styles based on focus (using theme colors)
    let focused_border = Style::default().fg(theme.border_focused_fg);
    let unfocused_border = Style::default().fg(theme.border_fg);

    let (tree_border_style, preview_border_style) = match app.focused_panel {
        FocusedPanel::Tree => (focused_border, unfocused_border),
        FocusedPanel::Preview => (unfocused_border, focused_border),
    };

    // Update scroll offset to keep selected item visible
    let visible_height = tree_area.height.saturating_sub(2) as usize; // account for border
    app.tree_state.update_scroll(visible_height);

    let tree_block = Block::default()
        .title(format!(" {} ", app.tree_state.root.name))
        .borders(Borders::ALL)
        .border_style(tree_border_style);

    let tree_widget = TreeWidget::new(&app.tree_state, &theme).block(tree_block);
    frame.render_widget(tree_widget, tree_area);

    // Render preview panel
    let preview_title = match &app.preview_state.current_path {
        Some(path) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Preview".to_string());
            if path.extension().and_then(|e| e.to_str()) == Some("ipynb") {
                // Count cells from content lines (cell headers start with ━━━)
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

    let preview_widget = PreviewWidget::new(&app.preview_state, &theme).block(preview_block);
    frame.render_widget(preview_widget, preview_area);

    // Clear expired status messages
    app.clear_expired_status();

    // Build status bar
    let selected_item = app.tree_state.flat_items.get(app.tree_state.selected_index);

    let path_str = selected_item
        .map(|item| item.path.to_string_lossy().to_string())
        .unwrap_or_default();

    let file_info = selected_item
        .map(|item| match item.node_type {
            NodeType::Directory => "Dir".to_string(),
            NodeType::File => "File".to_string(),
            NodeType::Symlink => "Symlink".to_string(),
        })
        .unwrap_or_default();

    let mut status_widget = StatusBarWidget::new(&path_str, &file_info, &theme);

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
    let watcher_indicator = if app.watcher_active {
        "👁".to_string()
    } else {
        "⏸".to_string()
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
}
