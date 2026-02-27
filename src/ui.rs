use ratatui::{
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders},
    Frame,
};

use crate::app::{App, AppMode};
use crate::components::dialog::DialogWidget;
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

    let tree_area = chunks[0];
    let status_area = chunks[1];

    // Update scroll offset to keep selected item visible
    let visible_height = tree_area.height.saturating_sub(2) as usize; // account for border
    app.tree_state.update_scroll(visible_height);

    let block = Block::default()
        .title(format!(" {} ", app.tree_state.root.name))
        .borders(Borders::ALL);

    let tree_widget = TreeWidget::new(&app.tree_state).block(block);
    frame.render_widget(tree_widget, tree_area);

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
