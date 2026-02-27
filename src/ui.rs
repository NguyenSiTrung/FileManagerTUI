use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders},
    Frame,
};

use crate::app::{App, AppMode, FocusedPanel};
use crate::components::dialog::DialogWidget;
use crate::components::preview::PreviewWidget;
use crate::components::status_bar::StatusBarWidget;
use crate::components::tree::TreeWidget;
use crate::fs::tree::NodeType;

/// Render the application UI.
pub fn render(app: &mut App, frame: &mut Frame) {
    let area = frame.area();

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

    // Determine border styles based on focus
    let focused_border = Style::default().fg(Color::Cyan);
    let unfocused_border = Style::default();

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

    let tree_widget = TreeWidget::new(&app.tree_state).block(tree_block);
    frame.render_widget(tree_widget, tree_area);

    // Render preview panel
    let preview_title = match &app.preview_state.current_path {
        Some(path) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Preview".to_string());
            format!(" {} ", name)
        }
        None => " Preview ".to_string(),
    };

    let preview_block = Block::default()
        .title(preview_title)
        .borders(Borders::ALL)
        .border_style(preview_border_style);

    let preview_widget = PreviewWidget::new(&app.preview_state).block(preview_block);
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

    let mut status_widget = StatusBarWidget::new(&path_str, &file_info);
    if let Some((ref msg, _)) = app.status_message {
        let is_error = msg.starts_with("Error");
        status_widget = status_widget.status_message(msg, is_error);
    }
    frame.render_widget(status_widget, status_area);

    // Render dialog overlay on top if in dialog mode
    if matches!(app.mode, AppMode::Dialog(_)) {
        let dialog_widget = DialogWidget::new(&app.mode, &app.dialog_state);
        frame.render_widget(dialog_widget, area);
    }
}
