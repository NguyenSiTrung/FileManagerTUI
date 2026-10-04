//! Document positions are UTF-8 bytes; terminal positions are display cells.
/// A document position independent of display width, wrapping, and scrolling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPosition {
    pub line: usize,
    pub byte: usize,
}

/// Logical-line tab stops are four terminal display cells apart.
pub const TAB_WIDTH: usize = 4;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Interior bytes snap to the beginning of their extended grapheme.
pub fn floor_grapheme_boundary(text: &str, byte: usize) -> usize {
    if byte >= text.len() {
        return text.len();
    }
    text.grapheme_indices(true)
        .map(|(i, _)| i)
        .take_while(|&i| i <= byte)
        .last()
        .unwrap_or(0)
}
/// Previous extended-grapheme byte boundary, clamped at the start of the line.
pub fn previous_grapheme_boundary(text: &str, byte: usize) -> usize {
    text.grapheme_indices(true)
        .map(|(i, _)| i)
        .take_while(|&i| i < byte.min(text.len()))
        .last()
        .unwrap_or(0)
}
/// Next extended-grapheme byte boundary, clamped at EOL.
pub fn next_grapheme_boundary(text: &str, byte: usize) -> usize {
    text.grapheme_indices(true)
        .map(|(i, _)| i)
        .find(|&i| i > byte)
        .unwrap_or(text.len())
}
/// Display width at a logical-line cell column; tabs advance to the next stop.
pub fn grapheme_width(grapheme: &str, col: usize, tab_width: usize) -> usize {
    if grapheme == "\t" {
        let width = tab_width.max(1);
        width - col % width
    } else {
        UnicodeWidthStr::width(grapheme)
    }
}
/// Interior bytes snap left; bytes beyond EOL clamp to EOL.
pub fn byte_to_display_col(text: &str, byte: usize, tab_width: usize) -> usize {
    let mut col = 0;
    for (i, g) in text.grapheme_indices(true) {
        if i + g.len() > byte {
            break;
        }
        col += grapheme_width(g, col, tab_width);
    }
    col
}
/// Cells inside tabs/wide graphemes snap left; cells beyond EOL clamp to EOL.
pub fn display_col_to_byte(text: &str, col: usize, tab_width: usize) -> usize {
    let mut display = 0;
    for (i, g) in text.grapheme_indices(true) {
        let width = grapheme_width(g, display, tab_width);
        if col < display + width {
            return i;
        }
        display += width;
    }
    text.len()
}

/// One extended grapheme and its UTF-8 byte and logical display-cell ranges.
#[derive(Debug, Clone, Copy)]
pub struct DisplayGrapheme<'a> {
    pub byte: usize,
    pub text: &'a str,
    pub start: usize,
    pub end: usize,
}

/// Stream logical-line graphemes without allocating glyphs. Tab stops never reset
/// at a visual-row boundary.
pub fn display_graphemes(text: &str) -> impl Iterator<Item = DisplayGrapheme<'_>> {
    display_graphemes_from(text, 0, 0)
}

fn display_graphemes_from(
    text: &str,
    byte_offset: usize,
    mut col: usize,
) -> impl Iterator<Item = DisplayGrapheme<'_>> {
    text[byte_offset..]
        .grapheme_indices(true)
        .map(move |(byte, text)| {
            let start = col;
            col += grapheme_width(text, col, TAB_WIDTH);
            DisplayGrapheme {
                byte: byte + byte_offset,
                text,
                start,
                end: col,
            }
        })
}

/// A visual row's half-open logical display-cell range. A short row before a
/// wide grapheme has blank padding; that padding maps to `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualRow {
    pub start: usize,
    pub end: usize,
    byte_start: usize,
    byte_col: usize,
}

impl VisualRow {
    /// Map a clipped horizontal window once, retaining a byte hint so painting
    /// does not rescan the logical-line prefix on every wrapped continuation.
    pub fn window(text: &str, start: usize, end: usize) -> Self {
        let glyph = display_graphemes(text).find(|g| g.end > start);
        Self {
            start,
            end,
            byte_start: glyph.map(|g| g.byte).unwrap_or(text.len()),
            byte_col: glyph.map(|g| g.start).unwrap_or(start),
        }
    }
}

/// Stream visual rows, moving wide graphemes intact to the next row when possible.
/// Tabs may span rows; oversized graphemes are clipped to blank cells. `eol_slot`
/// adds a cursor row when EOL falls exactly on a wrap boundary. Width zero yields
/// one empty row so callers can safely map an empty viewport.
pub fn visual_rows(
    text: &str,
    width: usize,
    wrap: bool,
    eol_slot: bool,
) -> impl Iterator<Item = VisualRow> + '_ {
    let mut glyphs = display_graphemes(text).peekable();
    let mut start: usize = 0;
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        if !wrap || width == 0 {
            done = true;
            return Some(VisualRow {
                start: 0,
                end: if width == 0 {
                    0
                } else {
                    byte_to_display_col(text, text.len(), TAB_WIDTH)
                },
                byte_start: 0,
                byte_col: 0,
            });
        }
        let byte_start = glyphs.peek().map(|g| g.byte).unwrap_or(text.len());
        let byte_col = glyphs.peek().map(|g| g.start).unwrap_or(start);
        let limit = start.saturating_add(width);
        loop {
            let Some(g) = glyphs.peek() else {
                done = true;
                return Some(VisualRow {
                    start,
                    end: start,
                    byte_start,
                    byte_col,
                });
            };
            if g.end <= start {
                glyphs.next();
                continue;
            }
            if g.end > limit {
                let end = if g.text != "\t" && g.end - g.start <= width && g.start > start {
                    g.start
                } else {
                    limit
                };
                let row = VisualRow {
                    start,
                    end,
                    byte_start,
                    byte_col,
                };
                start = end;
                return Some(row);
            }
            let end = g.end;
            glyphs.next();
            if end == limit {
                let row = VisualRow {
                    start,
                    end,
                    byte_start,
                    byte_col,
                };
                start = end;
                if glyphs.peek().is_none() && !eol_slot {
                    done = true;
                }
                return Some(row);
            }
            if glyphs.peek().is_none() {
                done = true;
                return Some(VisualRow {
                    start,
                    end,
                    byte_start,
                    byte_col,
                });
            }
        }
    })
}

/// Copy a half-open display-cell slice, expanding tabs. A clipped wide grapheme
/// contributes blanks rather than a phantom or out-of-range glyph.
pub fn display_slice(text: &str, start: usize, end: usize) -> String {
    let mut result = String::new();
    for g in display_graphemes(text) {
        if g.start >= end {
            break;
        }
        let left = g.start.max(start);
        let right = g.end.min(end);
        if right <= left {
            continue;
        }
        if g.text == "\t" || left != g.start || right != g.end {
            result.extend(std::iter::repeat_n(' ', right - left));
        } else {
            result.push_str(g.text);
        }
    }
    result
}

/// Paint a clipped logical display range. Every cell is cleared first, including
/// wide continuation cells. Styles are supplied by byte range, not scalar index.
pub fn paint_display_row(
    text: &str,
    row: VisualRow,
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
    base: ratatui::style::Style,
    mut style: impl FnMut(DisplayGrapheme<'_>) -> ratatui::style::Style,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    for x in area.x..area.right() {
        buf[(x, area.y)].reset();
        buf[(x, area.y)].set_style(base);
    }
    let end = row.end.min(row.start.saturating_add(area.width as usize));
    for g in display_graphemes_from(text, row.byte_start, row.byte_col) {
        if g.start >= end {
            break;
        }
        let left = g.start.max(row.start);
        let right = g.end.min(end);
        if right <= left {
            continue;
        }
        let style = style(g);
        for col in left..right {
            buf[(area.x + (col - row.start) as u16, area.y)].set_style(style);
        }
        if g.text != "\t" && left == g.start && right == g.end {
            buf.set_string(area.x + (left - row.start) as u16, area.y, g.text, style);
            for col in left..right {
                buf[(area.x + (col - row.start) as u16, area.y)].set_style(style);
            }
        }
    }
}

/// Flatten only one styled logical line for grapheme/byte mapping.
pub fn line_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Preserve line and span styles when a grapheme crosses syntax-span boundaries.
pub fn line_style_at_byte(line: &ratatui::text::Line<'_>, byte: usize) -> ratatui::style::Style {
    let mut offset = 0;
    for span in &line.spans {
        if byte < offset + span.content.len() {
            return line.style.patch(span.style);
        }
        offset += span.content.len();
    }
    line.style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_ranges_keep_wide_graphemes_and_logical_tab_stops() {
        let ranges = |text, width, eol| {
            visual_rows(text, width, true, eol)
                .map(|r| (r.start, r.end))
                .collect::<Vec<_>>()
        };
        assert_eq!(ranges("abc中\tZ", 4, false), vec![(0, 3), (3, 7), (7, 9)]);
        assert_eq!(ranges("abcd", 4, true), vec![(0, 4), (4, 4)]);
        assert_eq!(ranges("abcd", 4, false), vec![(0, 4)]);
        assert_eq!(ranges("\tX", 3, false), vec![(0, 3), (3, 5)]);
        assert_eq!(ranges("中", 1, false), vec![(0, 1), (1, 2)]);
        assert_eq!(ranges("", 4, true), vec![(0, 0)]);
        assert_eq!(ranges("abc", 0, true), vec![(0, 0)]);
        assert_eq!(ranges("\u{301}", 4, false), vec![(0, 0)]);
        assert_eq!(visual_rows("abc", 1, false, true).count(), 1);
    }

    #[test]
    fn clipping_clears_partial_wide_glyphs_and_continuation_cells() {
        use ratatui::{
            buffer::Buffer,
            layout::Rect,
            style::{Color, Style},
        };
        let area = Rect::new(0, 0, 4, 1);
        for (text, start, expected) in [
            ("a\t中X", 2, vec![" ", " ", "中", " "]),
            ("a\t中X", 5, vec![" ", "X", " ", " "]),
            ("e\u{301}👩‍💻Z", 0, vec!["e\u{301}", "👩‍💻", " ", "Z"]),
            ("abc中", 0, vec!["a", "b", "c", " "]),
        ] {
            let mut buf = Buffer::empty(area);
            for x in 0..4 {
                buf.set_string(x, 0, "X", Style::default());
            }
            let row = VisualRow::window(text, start, start + 4);
            paint_display_row(text, row, area, &mut buf, Style::default(), |_| {
                Style::default().fg(Color::Red)
            });
            for (x, symbol) in expected.iter().enumerate() {
                assert_eq!(buf[(x as u16, 0)].symbol(), *symbol);
            }
        }
        let mut buf = Buffer::empty(area);
        paint_display_row(
            "",
            VisualRow::window("", 0, 0),
            Rect::new(0, 0, 0, 0),
            &mut buf,
            Style::default(),
            |_| unreachable!(),
        );
    }

    #[test]
    fn display_copy_and_style_byte_mapping_preserve_graphemes() {
        use ratatui::{
            style::{Color, Modifier, Style},
            text::{Line, Span},
        };
        let line = Line::from(vec![
            Span::styled("e", Style::default().fg(Color::Red)),
            Span::styled("\u{301}中", Style::default().fg(Color::Blue)),
        ])
        .style(Style::default().add_modifier(Modifier::BOLD));
        let text = line_text(&line);
        assert_eq!(display_slice(&text, 0, 3), "e\u{301}中");
        assert_eq!(display_slice(&text, 2, 3), " ");
        assert_eq!(display_slice(&text, 99, 100), "");
        assert_eq!(display_slice(&text, 3, 2), "");
        assert_eq!(line_style_at_byte(&line, 0).fg, Some(Color::Red));
        assert_eq!(line_style_at_byte(&line, 3).fg, Some(Color::Blue));
        assert!(line_style_at_byte(&line, 99)
            .add_modifier
            .contains(Modifier::BOLD));
    }
    #[test]
    fn grapheme_navigation_clamps_and_skips_whole_clusters() {
        let text = "中e\u{301}👩‍💻";
        for (byte, floor, previous, next) in [
            (0, 0, 0, 3),
            (2, 0, 0, 3),
            (3, 3, 0, 6),
            (5, 3, 3, 6),
            (6, 6, 3, 17),
            (17, 17, 6, 17),
            (99, 17, 6, 17),
        ] {
            assert_eq!(floor_grapheme_boundary(text, byte), floor);
            assert_eq!(previous_grapheme_boundary(text, byte), previous);
            assert_eq!(next_grapheme_boundary(text, byte), next);
        }
        assert_eq!(display_col_to_byte("a\tb", 3, 4), 1);
        assert_eq!(byte_to_display_col("a\tb", 2, 4), 4);
        assert_eq!(byte_to_display_col("\t", 1, 0), 1);
        assert_eq!(previous_grapheme_boundary("", 1), 0);
        assert_eq!(next_grapheme_boundary("", 0), 0);
    }

    #[test]
    fn tab_and_wide_text_map_back_to_byte_boundaries() {
        let text = "\t中a";
        assert_eq!(byte_to_display_col(text, 1, 4), 4);
        assert_eq!(byte_to_display_col(text, 4, 4), 6);
        assert_eq!(display_col_to_byte(text, 6, 4), 4);
    }
    #[test]
    fn combining_emoji_and_interior_cells_snap_left() {
        let text = "e\u{301}👩‍💻中";
        assert_eq!(byte_to_display_col(text, 3, 4), 1);
        assert_eq!(display_col_to_byte(text, 2, 4), 3);
        assert_eq!(byte_to_display_col(text, text.len(), 4), 5);
        assert_eq!(display_col_to_byte(text, 99, 4), text.len());
        assert_eq!(display_col_to_byte("", 9, 0), 0);
        assert_eq!(byte_to_display_col("abc", 2, 4), 2);
    }
}
