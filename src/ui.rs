use ratatui::{
    widgets::{Block, Borders},
    Frame,
};

use crate::app::App;
use crate::components::tree::TreeWidget;

/// Render the application UI.
pub fn render(app: &mut App, frame: &mut Frame) {
    let area = frame.area();

    // Update scroll offset to keep selected item visible
    let visible_height = area.height.saturating_sub(2) as usize; // account for border
    app.tree_state.update_scroll(visible_height);

    let block = Block::default()
        .title(format!(" {} ", app.tree_state.root.name))
        .borders(Borders::ALL);

    let tree_widget = TreeWidget::new(&app.tree_state).block(block);
    frame.render_widget(tree_widget, area);
}
