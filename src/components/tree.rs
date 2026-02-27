use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Widget},
};

use crate::fs::tree::{FlatItem, NodeType, TreeState};

/// Tree widget that renders the file tree with box-drawing characters.
pub struct TreeWidget<'a> {
    tree_state: &'a TreeState,
    block: Option<Block<'a>>,
}

impl<'a> TreeWidget<'a> {
    pub fn new(tree_state: &'a TreeState) -> Self {
        Self {
            tree_state,
            block: None,
        }
    }

    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = block.into();
        self
    }

    /// Build the prefix string for tree indentation using box-drawing characters.
    ///
    /// We need to know the ancestor chain to draw continuation lines correctly.
    fn build_prefix(item: &FlatItem, items: &[FlatItem], item_index: usize) -> String {
        if item.depth == 0 {
            return String::new();
        }

        // Build prefix from left to right for each depth level
        let mut parts: Vec<&str> = Vec::new();

        // For each ancestor level (1..depth), determine if it's the last sibling at that level
        // We need to look backwards through ancestors to figure this out
        for d in 1..item.depth {
            // Find the ancestor at depth d that contains this item
            let mut ancestor_is_last = false;
            // Walk backwards from current item to find the ancestor at depth d
            for j in (0..item_index).rev() {
                if items[j].depth == d {
                    ancestor_is_last = items[j].is_last_sibling;
                    break;
                }
                if items[j].depth < d {
                    break;
                }
            }
            if ancestor_is_last {
                parts.push("   ");
            } else {
                parts.push("│  ");
            }
        }

        // The connector for this item
        if item.is_last_sibling {
            parts.push("└──");
        } else {
            parts.push("├──");
        }

        parts.join("")
    }

    /// Get the directory indicator character.
    fn dir_indicator(item: &FlatItem) -> &'static str {
        match item.node_type {
            NodeType::Directory if item.is_expanded => "▼ ",
            NodeType::Directory => "▶ ",
            _ => "",
        }
    }
}

impl<'a> Widget for TreeWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let inner_area = if let Some(block) = &self.block {
            let inner = block.inner(area);
            block.clone().render(area, buf);
            inner
        } else {
            area
        };

        let items = &self.tree_state.flat_items;
        let selected = self.tree_state.selected_index;
        let visible_height = inner_area.height as usize;

        if items.is_empty() || visible_height == 0 {
            return;
        }

        // Compute scroll offset to keep selected item visible
        let scroll = self.tree_state.scroll_offset;

        let visible_items = items.iter().enumerate().skip(scroll).take(visible_height);

        for (i, (idx, item)) in visible_items.enumerate() {
            let y = inner_area.y + i as u16;
            if y >= inner_area.y + inner_area.height {
                break;
            }

            let prefix = Self::build_prefix(item, items, idx);
            let indicator = Self::dir_indicator(item);

            let is_selected = idx == selected;
            let is_multi_selected = self.tree_state.multi_selected.contains(&idx);

            let style = if is_selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if is_multi_selected {
                Style::default()
                    .bg(Color::Rgb(40, 40, 80))
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                match item.node_type {
                    NodeType::Directory => Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                    NodeType::Symlink => Style::default().fg(Color::Cyan),
                    NodeType::File => Style::default(),
                }
            };

            let marker = if is_multi_selected { "● " } else { "" };
            let line_content = format!("{}{}{}{}", prefix, marker, indicator, item.name);
            let span = Span::styled(line_content, style);
            let line = Line::from(span);

            let line_area = Rect::new(inner_area.x, y, inner_area.width, 1);
            buf.set_line(line_area.x, line_area.y, &line, line_area.width);
        }
    }
}
