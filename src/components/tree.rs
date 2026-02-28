use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Widget},
};

use crate::fs::tree::{FlatItem, NodeType, TreeState};
use crate::theme::ThemeColors;

/// Tree widget that renders the file tree with box-drawing characters.
pub struct TreeWidget<'a> {
    tree_state: &'a TreeState,
    theme: &'a ThemeColors,
    use_icons: bool,
    block: Option<Block<'a>>,
}

impl<'a> TreeWidget<'a> {
    pub fn new(tree_state: &'a TreeState, theme: &'a ThemeColors, use_icons: bool) -> Self {
        Self {
            tree_state,
            theme,
            use_icons,
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

    /// Get the directory/file indicator.
    fn item_indicator(&self, item: &FlatItem) -> &'static str {
        if self.use_icons {
            match item.node_type {
                NodeType::Directory if item.is_expanded => " ",
                NodeType::Directory => " ",
                NodeType::Symlink => " ",
                NodeType::File => Self::file_icon_by_ext(&item.name),
                NodeType::LoadMore => "▼ ",
            }
        } else {
            match item.node_type {
                NodeType::Directory if item.is_expanded => "[D] ",
                NodeType::Directory => "[D] ",
                NodeType::Symlink => "[L] ",
                NodeType::File => "[F] ",
                NodeType::LoadMore => "[+] ",
            }
        }
    }

    /// Get a Nerd Font icon for a file based on its extension.
    fn file_icon_by_ext(name: &str) -> &'static str {
        let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
        match ext.as_str() {
            "rs" => " ",
            "py" => " ",
            "js" | "jsx" => " ",
            "ts" | "tsx" => " ",
            "html" | "htm" => " ",
            "css" | "scss" | "sass" => " ",
            "json" => " ",
            "toml" | "yaml" | "yml" | "ini" | "cfg" => " ",
            "md" | "markdown" | "rst" | "txt" => " ",
            "sh" | "bash" | "zsh" | "fish" => " ",
            "go" => " ",
            "java" | "jar" | "class" => " ",
            "c" | "h" => " ",
            "cpp" | "cxx" | "cc" | "hpp" => " ",
            "rb" => " ",
            "php" => " ",
            "lua" => " ",
            "r" => " ",
            "swift" => " ",
            "kt" | "kts" => " ",
            "ex" | "exs" => " ",
            "lock" => " ",
            "gitignore" | "gitmodules" | "gitattributes" => " ",
            "dockerfile" => " ",
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "ico" | "webp" => " ",
            "mp3" | "wav" | "flac" | "ogg" | "aac" => " ",
            "mp4" | "mkv" | "avi" | "mov" | "webm" => " ",
            "zip" | "tar" | "gz" | "xz" | "bz2" | "rar" | "7z" => " ",
            "pdf" => " ",
            "ipynb" => " ",
            "sql" | "db" | "sqlite" => " ",
            _ => " ",
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
            let indicator = self.item_indicator(item);

            let is_selected = idx == selected;
            let is_multi_selected = self.tree_state.multi_selected.contains(&idx);

            let style = if is_selected {
                Style::default()
                    .bg(self.theme.tree_selected_bg)
                    .fg(self.theme.tree_selected_fg)
                    .add_modifier(Modifier::BOLD)
            } else if is_multi_selected {
                Style::default()
                    .bg(self.theme.accent_fg)
                    .fg(self.theme.warning_fg)
                    .add_modifier(Modifier::BOLD)
            } else if item.is_hidden {
                Style::default().fg(self.theme.tree_hidden_fg)
            } else {
                match item.node_type {
                    NodeType::Directory => Style::default()
                        .fg(self.theme.tree_dir_fg)
                        .add_modifier(Modifier::BOLD),
                    NodeType::Symlink => Style::default().fg(self.theme.info_fg),
                    NodeType::File => Style::default().fg(self.theme.tree_file_fg),
                    NodeType::LoadMore => Style::default()
                        .fg(self.theme.info_fg)
                        .add_modifier(Modifier::ITALIC),
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
