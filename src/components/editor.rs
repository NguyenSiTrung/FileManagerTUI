use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Widget},
};
use syntect::highlighting::Theme;
use syntect::parsing::SyntaxSet;

use crate::editor::EditorState;
use crate::theme::ThemeColors;

/// Widget for rendering the editor view with line numbers, syntax highlighting, and cursor.
pub struct EditorWidget<'a> {
    editor: &'a EditorState,
    theme: &'a ThemeColors,
    syntax_set: &'a SyntaxSet,
    syntax_theme: &'a Theme,
    block: Option<Block<'a>>,
}

impl<'a> EditorWidget<'a> {
    pub fn new(
        editor: &'a EditorState,
        theme: &'a ThemeColors,
        syntax_set: &'a SyntaxSet,
        syntax_theme: &'a Theme,
    ) -> Self {
        Self {
            editor,
            theme,
            syntax_set,
            syntax_theme,
            block: None,
        }
    }

    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    /// Calculate the width needed for the line number gutter.
    fn gutter_width(&self) -> u16 {
        let max_line = self.editor.line_count();
        let digits = if max_line == 0 {
            1
        } else {
            (max_line as f64).log10().floor() as u16 + 1
        };
        digits + 2 // digits + space + separator
    }
}

impl<'a> Widget for EditorWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let inner = if let Some(block) = &self.block {
            let inner = block.inner(area);
            block.clone().render(area, buf);
            inner
        } else {
            area
        };

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        // Reserve space for find bar if active
        let find_bar_height = if self.editor.find_state.active {
            if self.editor.find_state.replace_mode {
                2u16
            } else {
                1u16
            }
        } else {
            0u16
        };

        let editor_height = inner.height.saturating_sub(find_bar_height) as usize;
        let gutter_w = self.gutter_width();
        let code_width = inner.width.saturating_sub(gutter_w);

        if code_width == 0 {
            return;
        }

        let scroll = self.editor.scroll_offset;

        // Prepare syntax highlighter for visible lines
        let file_path = &self.editor.file_path;
        let syntax = self
            .syntax_set
            .find_syntax_for_file(file_path)
            .ok()
            .flatten()
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());
        let mut highlight_state = syntect::easy::HighlightLines::new(syntax, self.syntax_theme);

        // If we're scrolling, we need to process lines before the viewport
        // to get correct syntax state. Process up to scroll offset.
        for i in 0..scroll.min(self.editor.buffer.len()) {
            let line = &self.editor.buffer[i];
            let _ = highlight_state.highlight_line(line, self.syntax_set);
        }

        // Render visible lines
        for row in 0..editor_height {
            let line_idx = scroll + row;
            let y = inner.y + row as u16;

            if line_idx < self.editor.buffer.len() {
                let line_num = line_idx + 1; // 1-indexed
                let is_current_line = line_idx == self.editor.cursor_line;

                // Line number gutter
                let num_str = format!("{:>width$} ", line_num, width = (gutter_w - 2) as usize);
                let gutter_style = if is_current_line {
                    Style::default()
                        .fg(self.theme.editor_line_nr_current)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(self.theme.editor_line_nr)
                };
                let gutter_span = Span::styled(num_str, gutter_style);
                buf.set_span(inner.x, y, &gutter_span, gutter_w);

                // Separator
                let sep_style = Style::default().fg(self.theme.editor_gutter_sep);
                buf.set_string(inner.x + gutter_w - 1, y, "│", sep_style);

                // Code content with syntax highlighting
                let line_content = &self.editor.buffer[line_idx];
                let code_x = inner.x + gutter_w;

                // Apply syntax highlighting
                let highlighted = highlight_state
                    .highlight_line(line_content, self.syntax_set)
                    .unwrap_or_default();

                let mut col_offset = 0u16;
                for (style, text) in &highlighted {
                    for ch in text.chars() {
                        if col_offset >= code_width {
                            break;
                        }
                        let char_col = col_offset as usize;
                        let is_cursor = is_current_line && char_col == self.editor.cursor_col;
                        let is_find_match = self.is_find_match(line_idx, char_col);
                        let is_sel = self.editor.is_selected(line_idx, char_col);

                        let fg = ratatui::style::Color::Rgb(
                            style.foreground.r,
                            style.foreground.g,
                            style.foreground.b,
                        );

                        let cell_style = if is_cursor {
                            Style::default()
                                .fg(self.theme.editor_cursor_fg)
                                .bg(self.theme.editor_cursor_bg)
                        } else if is_find_match {
                            Style::default()
                                .fg(ratatui::style::Color::Black)
                                .bg(self.theme.editor_find_match_bg)
                        } else if is_sel {
                            Style::default()
                                .fg(fg)
                                .bg(self.theme.editor_selection_bg)
                        } else if is_current_line {
                            Style::default()
                                .fg(fg)
                                .bg(self.theme.editor_current_line_bg)
                        } else {
                            Style::default().fg(fg)
                        };

                        let mut s = String::new();
                        s.push(ch);
                        buf.set_string(code_x + col_offset, y, &s, cell_style);
                        col_offset += 1;
                    }
                }

                // Show cursor at end of line if needed
                if is_current_line && self.editor.cursor_col >= line_content.len() {
                    let cursor_x = code_x + line_content.len() as u16;
                    if cursor_x < inner.x + inner.width {
                        buf.set_string(
                            cursor_x,
                            y,
                            " ",
                            Style::default()
                                .fg(self.theme.editor_cursor_fg)
                                .bg(self.theme.editor_cursor_bg),
                        );
                    }
                }

                // Fill rest of current line with highlight
                if is_current_line {
                    let start_fill = code_x + col_offset.max(self.editor.cursor_col as u16 + 1);
                    for fill_x in start_fill..inner.x + inner.width {
                        buf.set_string(
                            fill_x,
                            y,
                            " ",
                            Style::default().bg(self.theme.editor_current_line_bg),
                        );
                    }
                }
            } else {
                // Lines beyond buffer: show tilde
                let tilde_style = Style::default().fg(self.theme.dim_fg);
                buf.set_string(inner.x, y, "~", tilde_style);
            }
        }

        // Render find bar at bottom
        if self.editor.find_state.active {
            self.render_find_bar(inner, find_bar_height, buf);
        }
    }
}

impl<'a> EditorWidget<'a> {
    /// Check if a character position is part of a find match.
    fn is_find_match(&self, line: usize, col: usize) -> bool {
        if !self.editor.find_state.active || self.editor.find_state.query.is_empty() {
            return false;
        }
        let query_len = self.editor.find_state.query.len();
        for &(match_line, match_col) in &self.editor.find_state.matches {
            if match_line == line && col >= match_col && col < match_col + query_len {
                return true;
            }
        }
        false
    }

    /// Render the find/replace bar at the bottom of the editor area.
    fn render_find_bar(&self, inner: Rect, bar_height: u16, buf: &mut Buffer) {
        let bar_y = inner.y + inner.height - bar_height;
        let _bar_width = inner.width as usize;

        // Clear the bar area
        let bar_bg = Style::default()
            .fg(self.theme.status_fg)
            .bg(self.theme.editor_find_bar_bg);
        for y in bar_y..bar_y + bar_height {
            for x in inner.x..inner.x + inner.width {
                buf.set_string(x, y, " ", bar_bg);
            }
        }

        // Find label + query
        let find_active = !self.editor.find_state.in_replace_field;
        let find_label = "Find: ";
        let query = &self.editor.find_state.query;
        let match_info = if self.editor.find_state.matches.is_empty() {
            if query.is_empty() {
                String::new()
            } else {
                " (no matches)".to_string()
            }
        } else {
            format!(
                " ({}/{})",
                self.editor.find_state.current_match + 1,
                self.editor.find_state.matches.len()
            )
        };

        let find_style = if find_active {
            Style::default()
                .fg(self.theme.status_fg)
                .bg(self.theme.editor_find_bar_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(self.theme.dim_fg)
                .bg(self.theme.editor_find_bar_bg)
        };

        let find_line = Line::from(vec![
            Span::styled(find_label, find_style),
            Span::styled(query, find_style),
            Span::styled(
                &match_info,
                Style::default()
                    .fg(self.theme.dim_fg)
                    .bg(self.theme.editor_find_bar_bg),
            ),
        ]);
        buf.set_line(inner.x, bar_y, &find_line, inner.width);

        // Replace line (if in replace mode)
        if self.editor.find_state.replace_mode && bar_height > 1 {
            let replace_active = self.editor.find_state.in_replace_field;
            let replace_label = "Replace: ";
            let replacement = &self.editor.find_state.replacement;

            let replace_style = if replace_active {
                Style::default()
                    .fg(self.theme.status_fg)
                    .bg(self.theme.editor_find_bar_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(self.theme.dim_fg)
                    .bg(self.theme.editor_find_bar_bg)
            };

            let replace_line = Line::from(vec![
                Span::styled(replace_label, replace_style),
                Span::styled(replacement, replace_style),
            ]);
            buf.set_line(inner.x, bar_y + 1, &replace_line, inner.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use std::path::PathBuf;
    use syntect::highlighting::ThemeSet;

    fn test_theme() -> ThemeColors {
        crate::theme::dark_theme()
    }

    fn test_syntax() -> (SyntaxSet, Theme) {
        let ss = SyntaxSet::load_defaults_nonewlines();
        let ts = ThemeSet::load_defaults();
        let theme = ts.themes["base16-ocean.dark"].clone();
        (ss, theme)
    }

    #[test]
    fn test_editor_widget_renders_lines() {
        let editor = EditorState::new("line1\nline2\nline3", PathBuf::from("test.txt"));
        let theme = test_theme();
        let (ss, st) = test_syntax();
        let widget = EditorWidget::new(&editor, &theme, &ss, &st);

        let area = Rect::new(0, 0, 40, 5);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content = buffer_to_string(&buf, area);
        assert!(content.contains('1')); // line number 1
        assert!(content.contains('2')); // line number 2
        assert!(content.contains('3')); // line number 3
    }

    #[test]
    fn test_editor_widget_with_block() {
        let editor = EditorState::new("hello", PathBuf::from("test.txt"));
        let theme = test_theme();
        let (ss, st) = test_syntax();
        let block = Block::default()
            .title(" Test ")
            .borders(ratatui::widgets::Borders::ALL);
        let widget = EditorWidget::new(&editor, &theme, &ss, &st).block(block);

        let area = Rect::new(0, 0, 40, 5);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content = buffer_to_string(&buf, area);
        assert!(content.contains("Test"));
    }

    #[test]
    fn test_editor_widget_tilde_beyond_buffer() {
        let editor = EditorState::new("line1", PathBuf::from("test.txt"));
        let theme = test_theme();
        let (ss, st) = test_syntax();
        let widget = EditorWidget::new(&editor, &theme, &ss, &st);

        let area = Rect::new(0, 0, 40, 5);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content = buffer_to_string(&buf, area);
        assert!(content.contains('~')); // tilde on empty lines
    }

    #[test]
    fn test_gutter_width() {
        let editor = EditorState::new("a", PathBuf::from("test.txt"));
        let theme = test_theme();
        let (ss, st) = test_syntax();
        let widget = EditorWidget::new(&editor, &theme, &ss, &st);
        assert_eq!(widget.gutter_width(), 3); // 1 digit + 1 space + 1 separator

        let many_lines = (0..100)
            .map(|i| format!("line{}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let editor2 = EditorState::new(&many_lines, PathBuf::from("test.txt"));
        let widget2 = EditorWidget::new(&editor2, &theme, &ss, &st);
        assert_eq!(widget2.gutter_width(), 5); // 3 digits + 1 space + 1 separator
    }

    fn buffer_to_string(buf: &Buffer, area: Rect) -> String {
        let mut s = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                s.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            s.push('\n');
        }
        s
    }
}
