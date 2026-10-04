use crate::text::{self, byte_to_display_col, TAB_WIDTH};
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
    /// Prepared-only syntax source. When set, rendering never runs the
    /// highlighter; unprepared lines render in the explicit pending style.
    prepared: Option<&'a crate::highlighting::SyntaxCache>,
    /// When true (production `App::prepared_pipeline`), the render-path
    /// highlighter fallback is unreachable even if no cache was attached.
    prepared_only: bool,
    /// Worst diagnostic severity per line — colors the gutter number.
    diagnostic_lines: Option<&'a std::collections::BTreeMap<u32, crate::diagnostics::Severity>>,
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
            prepared: None,
            prepared_only: false,
            diagnostic_lines: None,
            block: None,
        }
    }

    /// Diagnostic gutter markers for this document (worst severity wins).
    pub fn diagnostic_lines(
        mut self,
        lines: &'a std::collections::BTreeMap<u32, crate::diagnostics::Severity>,
    ) -> Self {
        self.diagnostic_lines = Some(lines);
        self
    }

    /// Render from prepared runs only (no render-path highlighting).
    pub fn prepared(mut self, cache: &'a crate::highlighting::SyntaxCache) -> Self {
        self.prepared = Some(cache);
        self
    }

    /// Mark the prepared pipeline: never run the render-path highlighter, even
    /// when no cache is attached (lines stay in the explicit pending style).
    pub fn prepared_only(mut self, prepared_only: bool) -> Self {
        self.prepared_only = prepared_only;
        self
    }

    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    /// Calculate the width needed for the line number gutter.
    fn gutter_width(&self) -> u16 {
        self.editor.gutter_width()
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

        let find_bar_height = self.editor.find_bar_height(inner.height as usize) as u16;
        let editor_height = inner.height.saturating_sub(find_bar_height) as usize;
        let gutter_w = self.gutter_width().min(inner.width);
        let code_width = inner.width.saturating_sub(gutter_w);
        if code_width == 0 {
            if find_bar_height > 0 {
                self.render_find_bar(inner, find_bar_height, buf);
            }
            return;
        }
        let scroll = self.editor.scroll_offset;
        let (first_line, _) = self.editor.visual_row(scroll);
        let prepared = self.prepared;
        // Prepared pipeline: the fallback highlighter is never constructed, so
        // a missing cache can never trigger full-prefix highlighting in render.
        let mut highlighter = if prepared.is_none() && !self.prepared_only {
            let syntax = self
                .syntax_set
                .find_syntax_for_file(&self.editor.file_path)
                .ok()
                .flatten()
                .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());
            let mut highlighter = syntect::easy::HighlightLines::new(syntax, self.syntax_theme);
            for line in &self.editor.buffer[..first_line] {
                let _ = highlighter.highlight_line(&format!("{line}\n"), self.syntax_set);
            }
            Some(highlighter)
        } else {
            None
        };
        let mut highlighted_line = None;
        let mut styles = Vec::new();
        let mut viewport_rows = self
            .editor
            .buffer
            .iter()
            .enumerate()
            .flat_map(|(line, content)| {
                text::visual_rows(
                    content,
                    self.editor.visible_width,
                    self.editor.line_wrap,
                    true,
                )
                .map(move |mapped| (line, mapped))
            })
            .skip(scroll);
        let cursor_row = self.editor.cursor_visual_row();
        for row in 0..editor_height {
            let y = inner.y + row as u16;
            for x in inner.x..inner.right() {
                buf[(x, y)].reset();
            }
            let Some((line_idx, mapped)) = viewport_rows.next() else {
                buf.set_string(inner.x, y, "~", Style::default().fg(self.theme.dim_fg));
                continue;
            };
            let current = line_idx == self.editor.cursor_line;
            let content = &self.editor.buffer[line_idx];
            if highlighted_line != Some(line_idx) {
                styles.clear();
                if let Some(cache) = prepared {
                    if let Some(runs) = cache.runs(line_idx) {
                        for run in runs {
                            styles.push((run.start, run.end, run.style));
                        }
                    }
                    // Unprepared line: explicit pending style (no stale runs).
                } else if let Some(highlighter) = highlighter.as_mut() {
                    let with_nl = format!("{content}\n");
                    let highlighted = highlighter
                        .highlight_line(&with_nl, self.syntax_set)
                        .unwrap_or_default();
                    let mut byte = 0;
                    for (style, text) in highlighted {
                        styles.push((byte, byte + text.len(), style));
                        byte += text.len();
                    }
                }
                highlighted_line = Some(line_idx);
            }
            if mapped.start == 0 || !self.editor.line_wrap {
                let num = format!("{:>width$} ", line_idx + 1, width = (gutter_w - 2) as usize);
                let style = if current {
                    Style::default()
                        .fg(self.theme.editor_line_nr_current)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(self.theme.editor_line_nr)
                };
                // A diagnostic on this line paints the gutter number in
                // the worst severity's color (number stays readable).
                let style = match self
                    .diagnostic_lines
                    .and_then(|lines| lines.get(&(line_idx as u32)))
                {
                    Some(crate::diagnostics::Severity::Error) => {
                        style.fg(self.theme.error_fg).add_modifier(Modifier::BOLD)
                    }
                    Some(crate::diagnostics::Severity::Warning) => {
                        style.fg(self.theme.warning_fg).add_modifier(Modifier::BOLD)
                    }
                    Some(crate::diagnostics::Severity::Information) => {
                        style.fg(self.theme.editor_line_nr_current)
                    }
                    Some(crate::diagnostics::Severity::Hint) => style
                        .fg(self.theme.editor_line_nr)
                        .add_modifier(Modifier::UNDERLINED),
                    None => style,
                };
                buf.set_span(inner.x, y, &Span::styled(num, style), gutter_w);
            }
            buf.set_string(
                inner.x + gutter_w - 1,
                y,
                "│",
                Style::default().fg(self.theme.editor_gutter_sep),
            );
            let start = if self.editor.line_wrap {
                mapped.start
            } else {
                self.editor.horizontal_offset
            };
            let end = if self.editor.line_wrap {
                mapped.end
            } else {
                start.saturating_add(code_width as usize)
            };
            let base = if current {
                Style::default().bg(self.theme.editor_current_line_bg)
            } else {
                Style::default()
            };
            let cursor_byte = self.editor.cursor_col;
            let display_row = if self.editor.line_wrap {
                mapped
            } else {
                text::VisualRow::window(content, start, end)
            };
            text::paint_display_row(
                content,
                display_row,
                Rect::new(inner.x + gutter_w, y, code_width, 1),
                buf,
                base,
                |g| {
                    let syntax_style =
                        styles.get(styles.partition_point(|(_, end, _)| *end <= g.byte));
                    let fg = syntax_style
                        .map(|(_, _, s)| {
                            ratatui::style::Color::Rgb(
                                s.foreground.r,
                                s.foreground.g,
                                s.foreground.b,
                            )
                        })
                        .unwrap_or(self.theme.status_fg);
                    let mut style = base.fg(fg);
                    if let Some((_, _, syntax)) = syntax_style {
                        for (font, modifier) in [
                            (syntect::highlighting::FontStyle::BOLD, Modifier::BOLD),
                            (syntect::highlighting::FontStyle::ITALIC, Modifier::ITALIC),
                            (
                                syntect::highlighting::FontStyle::UNDERLINE,
                                Modifier::UNDERLINED,
                            ),
                        ] {
                            if syntax.font_style.contains(font) {
                                style = style.add_modifier(modifier);
                            }
                        }
                    }
                    if current && g.byte == cursor_byte {
                        style = style
                            .fg(self.theme.editor_cursor_fg)
                            .bg(self.theme.editor_cursor_bg);
                    } else if (g.byte..g.byte + g.text.len())
                        .any(|b| self.is_find_match(line_idx, b))
                    {
                        style = style
                            .fg(ratatui::style::Color::Black)
                            .bg(self.theme.editor_find_match_bg);
                    } else if (g.byte..g.byte + g.text.len())
                        .any(|b| self.editor.is_selected(line_idx, b))
                    {
                        style = style.bg(self.theme.editor_selection_bg);
                    }
                    style
                },
            );
            let cursor_col = byte_to_display_col(content, cursor_byte, TAB_WIDTH);
            if current
                && cursor_byte == content.len()
                && cursor_col >= start
                && cursor_col - start < code_width as usize
                && (!self.editor.line_wrap || scroll + row == cursor_row)
            {
                buf.set_string(
                    inner.x + gutter_w + (cursor_col - start) as u16,
                    y,
                    " ",
                    Style::default()
                        .fg(self.theme.editor_cursor_fg)
                        .bg(self.theme.editor_cursor_bg),
                );
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
        let bar_height = bar_height.min(inner.height);
        if bar_height == 0 {
            return;
        }
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

    #[test]
    fn tiny_replace_bar_does_not_underflow() {
        let mut editor = EditorState::new("abc", "test.txt".into());
        editor.open_find_replace();
        let theme = test_theme();
        let (ss, st) = test_syntax();
        let area = Rect::new(0, 0, 4, 1);
        let mut buf = Buffer::empty(area);
        EditorWidget::new(&editor, &theme, &ss, &st).render(area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "F");
    }

    #[test]
    fn wrapped_wide_boundary_tracks_cursor_on_continuation() {
        let mut editor = EditorState::new("abc中Z", "test.txt".into());
        editor.visible_width = 4;
        editor.visible_height = 1;
        editor.line_wrap = true;
        editor.set_cursor_position(0, 3);
        assert_eq!(editor.cursor_visual_row(), 1);
        assert_eq!(editor.scroll_offset, 1);
        let area = Rect::new(0, 0, 7, 1);
        let mut buf = Buffer::empty(area);
        let theme = test_theme();
        let (ss, st) = test_syntax();
        EditorWidget::new(&editor, &theme, &ss, &st).render(area, &mut buf);
        assert_eq!(buf[(3, 0)].symbol(), "中");
        assert_eq!(buf[(3, 0)].bg, theme.editor_cursor_bg);
        assert_eq!(buf[(0, 0)].symbol(), " ");
    }

    #[test]
    fn same_byte_delete_reflow_renders_cursor_in_one_row_code_viewport() {
        let theme = test_theme();
        let (ss, st) = test_syntax();
        for find_replace in [false, true] {
            let mut editor = EditorState::new("abc中dddd\nmore\nmore", "test.txt".into());
            editor.update_viewport(4, 1);
            editor.toggle_wrap();
            editor.set_cursor_position(0, 3);
            if find_replace {
                editor.open_find_replace();
            }
            let area = Rect::new(0, 0, 7, if find_replace { 3 } else { 1 });
            let mut before = Buffer::empty(area);
            EditorWidget::new(&editor, &theme, &ss, &st).render(area, &mut before);
            assert_eq!(before[(3, 0)].bg, theme.editor_cursor_bg);
            editor.delete_char_at();
            editor.update_viewport(4, 1);
            let mut after = Buffer::empty(area);
            EditorWidget::new(&editor, &theme, &ss, &st).render(area, &mut after);
            assert_eq!(editor.cursor_col, 3);
            assert_eq!(editor.scroll_offset, 0);
            assert_eq!(after[(6, 0)].symbol(), "d");
            assert_eq!(after[(6, 0)].bg, theme.editor_cursor_bg);
        }
    }

    #[test]
    fn long_line_navigation_and_resize_keep_cursor_in_code_viewport() {
        let mut editor = EditorState::new(&"a".repeat(200), "test.txt".into());
        editor.visible_width = 7;
        editor.visible_height = 2;
        editor.move_end();
        assert_eq!(editor.horizontal_offset, 194);
        let theme = test_theme();
        let (ss, st) = test_syntax();
        for width in [7, 1, 20] {
            editor.visible_width = width;
            editor.ensure_cursor_visible();
            let area = Rect::new(2, 1, width as u16 + 3, 2);
            let mut buf = Buffer::empty(area);
            EditorWidget::new(&editor, &theme, &ss, &st).render(area, &mut buf);
            let screen_col = area.x + 3 + (200 - editor.horizontal_offset) as u16;
            assert!(screen_col >= area.x + 3);
            assert!(screen_col < area.right());
            assert_eq!(buf[(screen_col, 1)].bg, theme.editor_cursor_bg);
        }
        editor.move_home();
        assert_eq!(editor.horizontal_offset, 0);
    }

    #[test]
    fn exact_width_wrap_has_eol_cursor_row_and_blank_gutter() {
        let mut editor = EditorState::new("abcd", "test.txt".into());
        editor.visible_width = 4;
        editor.visible_height = 3;
        editor.toggle_wrap();
        editor.move_end();
        assert_eq!(editor.cursor_visual_row(), 1);
        assert_eq!(editor.visual_row_count(), 2);
        let theme = test_theme();
        let (ss, st) = test_syntax();
        let area = Rect::new(0, 0, 7, 3);
        let mut buf = Buffer::empty(area);
        EditorWidget::new(&editor, &theme, &ss, &st).render(area, &mut buf);
        assert_eq!(buf[(0, 1)].symbol(), " ");
        assert_eq!(buf[(2, 1)].symbol(), "│");
        assert_eq!(buf[(3, 1)].bg, theme.editor_cursor_bg);
        editor.toggle_wrap();
        assert_eq!(editor.horizontal_offset, 1);
        assert_eq!(editor.scroll_offset, 0);
    }

    #[test]
    fn wrapped_vertical_navigation_and_selection_use_visual_rows() {
        let mut editor = EditorState::new("abcdefghij\nXYZ", "test.txt".into());
        editor.visible_width = 4;
        editor.visible_height = 2;
        editor.toggle_wrap();
        editor.move_right();
        editor.move_down();
        assert_eq!(
            editor.cursor_position(),
            crate::text::TextPosition { line: 0, byte: 5 }
        );
        editor.select_down();
        assert_eq!(
            editor.cursor_position(),
            crate::text::TextPosition { line: 0, byte: 9 }
        );
        assert_eq!(editor.selected_text(), "fghi");
        editor.move_up();
        assert_eq!(editor.cursor_col, 5);
        editor.page_down();
        assert_eq!(
            editor.cursor_position(),
            crate::text::TextPosition { line: 1, byte: 1 }
        );
    }

    #[test]
    fn editor_clipped_and_wrapped_syntax_keeps_font_style() {
        let mut editor = EditorState::new("let variable = 1;", "test.rs".into());
        editor.visible_width = 4;
        editor.visible_height = 6;
        editor.horizontal_offset = 4;
        let theme = test_theme();
        let (ss, mut st) = test_syntax();
        st.scopes = vec![syntect::highlighting::ThemeItem {
            scope: "source.rust".parse().unwrap(),
            style: syntect::highlighting::StyleModifier {
                foreground: None,
                background: None,
                font_style: Some(syntect::highlighting::FontStyle::ITALIC),
            },
        }];
        let area = Rect::new(0, 0, 7, 6);
        let mut buf = Buffer::empty(area);
        EditorWidget::new(&editor, &theme, &ss, &st).render(area, &mut buf);
        assert!(buf[(3, 0)].modifier.contains(Modifier::ITALIC));
        editor.toggle_wrap();
        let mut buf = Buffer::empty(area);
        EditorWidget::new(&editor, &theme, &ss, &st).render(area, &mut buf);
        assert!(buf[(3, 1)].modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn viewport_update_follows_edits_not_intentional_unchanged_scroll() {
        let mut editor = EditorState::new(
            &(0..100).map(|_| "abcdef").collect::<Vec<_>>().join("\n"),
            "test.txt".into(),
        );
        editor.update_viewport(4, 3);
        editor.set_cursor_position(50, 6);
        editor.scroll_offset = 10;
        editor.update_viewport(4, 3);
        assert_eq!(editor.scroll_offset, 10);
        editor.insert_char('X');
        editor.update_viewport(4, 3);
        assert!(editor.scroll_offset <= 50 && editor.scroll_offset + 3 > 50);
        assert!(7 - editor.horizontal_offset < 4);
        editor.update_viewport(10, 10);
        assert_eq!(editor.horizontal_offset, 0);
    }

    #[test]
    fn clipped_byte_selection_and_find_style_graphemes() {
        let mut editor = EditorState::new("a\t中e\u{301}👩‍💻Z", "test.txt".into());
        editor.visible_width = 5;
        editor.cursor_col = editor.buffer[0].len();
        editor.horizontal_offset = 2;
        editor.selection = Some(crate::editor::Selection::new(0, 1));
        editor.open_find();
        editor.find_state.query = "\u{301}".into();
        editor.update_find_matches();
        let theme = test_theme();
        let (ss, st) = test_syntax();
        let area = Rect::new(0, 0, 8, 3);
        let mut buf = Buffer::empty(area);
        EditorWidget::new(&editor, &theme, &ss, &st).render(area, &mut buf);
        assert_eq!(buf[(3, 0)].symbol(), " ");
        assert_eq!(buf[(3, 0)].bg, theme.editor_selection_bg);
        assert_eq!(buf[(5, 0)].symbol(), "中");
        assert_eq!(buf[(7, 0)].symbol(), "e\u{301}");
        assert_eq!(buf[(7, 0)].bg, theme.editor_find_match_bg);
    }

    #[test]
    fn zero_area_and_narrow_find_replace_are_safe() {
        let mut editor = EditorState::new("", "test.txt".into());
        editor.open_find_replace();
        let theme = test_theme();
        let (ss, st) = test_syntax();
        for width in 0..6 {
            for height in 0..4 {
                let area = Rect::new(0, 0, width, height);
                let mut buf = Buffer::empty(area);
                EditorWidget::new(&editor, &theme, &ss, &st).render(area, &mut buf);
            }
        }
        editor.scroll_offset = 100;
        editor.horizontal_offset = 100;
        editor.visible_height = 0;
        editor.visible_width = 0;
        editor.ensure_cursor_visible();
        assert_eq!(editor.scroll_offset, 0);
        assert_eq!(editor.horizontal_offset, 0);
    }

    fn test_syntax() -> (SyntaxSet, Theme) {
        let ss = SyntaxSet::load_defaults_nonewlines();
        let ts = ThemeSet::load_defaults();
        let theme = ts.themes["base16-ocean.dark"].clone();
        (ss, theme)
    }

    fn code_fg_set(buf: &Buffer, editor: &EditorState, width: u16) -> Vec<ratatui::style::Color> {
        let gutter = editor.gutter_width();
        (gutter..width).map(|x| buf[(x, 0)].fg).collect()
    }

    #[test]
    fn prepared_only_without_cache_never_runs_render_highlighter() {
        let mut editor = EditorState::new("fn main() {}\n", "test.rs".into());
        editor.update_viewport(20, 3);
        editor.set_cursor_position(1, 0);
        let theme = test_theme();
        let (ss, st) = test_syntax();
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        EditorWidget::new(&editor, &theme, &ss, &st)
            .prepared_only(true)
            .render(area, &mut buf);
        for fg in code_fg_set(&buf, &editor, 20) {
            assert!(
                fg == theme.status_fg || fg == ratatui::style::Color::Reset,
                "prepared pipeline without a cache must render pending, not highlighted: {fg:?}"
            );
        }
    }

    #[test]
    fn prepared_cache_renders_prepared_runs_and_pending_never_stale() {
        let mut editor = EditorState::new("fn main() {}\nlet x = 1;\n", "test.rs".into());
        editor.update_viewport(20, 3);
        // Keep the cursor off row 0 so its cell styling never masks syntax color.
        editor.set_cursor_position(1, 0);
        let theme = test_theme();
        let (ss, st) = test_syntax();
        let syntax = ss.find_syntax_by_name("Rust").unwrap();
        let cache = crate::highlighting::SyntaxCache::build(
            &editor.buffer,
            syntax,
            "Rust",
            &ss,
            &st,
            7,
            9,
            1 << 20,
        );
        assert!(cache.is_prepared(0));
        let area = Rect::new(0, 0, 20, 3);
        let mut prepared_buf = Buffer::empty(area);
        EditorWidget::new(&editor, &theme, &ss, &st)
            .prepared(&cache)
            .render(area, &mut prepared_buf);
        let prepared_fg = code_fg_set(&prepared_buf, &editor, 20);
        assert!(
            prepared_fg
                .iter()
                .any(|fg| *fg != theme.status_fg && *fg != ratatui::style::Color::Reset),
            "prepared runs must color at least one code cell"
        );

        // A cache with no prepared lines must render the explicit pending style
        // for every code cell (never stale runs).
        let empty = crate::highlighting::SyntaxCache::default();
        assert!(empty.runs(0).is_none());
        let mut pending_buf = Buffer::empty(area);
        EditorWidget::new(&editor, &theme, &ss, &st)
            .prepared(&empty)
            .render(area, &mut pending_buf);
        for fg in code_fg_set(&pending_buf, &editor, 20) {
            assert!(
                fg == theme.status_fg || fg == ratatui::style::Color::Reset,
                "unprepared lines render pending, not stale: {fg:?}"
            );
        }
    }

    #[test]
    fn unicode_widget_selection_find_and_eol_style_all_cells() {
        let mut editor = EditorState::new("\t中e\u{301}🙂", "test.txt".into());
        editor.set_cursor_position(0, 1);
        editor.select_right();
        let theme = test_theme();
        let (ss, st) = test_syntax();
        let area = Rect::new(0, 0, 30, 3);
        let mut buf = Buffer::empty(area);
        EditorWidget::new(&editor, &theme, &ss, &st).render(area, &mut buf);
        assert_eq!(buf[(7, 0)].bg, theme.editor_selection_bg);
        assert_eq!(buf[(8, 0)].bg, theme.editor_selection_bg);
        assert_eq!(buf[(9, 0)].bg, theme.editor_cursor_bg);
        editor.selection = None;
        editor.move_end();
        editor.open_find();
        editor.find_state.query = "\u{301}🙂".into();
        editor.update_find_matches();
        let mut buf = Buffer::empty(area);
        EditorWidget::new(&editor, &theme, &ss, &st).render(area, &mut buf);
        for x in 9..12 {
            assert_eq!(buf[(x, 0)].bg, theme.editor_find_match_bg);
        }
        assert_eq!(buf[(12, 0)].bg, theme.editor_cursor_bg);
    }

    #[test]
    fn unicode_widget_tabs_graphemes_and_cursor_cells() {
        let mut editor = EditorState::new("\t中e\u{301}🙂", "test.txt".into());
        editor.set_cursor_position(0, 7);
        let theme = test_theme();
        let (ss, st) = test_syntax();
        let area = Rect::new(0, 0, 30, 3);
        let mut buf = Buffer::empty(area);
        EditorWidget::new(&editor, &theme, &ss, &st).render(area, &mut buf);
        assert_eq!(buf[(7, 0)].symbol(), "中");
        assert_eq!(buf[(9, 0)].symbol(), "e\u{301}");
        assert_eq!(buf[(10, 0)].symbol(), "🙂");
        assert_eq!(buf[(10, 0)].bg, theme.editor_cursor_bg);
        assert_eq!(buf[(11, 0)].bg, theme.editor_cursor_bg);
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
