use ratatui::{
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;

/// Render the application UI.
pub fn render(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let block = Block::default()
        .title(format!(" {} ", app.tree_state.root.name))
        .borders(Borders::ALL);

    let items: Vec<Line> = app
        .tree_state
        .flat_items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let style = if i == app.tree_state.selected_index {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default()
            };
            Line::styled(format!("  {}", item.name), style)
        })
        .collect();

    let paragraph = Paragraph::new(items).block(block);
    frame.render_widget(paragraph, area);
}
