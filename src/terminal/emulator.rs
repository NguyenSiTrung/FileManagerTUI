//! Terminal emulator: ANSI escape sequence parser + screen buffer.
//!
//! Uses the `vte` crate (from Alacritty) to parse ANSI sequences and
//! maintains a grid of cells that map to ratatui styled spans for rendering.
//!
//! Phase 9 Task 1 adapted the emulator surface to cover alternate screens,
//! cursor visibility/shape modes, DSR/DA replies, DECSTBM scroll regions,
//! wide/combining glyphs, and mid-sequence resizing. The parser and grid stay
//! in this module so every fixture has a local survival mechanism and the
//! existing callers (`src/components/terminal.rs`, the PTY path) keep working.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

pub const MAX_SCROLLBACK_LINES: usize = 100_000;

/// Cursor rendering shape requested through DECSCUSR (`CSI Ps SP q`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

/// A single character cell in the terminal grid.
#[derive(Debug, Clone)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub modifiers: Modifier,
    /// Zero-width combining marks attached to the base `ch`.
    pub combining: String,
    /// True when this cell starts a double-width glyph.
    pub wide: bool,
    /// True when this cell is the trailing half of the preceding wide glyph.
    pub continuation: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Color::Reset,
            bg: Color::Reset,
            modifiers: Modifier::empty(),
            combining: String::new(),
            wide: false,
            continuation: false,
        }
    }
}

impl Cell {
    /// The rendered grapheme for this cell: the base char plus combining marks,
    /// or an empty string for the trailing half of a wide glyph.
    fn glyph(&self) -> String {
        if self.continuation {
            String::new()
        } else if self.combining.is_empty() {
            self.ch.to_string()
        } else {
            format!("{}{}", self.ch, self.combining)
        }
    }
}

/// Primary screen state saved while the alternate screen is active.
struct SavedPrimary {
    grid: Vec<Vec<Cell>>,
    cursor_row: usize,
    cursor_col: usize,
    saved_cursor: Option<(usize, usize)>,
}

/// The terminal emulator with screen buffer and VTE parser.
pub struct TerminalEmulator {
    /// Current visible screen grid (rows × cols).
    grid: Vec<Vec<Cell>>,
    /// Scrollback buffer (oldest lines first).
    scrollback: Vec<Vec<Cell>>,
    /// Maximum scrollback lines.
    max_scrollback: usize,
    /// Cursor row (0-based, relative to visible grid).
    cursor_row: usize,
    /// Cursor column (0-based, relative to visible grid).
    cursor_col: usize,
    /// Number of visible rows.
    rows: usize,
    /// Number of visible columns.
    cols: usize,
    /// Current SGR style.
    current_fg: Color,
    current_bg: Color,
    current_modifiers: Modifier,
    /// VTE state machine parser.
    parser: vte::Parser,
    /// Saved cursor position (for ESC 7 / ESC 8).
    saved_cursor: Option<(usize, usize)>,
    /// Bytes the emulator owes the child (DSR/DA replies), oldest first.
    replies: Vec<u8>,
    /// DEC private mode 25 (DECTCEM) cursor visibility.
    cursor_visible: bool,
    /// DECSCUSR cursor shape.
    cursor_shape: CursorShape,
    /// Top margin of the scrolling region (inclusive, 0-based).
    scroll_top: usize,
    /// Bottom margin of the scrolling region (inclusive, 0-based).
    scroll_bottom: usize,
    /// True while the alternate screen buffer is displayed.
    alternate: bool,
    /// Primary screen saved when the alternate screen is entered.
    saved_primary: Option<SavedPrimary>,
}

impl TerminalEmulator {
    /// Create a new terminal emulator with the given dimensions.
    pub fn new(rows: usize, cols: usize) -> Self {
        let grid = vec![vec![Cell::default(); cols]; rows];
        Self {
            grid,
            scrollback: Vec::new(),
            max_scrollback: 1000,
            cursor_row: 0,
            cursor_col: 0,
            rows,
            cols,
            current_fg: Color::Reset,
            current_bg: Color::Reset,
            current_modifiers: Modifier::empty(),
            parser: vte::Parser::new(),
            saved_cursor: None,
            replies: Vec::new(),
            cursor_visible: true,
            cursor_shape: CursorShape::Block,
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            alternate: false,
            saved_primary: None,
        }
    }

    /// Maximum retained history, bounded to 0..=100,000 lines.
    pub fn scrollback_limit(&self) -> usize {
        self.max_scrollback
    }

    /// Change history capacity without touching the grid, cursor or parser.
    /// Returns the number of oldest history rows removed.
    pub fn set_scrollback_limit(&mut self, limit: usize) -> usize {
        self.max_scrollback = limit.min(MAX_SCROLLBACK_LINES);
        let removed = self.scrollback.len().saturating_sub(self.max_scrollback);
        self.scrollback.drain(..removed);
        removed
    }

    /// Process raw bytes from the PTY through the VTE parser.
    pub fn process(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        // Move the parser out so the performer can borrow the whole emulator.
        // Parser state survives across calls (including resizes between them).
        let mut parser = std::mem::replace(&mut self.parser, vte::Parser::new());
        for &byte in data {
            parser.advance(&mut Performer { emu: self }, byte);
        }
        self.parser = parser;
    }

    /// Take the reply bytes (DSR/DA responses) produced so far, oldest first.
    ///
    /// The coordinator returns these to the child; the queue is drained so each
    /// reply is delivered exactly once.
    #[allow(dead_code)]
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    /// Whether the child asked for the text cursor to be shown (DEC mode 25).
    pub fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    /// The cursor shape requested through DECSCUSR.
    #[allow(dead_code)]
    pub fn cursor_shape(&self) -> CursorShape {
        self.cursor_shape
    }

    /// Whether the alternate screen buffer is currently displayed.
    #[allow(dead_code)]
    pub fn alternate_screen(&self) -> bool {
        self.alternate
    }

    /// The active scrolling region as an inclusive `(top, bottom)` row pair.
    #[allow(dead_code)]
    pub fn scroll_region(&self) -> (usize, usize) {
        (self.scroll_top, self.scroll_bottom)
    }

    /// Resize the emulator grid.
    pub fn resize(&mut self, new_rows: usize, new_cols: usize) {
        let resized = resize_grid(&self.grid, new_rows, new_cols);
        self.grid = resized;
        self.rows = new_rows;
        self.cols = new_cols;
        // Clamp cursor
        self.cursor_row = self.cursor_row.min(new_rows.saturating_sub(1));
        self.cursor_col = self.cursor_col.min(new_cols.saturating_sub(1));
        // Reset the scrolling region to the new full screen.
        self.scroll_top = 0;
        self.scroll_bottom = new_rows.saturating_sub(1);
        // The hidden primary screen must follow the new geometry too.
        if let Some(saved) = self.saved_primary.as_mut() {
            saved.grid = resize_grid(&saved.grid, new_rows, new_cols);
            saved.cursor_row = saved.cursor_row.min(new_rows.saturating_sub(1));
            saved.cursor_col = saved.cursor_col.min(new_cols.saturating_sub(1));
        }
    }

    /// Render the visible grid as ratatui Lines (for the widget).
    pub fn render_lines(&self) -> Vec<Line<'static>> {
        self.grid.iter().map(|row| render_row(row)).collect()
    }

    /// Render lines at a given scroll offset.
    /// scroll_offset=0 shows the live grid (same as render_lines()).
    /// scroll_offset>0 shows older content from scrollback+grid.
    pub fn render_lines_at_offset(&self, scroll_offset: usize) -> Vec<Line<'static>> {
        if scroll_offset == 0 {
            return self.render_lines();
        }

        let total = self.total_lines();
        let first = total.saturating_sub(self.rows + scroll_offset);
        let mut result = Vec::with_capacity(self.rows);

        for abs_line in first..first + self.rows {
            if abs_line >= total {
                // Empty line past the end
                result.push(Line::from(""));
                continue;
            }
            let row = if abs_line < self.scrollback.len() {
                &self.scrollback[abs_line]
            } else {
                &self.grid[abs_line - self.scrollback.len()]
            };
            result.push(render_row(row));
        }
        result
    }

    /// Total lines including scrollback.
    #[allow(dead_code)]
    pub fn total_lines(&self) -> usize {
        self.scrollback.len() + self.rows
    }

    /// Get scrollback lines for rendering (oldest first).
    #[allow(dead_code)]
    pub fn scrollback_lines(&self) -> Vec<Line<'static>> {
        self.scrollback.iter().map(|row| render_row(row)).collect()
    }

    /// Get visible rows count.
    pub fn visible_rows(&self) -> usize {
        self.rows
    }

    /// Get visible cols count.
    pub fn visible_cols(&self) -> usize {
        self.cols
    }

    /// Get cursor position (row, col).
    pub fn cursor_position(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    /// Get number of scrollback lines.
    #[allow(dead_code)]
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// Get a reference to a cell at absolute line index and column.
    /// Absolute line 0..scrollback_len are scrollback, scrollback_len..total_lines are grid.
    /// Returns None if out of bounds.
    pub fn cell_at(&self, abs_line: usize, col: usize) -> Option<&Cell> {
        let sb_len = self.scrollback.len();
        if abs_line < sb_len {
            self.scrollback.get(abs_line).and_then(|row| row.get(col))
        } else {
            let grid_row = abs_line - sb_len;
            self.grid.get(grid_row).and_then(|row| row.get(col))
        }
    }

    /// Get number of columns in a given absolute line.
    fn line_cols(&self, abs_line: usize) -> usize {
        let sb_len = self.scrollback.len();
        if abs_line < sb_len {
            self.scrollback.get(abs_line).map_or(0, |r| r.len())
        } else {
            let grid_row = abs_line - sb_len;
            self.grid.get(grid_row).map_or(0, |r| r.len())
        }
    }

    /// Extract text from the emulator grid between two coordinates.
    /// Lines are in absolute space (scrollback + grid).
    /// Returns None if the range is invalid.
    pub fn extract_text(
        &self,
        start_line: usize,
        start_col: usize,
        end_line: usize,
        end_col: usize,
    ) -> Option<String> {
        let total = self.total_lines();
        if start_line > end_line || start_line >= total {
            return None;
        }

        let mut result = String::new();

        for line_idx in start_line..=end_line.min(total - 1) {
            let cols = self.line_cols(line_idx);
            let from = if line_idx == start_line { start_col } else { 0 };
            let to = if line_idx == end_line {
                end_col.min(cols.saturating_sub(1))
            } else {
                cols.saturating_sub(1)
            };

            let mut line_text = String::new();
            for c in from..=to {
                if let Some(cell) = self.cell_at(line_idx, c) {
                    // Skip the trailing half of a wide glyph so it is counted once.
                    if cell.continuation {
                        continue;
                    }
                    line_text.push(cell.ch);
                    line_text.push_str(&cell.combining);
                }
            }
            // Trim trailing spaces from each line
            let trimmed = line_text.trim_end();
            result.push_str(trimmed);

            if line_idx < end_line.min(total - 1) {
                result.push('\n');
            }
        }

        Some(result)
    }
}

/// Copy a grid into new geometry, preserving the overlapping top-left region.
fn resize_grid(grid: &[Vec<Cell>], new_rows: usize, new_cols: usize) -> Vec<Vec<Cell>> {
    let mut new_grid = vec![vec![Cell::default(); new_cols]; new_rows];
    for (r, row) in grid.iter().enumerate() {
        if r >= new_rows {
            break;
        }
        for (c, cell) in row.iter().enumerate() {
            if c >= new_cols {
                break;
            }
            new_grid[r][c] = cell.clone();
        }
    }
    new_grid
}

/// Render a single grid row as a ratatui line.
fn render_row(row: &[Cell]) -> Line<'static> {
    let spans: Vec<Span<'static>> = row
        .iter()
        .map(|cell| {
            let style = Style::default()
                .fg(cell.fg)
                .bg(cell.bg)
                .add_modifier(cell.modifiers);
            Span::styled(cell.glyph(), style)
        })
        .collect();
    Line::from(spans)
}

/// Internal performer struct that receives VTE callbacks.
/// It borrows the whole emulator; the parser is moved out during `process`.
struct Performer<'a> {
    emu: &'a mut TerminalEmulator,
}

impl<'a> Performer<'a> {
    /// A blank cell carrying the current SGR style.
    fn blank_cell(&self) -> Cell {
        Cell {
            ch: ' ',
            fg: self.emu.current_fg,
            bg: self.emu.current_bg,
            modifiers: self.emu.current_modifiers,
            combining: String::new(),
            wide: false,
            continuation: false,
        }
    }

    /// Scroll the active scrolling region up by `n` lines.
    /// Lines leaving the top of the full screen enter scrollback; region
    /// scrolls below the screen top do not (they are not history).
    fn scroll_up_region(&mut self, n: usize) {
        if self.emu.rows == 0 {
            return;
        }
        let top = self.emu.scroll_top.min(self.emu.rows - 1);
        let bottom = self.emu.scroll_bottom.min(self.emu.rows - 1);
        for _ in 0..n {
            let line = self.emu.grid.remove(top);
            if top == 0 && !self.emu.alternate {
                self.emu.scrollback.push(line);
                if self.emu.scrollback.len() > self.emu.max_scrollback {
                    self.emu.scrollback.remove(0);
                }
            }
            let blank = vec![Cell::default(); self.emu.cols];
            let insert_at = bottom.min(self.emu.grid.len());
            self.emu.grid.insert(insert_at, blank);
        }
    }

    /// Scroll the active scrolling region down by `n` lines.
    fn scroll_down_region(&mut self, n: usize) {
        if self.emu.rows == 0 {
            return;
        }
        let top = self.emu.scroll_top.min(self.emu.rows - 1);
        let bottom = self.emu.scroll_bottom.min(self.emu.rows - 1);
        for _ in 0..n {
            self.emu.grid.remove(bottom);
            self.emu
                .grid
                .insert(top, vec![Cell::default(); self.emu.cols]);
        }
    }

    /// Move the cursor down one row, scrolling the region at the bottom margin.
    fn index_down(&mut self) {
        if self.emu.rows == 0 {
            return;
        }
        if self.emu.cursor_row >= self.emu.scroll_bottom {
            self.scroll_up_region(1);
        } else {
            self.emu.cursor_row += 1;
        }
    }

    /// Move the cursor up one row, scrolling the region at the top margin.
    fn reverse_index(&mut self) {
        if self.emu.rows == 0 {
            return;
        }
        if self.emu.cursor_row <= self.emu.scroll_top {
            self.scroll_down_region(1);
        } else {
            self.emu.cursor_row -= 1;
        }
    }

    /// Switch to the alternate screen buffer (DEC private mode 1049/1047/47).
    fn enter_alternate(&mut self) {
        if self.emu.alternate {
            // Re-entering clears the alternate buffer and homes the cursor.
            self.emu.grid = vec![vec![Cell::default(); self.emu.cols]; self.emu.rows];
            self.emu.cursor_row = 0;
            self.emu.cursor_col = 0;
            return;
        }
        let saved = SavedPrimary {
            grid: std::mem::take(&mut self.emu.grid),
            cursor_row: self.emu.cursor_row,
            cursor_col: self.emu.cursor_col,
            saved_cursor: self.emu.saved_cursor,
        };
        self.emu.grid = vec![vec![Cell::default(); self.emu.cols]; self.emu.rows];
        self.emu.cursor_row = 0;
        self.emu.cursor_col = 0;
        self.emu.saved_cursor = None;
        self.emu.saved_primary = Some(saved);
        self.emu.alternate = true;
        self.emu.scroll_top = 0;
        self.emu.scroll_bottom = self.emu.rows.saturating_sub(1);
    }

    /// Restore the primary screen buffer.
    fn leave_alternate(&mut self) {
        if !self.emu.alternate {
            return;
        }
        if let Some(saved) = self.emu.saved_primary.take() {
            self.emu.grid = saved.grid;
            self.emu.cursor_row = saved.cursor_row.min(self.emu.rows.saturating_sub(1));
            self.emu.cursor_col = saved.cursor_col.min(self.emu.cols.saturating_sub(1));
            self.emu.saved_cursor = saved.saved_cursor;
        }
        self.emu.alternate = false;
        self.emu.scroll_top = 0;
        self.emu.scroll_bottom = self.emu.rows.saturating_sub(1);
    }

    /// Full reset (RIS).
    fn full_reset(&mut self) {
        self.emu.current_fg = Color::Reset;
        self.emu.current_bg = Color::Reset;
        self.emu.current_modifiers = Modifier::empty();
        self.emu.cursor_row = 0;
        self.emu.cursor_col = 0;
        self.emu.saved_cursor = None;
        self.emu.cursor_visible = true;
        self.emu.cursor_shape = CursorShape::Block;
        self.emu.alternate = false;
        self.emu.saved_primary = None;
        self.emu.replies.clear();
        self.emu.scroll_top = 0;
        self.emu.scroll_bottom = self.emu.rows.saturating_sub(1);
        for r in 0..self.emu.rows {
            for c in 0..self.emu.cols {
                self.emu.grid[r][c] = Cell::default();
            }
        }
    }

    /// Handle DEC private mode set/reset (with the `?` intermediate).
    fn set_private_mode(&mut self, mode: u16, enabled: bool) {
        match mode {
            25 => self.emu.cursor_visible = enabled,
            47 | 1047 | 1049 => {
                if enabled {
                    self.enter_alternate();
                } else {
                    self.leave_alternate();
                }
            }
            _ => {}
        }
    }
}

impl<'a> vte::Perform for Performer<'a> {
    /// Handle printable characters.
    fn print(&mut self, c: char) {
        if self.emu.rows == 0 || self.emu.cols == 0 {
            return;
        }
        let width = UnicodeWidthChar::width(c).unwrap_or(0);

        // Zero-width combining marks attach to the preceding base cell.
        if width == 0 {
            if self.emu.cursor_col == 0 {
                return;
            }
            let row = self.emu.cursor_row.min(self.emu.rows - 1);
            let mut target = self.emu.cursor_col - 1;
            if target >= self.emu.cols {
                target = self.emu.cols - 1;
            }
            if self.emu.grid[row][target].continuation && target > 0 {
                target -= 1;
            }
            self.emu.grid[row][target].combining.push(c);
            return;
        }

        if self.emu.cursor_col >= self.emu.cols
            || (width == 2 && self.emu.cursor_col + 1 >= self.emu.cols)
        {
            // Line wrap
            self.emu.cursor_col = 0;
            self.index_down();
        }

        let row = self.emu.cursor_row;
        let col = self.emu.cursor_col;
        if row < self.emu.rows && col < self.emu.cols {
            let mut cell = self.blank_cell();
            cell.ch = c;
            cell.wide = width == 2;
            self.emu.grid[row][col] = cell;
            if width == 2 && col + 1 < self.emu.cols {
                let mut cont = self.blank_cell();
                cont.continuation = true;
                self.emu.grid[row][col + 1] = cont;
            }
        }
        self.emu.cursor_col += width;
    }

    /// Handle control characters.
    fn execute(&mut self, byte: u8) {
        match byte {
            // Carriage Return
            b'\r' => {
                self.emu.cursor_col = 0;
            }
            // Line Feed / Newline
            b'\n' => {
                self.index_down();
            }
            // Backspace
            0x08 => {
                if self.emu.cursor_col > 0 {
                    self.emu.cursor_col -= 1;
                }
            }
            // Tab
            b'\t' => {
                if self.emu.cols > 0 {
                    let tab_stop = (self.emu.cursor_col + 8) & !7;
                    self.emu.cursor_col = tab_stop.min(self.emu.cols - 1);
                }
            }
            // Bell
            0x07 => {
                // Ignore bell
            }
            _ => {}
        }
    }

    /// Handle CSI sequences (cursor movement, erase, SGR, etc).
    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        let params_vec: Vec<u16> = params.iter().flat_map(|sub| sub.iter().copied()).collect();
        let private = intermediates.first() == Some(&b'?');

        match action {
            // Cursor Up (CUU)
            'A' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                self.emu.cursor_row = self.emu.cursor_row.saturating_sub(n);
            }
            // Cursor Down (CUD)
            'B' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                self.emu.cursor_row = (self.emu.cursor_row + n).min(self.emu.rows - 1);
            }
            // Cursor Forward (CUF)
            'C' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                self.emu.cursor_col = (self.emu.cursor_col + n).min(self.emu.cols - 1);
            }
            // Cursor Back (CUB)
            'D' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                self.emu.cursor_col = self.emu.cursor_col.saturating_sub(n);
            }
            // Cursor Position (CUP) / Horizontal Vertical Position (HVP)
            'H' | 'f' => {
                let row = params_vec.first().copied().unwrap_or(1).max(1) as usize - 1;
                let col = params_vec.get(1).copied().unwrap_or(1).max(1) as usize - 1;
                self.emu.cursor_row = row.min(self.emu.rows - 1);
                self.emu.cursor_col = col.min(self.emu.cols - 1);
            }
            // Erase in Display (ED)
            'J' => {
                let mode = params_vec.first().copied().unwrap_or(0);
                match mode {
                    0 => {
                        // Clear from cursor to end of screen
                        let blank = self.blank_cell();
                        for c in self.emu.cursor_col..self.emu.cols {
                            self.emu.grid[self.emu.cursor_row][c] = blank.clone();
                        }
                        for r in (self.emu.cursor_row + 1)..self.emu.rows {
                            for c in 0..self.emu.cols {
                                self.emu.grid[r][c] = blank.clone();
                            }
                        }
                    }
                    1 => {
                        // Clear from start to cursor
                        let blank = self.blank_cell();
                        for r in 0..self.emu.cursor_row {
                            for c in 0..self.emu.cols {
                                self.emu.grid[r][c] = blank.clone();
                            }
                        }
                        for c in 0..=self.emu.cursor_col {
                            if c < self.emu.cols {
                                self.emu.grid[self.emu.cursor_row][c] = blank.clone();
                            }
                        }
                    }
                    2 | 3 => {
                        // Clear entire screen
                        let blank = self.blank_cell();
                        for r in 0..self.emu.rows {
                            for c in 0..self.emu.cols {
                                self.emu.grid[r][c] = blank.clone();
                            }
                        }
                    }
                    _ => {}
                }
            }
            // Erase in Line (EL)
            'K' => {
                let mode = params_vec.first().copied().unwrap_or(0);
                let blank = self.blank_cell();
                match mode {
                    0 => {
                        for c in self.emu.cursor_col..self.emu.cols {
                            self.emu.grid[self.emu.cursor_row][c] = blank.clone();
                        }
                    }
                    1 => {
                        for c in 0..=self.emu.cursor_col {
                            if c < self.emu.cols {
                                self.emu.grid[self.emu.cursor_row][c] = blank.clone();
                            }
                        }
                    }
                    2 => {
                        for c in 0..self.emu.cols {
                            self.emu.grid[self.emu.cursor_row][c] = blank.clone();
                        }
                    }
                    _ => {}
                }
            }
            // SGR (Select Graphic Rendition)
            'm' => {
                self.handle_sgr(&params_vec);
            }
            // Cursor Next Line (CNL)
            'E' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                self.emu.cursor_row = (self.emu.cursor_row + n).min(self.emu.rows - 1);
                self.emu.cursor_col = 0;
            }
            // Cursor Previous Line (CPL)
            'F' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                self.emu.cursor_row = self.emu.cursor_row.saturating_sub(n);
                self.emu.cursor_col = 0;
            }
            // Cursor Horizontal Absolute (CHA)
            'G' => {
                let col = params_vec.first().copied().unwrap_or(1).max(1) as usize - 1;
                self.emu.cursor_col = col.min(self.emu.cols - 1);
            }
            // Scroll Up (SU)
            'S' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                self.scroll_up_region(n);
            }
            // Scroll Down (SD)
            'T' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                self.scroll_down_region(n);
            }
            // Set Top and Bottom Margins (DECSTBM)
            'r' => {
                if intermediates.is_empty() && self.emu.rows > 0 {
                    let rows = self.emu.rows;
                    let top = params_vec.first().copied().unwrap_or(1);
                    let bottom = params_vec.get(1).copied().unwrap_or(rows as u16);
                    let top = if top == 0 { 1 } else { top as usize };
                    let bottom = if bottom == 0 { rows } else { bottom as usize };
                    let top = top.clamp(1, rows);
                    let bottom = bottom.clamp(1, rows);
                    if top < bottom {
                        self.emu.scroll_top = top - 1;
                        self.emu.scroll_bottom = bottom - 1;
                        // DECSTBM homes the cursor.
                        self.emu.cursor_row = 0;
                        self.emu.cursor_col = 0;
                    }
                }
            }
            // Delete characters (DCH)
            'P' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                let row = self.emu.cursor_row;
                let col = self.emu.cursor_col;
                if row < self.emu.rows {
                    let blank = self.blank_cell();
                    for i in col..self.emu.cols {
                        if i + n < self.emu.cols {
                            self.emu.grid[row][i] = self.emu.grid[row][i + n].clone();
                        } else {
                            self.emu.grid[row][i] = blank.clone();
                        }
                    }
                }
            }
            // Insert characters (ICH)
            '@' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                let row = self.emu.cursor_row;
                let col = self.emu.cursor_col;
                if row < self.emu.rows {
                    let blank = self.blank_cell();
                    for i in (col..self.emu.cols).rev() {
                        if i >= col + n {
                            self.emu.grid[row][i] = self.emu.grid[row][i - n].clone();
                        } else {
                            self.emu.grid[row][i] = blank.clone();
                        }
                    }
                }
            }
            // Insert Lines (IL)
            'L' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                let row = self.emu.cursor_row;
                for _ in 0..n {
                    if row < self.emu.rows {
                        self.emu.grid.pop(); // remove last line
                        self.emu
                            .grid
                            .insert(row, vec![Cell::default(); self.emu.cols]);
                    }
                }
            }
            // Delete Lines (DL)
            'M' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                let row = self.emu.cursor_row;
                for _ in 0..n {
                    if row < self.emu.rows && self.emu.grid.len() > row {
                        self.emu.grid.remove(row);
                        self.emu.grid.push(vec![Cell::default(); self.emu.cols]);
                    }
                }
            }
            // Device Status Report (DSR) — queue a reply for the child.
            'n' => {
                if intermediates.is_empty() {
                    match params_vec.first().copied().unwrap_or(0) {
                        5 => self.emu.replies.extend_from_slice(b"\x1b[0n"),
                        6 => {
                            let row = self.emu.cursor_row + 1;
                            let col = self.emu.cursor_col + 1;
                            let reply = format!("\x1b[{row};{col}R");
                            self.emu.replies.extend_from_slice(reply.as_bytes());
                        }
                        _ => {}
                    }
                }
            }
            // Device Attributes (DA) — queue a VT100-with-AVO reply.
            'c' => match intermediates.first() {
                None => self.emu.replies.extend_from_slice(b"\x1b[?1;2c"),
                Some(&b'>') => self.emu.replies.extend_from_slice(b"\x1b[>0;0;0c"),
                _ => {}
            },
            // DECSCUSR — cursor shape (`CSI Ps SP q`).
            'q' => {
                if intermediates.first() == Some(&b' ') {
                    let ps = params_vec.first().copied().unwrap_or(0);
                    self.emu.cursor_shape = match ps {
                        3 | 4 => CursorShape::Underline,
                        5 | 6 => CursorShape::Bar,
                        _ => CursorShape::Block,
                    };
                }
            }
            // Set Mode / Reset Mode (DEC private: cursor visibility, alt screen).
            'h' => {
                if private {
                    for mode in params_vec {
                        self.set_private_mode(mode, true);
                    }
                }
            }
            'l' => {
                if private {
                    for mode in params_vec {
                        self.set_private_mode(mode, false);
                    }
                }
            }
            // Save/Restore cursor (DECSC/DECRC via CSI)
            's' => {
                self.emu.saved_cursor = Some((self.emu.cursor_row, self.emu.cursor_col));
            }
            'u' => {
                if let Some((r, c)) = self.emu.saved_cursor {
                    self.emu.cursor_row = r.min(self.emu.rows - 1);
                    self.emu.cursor_col = c.min(self.emu.cols - 1);
                }
            }
            // Erase Characters (ECH)
            'X' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                let blank = self.blank_cell();
                for i in 0..n {
                    let c = self.emu.cursor_col + i;
                    if c < self.emu.cols && self.emu.cursor_row < self.emu.rows {
                        self.emu.grid[self.emu.cursor_row][c] = blank.clone();
                    }
                }
            }
            _ => {
                // Unknown CSI sequence, ignore
            }
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            // Save cursor (DECSC)
            b'7' => {
                self.emu.saved_cursor = Some((self.emu.cursor_row, self.emu.cursor_col));
            }
            // Restore cursor (DECRC)
            b'8' => {
                if let Some((r, c)) = self.emu.saved_cursor {
                    self.emu.cursor_row = r.min(self.emu.rows - 1);
                    self.emu.cursor_col = c.min(self.emu.cols - 1);
                }
            }
            // Reset (RIS)
            b'c' => {
                self.full_reset();
            }
            // Index (IND) - move cursor down, scroll if needed
            b'D' => {
                self.index_down();
            }
            // Reverse index (RI) - move cursor up, scroll if needed
            b'M' => {
                self.reverse_index();
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {
        // OSC sequences (terminal title, etc.) — store but ignore for now
    }

    fn hook(&mut self, _params: &vte::Params, _intermediates: &[u8], _ignore: bool, _action: char) {
        // DCS sequences — ignore
    }

    fn unhook(&mut self) {}
    fn put(&mut self, _byte: u8) {}
}

impl<'a> Performer<'a> {
    /// Handle SGR (Select Graphic Rendition) parameters.
    fn handle_sgr(&mut self, params: &[u16]) {
        if params.is_empty() {
            // Reset
            self.emu.current_fg = Color::Reset;
            self.emu.current_bg = Color::Reset;
            self.emu.current_modifiers = Modifier::empty();
            return;
        }

        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => {
                    self.emu.current_fg = Color::Reset;
                    self.emu.current_bg = Color::Reset;
                    self.emu.current_modifiers = Modifier::empty();
                }
                1 => self.emu.current_modifiers |= Modifier::BOLD,
                2 => self.emu.current_modifiers |= Modifier::DIM,
                3 => self.emu.current_modifiers |= Modifier::ITALIC,
                4 => self.emu.current_modifiers |= Modifier::UNDERLINED,
                5 => self.emu.current_modifiers |= Modifier::SLOW_BLINK,
                7 => self.emu.current_modifiers |= Modifier::REVERSED,
                8 => self.emu.current_modifiers |= Modifier::HIDDEN,
                9 => self.emu.current_modifiers |= Modifier::CROSSED_OUT,
                // Reset attributes
                21 | 22 => {
                    self.emu.current_modifiers -= Modifier::BOLD;
                    self.emu.current_modifiers -= Modifier::DIM;
                }
                23 => self.emu.current_modifiers -= Modifier::ITALIC,
                24 => self.emu.current_modifiers -= Modifier::UNDERLINED,
                25 => self.emu.current_modifiers -= Modifier::SLOW_BLINK,
                27 => self.emu.current_modifiers -= Modifier::REVERSED,
                28 => self.emu.current_modifiers -= Modifier::HIDDEN,
                29 => self.emu.current_modifiers -= Modifier::CROSSED_OUT,
                // Standard foreground colors (30-37)
                30 => self.emu.current_fg = Color::Black,
                31 => self.emu.current_fg = Color::Red,
                32 => self.emu.current_fg = Color::Green,
                33 => self.emu.current_fg = Color::Yellow,
                34 => self.emu.current_fg = Color::Blue,
                35 => self.emu.current_fg = Color::Magenta,
                36 => self.emu.current_fg = Color::Cyan,
                37 => self.emu.current_fg = Color::White,
                // Extended foreground: 38;5;N (256-color) or 38;2;R;G;B (truecolor)
                38 => {
                    if i + 2 < params.len() && params[i + 1] == 5 {
                        // 256-color
                        self.emu.current_fg = Color::Indexed(params[i + 2] as u8);
                        i += 2;
                    } else if i + 4 < params.len() && params[i + 1] == 2 {
                        // Truecolor
                        self.emu.current_fg = Color::Rgb(
                            params[i + 2] as u8,
                            params[i + 3] as u8,
                            params[i + 4] as u8,
                        );
                        i += 4;
                    }
                }
                39 => self.emu.current_fg = Color::Reset,
                // Standard background colors (40-47)
                40 => self.emu.current_bg = Color::Black,
                41 => self.emu.current_bg = Color::Red,
                42 => self.emu.current_bg = Color::Green,
                43 => self.emu.current_bg = Color::Yellow,
                44 => self.emu.current_bg = Color::Blue,
                45 => self.emu.current_bg = Color::Magenta,
                46 => self.emu.current_bg = Color::Cyan,
                47 => self.emu.current_bg = Color::White,
                // Extended background: 48;5;N (256-color) or 48;2;R;G;B (truecolor)
                48 => {
                    if i + 2 < params.len() && params[i + 1] == 5 {
                        self.emu.current_bg = Color::Indexed(params[i + 2] as u8);
                        i += 2;
                    } else if i + 4 < params.len() && params[i + 1] == 2 {
                        self.emu.current_bg = Color::Rgb(
                            params[i + 2] as u8,
                            params[i + 3] as u8,
                            params[i + 4] as u8,
                        );
                        i += 4;
                    }
                }
                49 => self.emu.current_bg = Color::Reset,
                // Bright foreground colors (90-97)
                90 => self.emu.current_fg = Color::DarkGray,
                91 => self.emu.current_fg = Color::LightRed,
                92 => self.emu.current_fg = Color::LightGreen,
                93 => self.emu.current_fg = Color::LightYellow,
                94 => self.emu.current_fg = Color::LightBlue,
                95 => self.emu.current_fg = Color::LightMagenta,
                96 => self.emu.current_fg = Color::LightCyan,
                97 => self.emu.current_fg = Color::Gray,
                // Bright background colors (100-107)
                100 => self.emu.current_bg = Color::DarkGray,
                101 => self.emu.current_bg = Color::LightRed,
                102 => self.emu.current_bg = Color::LightGreen,
                103 => self.emu.current_bg = Color::LightYellow,
                104 => self.emu.current_bg = Color::LightBlue,
                105 => self.emu.current_bg = Color::LightMagenta,
                106 => self.emu.current_bg = Color::LightCyan,
                107 => self.emu.current_bg = Color::Gray,
                _ => {}
            }
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_emulator() {
        let emu = TerminalEmulator::new(24, 80);
        assert_eq!(emu.visible_rows(), 24);
        assert_eq!(emu.visible_cols(), 80);
        assert_eq!(emu.cursor_position(), (0, 0));
    }

    #[test]
    fn test_print_characters() {
        let mut emu = TerminalEmulator::new(24, 80);
        emu.process(b"Hello");
        assert_eq!(emu.cursor_position(), (0, 5));
        // Check that "Hello" was written
        assert_eq!(emu.grid[0][0].ch, 'H');
        assert_eq!(emu.grid[0][1].ch, 'e');
        assert_eq!(emu.grid[0][2].ch, 'l');
        assert_eq!(emu.grid[0][3].ch, 'l');
        assert_eq!(emu.grid[0][4].ch, 'o');
    }

    #[test]
    fn test_newline_and_carriage_return() {
        let mut emu = TerminalEmulator::new(24, 80);
        emu.process(b"Line1\r\nLine2");
        assert_eq!(emu.grid[0][0].ch, 'L');
        assert_eq!(emu.grid[0][4].ch, '1');
        assert_eq!(emu.grid[1][0].ch, 'L');
        assert_eq!(emu.grid[1][4].ch, '2');
    }

    #[test]
    fn test_cursor_movement() {
        let mut emu = TerminalEmulator::new(24, 80);
        // Move cursor to row 5, col 10 (1-based: 6, 11)
        emu.process(b"\x1b[6;11H");
        assert_eq!(emu.cursor_position(), (5, 10));
    }

    #[test]
    fn test_erase_line() {
        let mut emu = TerminalEmulator::new(24, 80);
        emu.process(b"Hello World");
        emu.process(b"\r"); // Move to start
        emu.process(b"\x1b[2K"); // Erase entire line
        for c in 0..11 {
            assert_eq!(emu.grid[0][c].ch, ' ');
        }
    }

    #[test]
    fn test_sgr_colors() {
        let mut emu = TerminalEmulator::new(24, 80);
        // Set red foreground and print
        emu.process(b"\x1b[31mR\x1b[0m");
        assert_eq!(emu.grid[0][0].ch, 'R');
        assert_eq!(emu.grid[0][0].fg, Color::Red);
    }

    #[test]
    fn test_scrollback() {
        let mut emu = TerminalEmulator::new(3, 10);
        // Print 5 lines in a 3-row terminal → first 2 go to scrollback
        emu.process(b"Line1\r\nLine2\r\nLine3\r\nLine4\r\nLine5");
        assert_eq!(emu.scrollback.len(), 2);
        assert_eq!(emu.scrollback[0][0].ch, 'L');
        assert_eq!(emu.scrollback[0][4].ch, '1');
    }

    #[test]
    fn test_resize() {
        let mut emu = TerminalEmulator::new(24, 80);
        emu.process(b"Hello");
        emu.resize(10, 40);
        assert_eq!(emu.visible_rows(), 10);
        assert_eq!(emu.visible_cols(), 40);
        // Content should be preserved
        assert_eq!(emu.grid[0][0].ch, 'H');
    }

    #[test]
    fn test_render_lines() {
        let mut emu = TerminalEmulator::new(3, 5);
        emu.process(b"Hi");
        let lines = emu.render_lines();
        assert_eq!(lines.len(), 3);
        // First line should have H and i
        assert_eq!(lines[0].spans.len(), 5);
    }

    #[test]
    fn test_tab() {
        let mut emu = TerminalEmulator::new(24, 80);
        emu.process(b"\tX");
        // Tab should move to column 8
        assert_eq!(emu.grid[0][8].ch, 'X');
    }

    #[test]
    fn test_backspace() {
        let mut emu = TerminalEmulator::new(24, 80);
        emu.process(b"AB\x08C");
        // Backspace moves cursor back, then C overwrites B
        assert_eq!(emu.grid[0][0].ch, 'A');
        assert_eq!(emu.grid[0][1].ch, 'C');
    }

    #[test]
    fn test_erase_display_clear_to_end() {
        let mut emu = TerminalEmulator::new(3, 10);
        emu.process(b"AAAAAAAAAA\r\nBBBBBBBBBB\r\nCCCCCCCCCC");
        // Move to row 1, col 5 and clear to end
        emu.process(b"\x1b[2;6H\x1b[0J");
        // Row 0 should be intact
        assert_eq!(emu.grid[0][0].ch, 'A');
        // Row 1, cols 0-4 should be intact, 5-9 should be blank
        assert_eq!(emu.grid[1][4].ch, 'B');
        assert_eq!(emu.grid[1][5].ch, ' ');
        // Row 2 should be all blank
        assert_eq!(emu.grid[2][0].ch, ' ');
    }

    #[test]
    fn test_256_color() {
        let mut emu = TerminalEmulator::new(24, 80);
        // Set 256-color foreground: ESC[38;5;196m (bright red)
        emu.process(b"\x1b[38;5;196mX");
        assert_eq!(emu.grid[0][0].fg, Color::Indexed(196));
    }

    #[test]
    fn test_truecolor() {
        let mut emu = TerminalEmulator::new(24, 80);
        // Set RGB foreground: ESC[38;2;255;128;0m
        emu.process(b"\x1b[38;2;255;128;0mX");
        assert_eq!(emu.grid[0][0].fg, Color::Rgb(255, 128, 0));
    }

    #[test]
    fn test_scrollback_len() {
        let mut emu = TerminalEmulator::new(3, 10);
        assert_eq!(emu.scrollback_len(), 0);
        // Fill and overflow: 5 lines in 3-row terminal
        emu.process(b"L1\r\nL2\r\nL3\r\nL4\r\nL5");
        assert_eq!(emu.scrollback_len(), 2);
    }

    #[test]
    fn test_cell_at_grid() {
        let mut emu = TerminalEmulator::new(3, 10);
        emu.process(b"Hello");
        // scrollback_len=0, so line 0 = grid row 0
        assert_eq!(emu.cell_at(0, 0).unwrap().ch, 'H');
        assert_eq!(emu.cell_at(0, 4).unwrap().ch, 'o');
        // Out of bounds
        assert!(emu.cell_at(0, 100).is_none());
        assert!(emu.cell_at(100, 0).is_none());
    }

    #[test]
    fn test_cell_at_scrollback() {
        let mut emu = TerminalEmulator::new(3, 10);
        emu.process(b"L1\r\nL2\r\nL3\r\nL4\r\nL5");
        // scrollback has L1, L2
        assert_eq!(emu.cell_at(0, 0).unwrap().ch, 'L');
        assert_eq!(emu.cell_at(0, 1).unwrap().ch, '1');
        assert_eq!(emu.cell_at(1, 1).unwrap().ch, '2');
        // grid starts at abs line 2
        assert_eq!(emu.cell_at(2, 1).unwrap().ch, '3');
    }

    #[test]
    fn test_extract_text_single_cell() {
        let mut emu = TerminalEmulator::new(3, 10);
        emu.process(b"Hello");
        let text = emu.extract_text(0, 0, 0, 0).unwrap();
        assert_eq!(text, "H");
    }

    #[test]
    fn test_extract_text_single_line() {
        let mut emu = TerminalEmulator::new(3, 10);
        emu.process(b"Hello");
        let text = emu.extract_text(0, 0, 0, 4).unwrap();
        assert_eq!(text, "Hello");
    }

    #[test]
    fn test_extract_text_multi_line() {
        let mut emu = TerminalEmulator::new(5, 10);
        emu.process(b"AAA\r\nBBB\r\nCCC");
        let text = emu.extract_text(0, 0, 2, 2).unwrap();
        assert_eq!(text, "AAA\nBBB\nCCC");
    }

    #[test]
    fn test_extract_text_with_scrollback() {
        let mut emu = TerminalEmulator::new(3, 10);
        emu.process(b"L1\r\nL2\r\nL3\r\nL4\r\nL5");
        // L1, L2 in scrollback; L3, L4, L5 in grid
        // Extract from scrollback line 0 to grid line 0 (abs 2)
        let text = emu.extract_text(0, 0, 2, 1).unwrap();
        assert!(text.contains("L1"));
        assert!(text.contains("L2"));
        assert!(text.contains("L3"));
    }

    #[test]
    fn test_extract_text_invalid_range() {
        let emu = TerminalEmulator::new(3, 10);
        // start > end
        assert!(emu.extract_text(5, 0, 2, 0).is_none());
        // start out of bounds
        assert!(emu.extract_text(100, 0, 200, 0).is_none());
    }

    #[test]
    fn test_extract_text_trims_trailing_spaces() {
        let mut emu = TerminalEmulator::new(3, 10);
        emu.process(b"Hi");
        // Grid row has "Hi        " (padded to cols=10)
        let text = emu.extract_text(0, 0, 0, 9).unwrap();
        assert_eq!(text, "Hi");
    }

    // ---- Phase 9 Task 1 fixtures -------------------------------------------
    // Each fixture below is paired with a local survival mechanism in the
    // production code above; disabling that mechanism fails the fixture (see
    // the task report for the recorded neuter-to-red evidence).

    #[test]
    fn fixture_alternate_screen_enter_and_leave() {
        let mut emu = TerminalEmulator::new(6, 20);
        emu.process(b"PRIMARY");
        assert_eq!(emu.grid[0][0].ch, 'P');
        assert!(!emu.alternate_screen());
        emu.process(b"\x1b[?1049h");
        assert!(emu.alternate_screen());
        emu.process(b"\x1b[1;1HALT");
        assert_eq!(emu.grid[0][0].ch, 'A');
        assert_eq!(emu.grid[0][1].ch, 'L');
        assert_eq!(emu.grid[0][2].ch, 'T');
        emu.process(b"\x1b[?1049l");
        assert!(!emu.alternate_screen());
        assert_eq!(emu.grid[0][0].ch, 'P');
        assert_eq!(emu.grid[0][6].ch, 'Y');
    }

    #[test]
    fn fixture_cursor_visibility_and_shape_modes() {
        let mut emu = TerminalEmulator::new(6, 20);
        assert!(emu.cursor_visible());
        emu.process(b"\x1b[?25l");
        assert!(!emu.cursor_visible());
        emu.process(b"\x1b[?25h");
        assert!(emu.cursor_visible());
        assert_eq!(emu.cursor_shape(), CursorShape::Block);
        emu.process(b"\x1b[4 q");
        assert_eq!(emu.cursor_shape(), CursorShape::Underline);
        emu.process(b"\x1b[6 q");
        assert_eq!(emu.cursor_shape(), CursorShape::Bar);
        emu.process(b"\x1b[2 q");
        assert_eq!(emu.cursor_shape(), CursorShape::Block);
    }

    #[test]
    fn fixture_dsr_and_da_replies_are_ordered() {
        let mut emu = TerminalEmulator::new(6, 20);
        emu.process(b"AB");
        emu.process(b"\x1b[6n");
        emu.process(b"\x1b[c");
        assert_eq!(emu.take_replies(), b"\x1b[1;3R\x1b[?1;2c".to_vec());
        assert!(emu.take_replies().is_empty());
        emu.process(b"\x1b[5n");
        assert_eq!(emu.take_replies(), b"\x1b[0n".to_vec());
    }

    #[test]
    fn fixture_scroll_region_confines_line_feed() {
        let mut emu = TerminalEmulator::new(6, 6);
        emu.process(b"\x1b[1;1HAAA");
        emu.process(b"\x1b[2;1HBBB");
        emu.process(b"\x1b[3;1HCCC");
        emu.process(b"\x1b[4;1HDDD");
        emu.process(b"\x1b[2;3r");
        assert_eq!(emu.scroll_region(), (1, 2));
        emu.process(b"\x1b[3;1H");
        emu.process(b"\n");
        assert_eq!(emu.grid[0][0].ch, 'A'); // above region untouched
        assert_eq!(emu.grid[1][0].ch, 'C'); // region scrolled up
        assert_eq!(emu.grid[1][2].ch, 'C');
        assert_eq!(emu.grid[2][0].ch, ' '); // region bottom blanked
        assert_eq!(emu.grid[3][0].ch, 'D'); // below region untouched
        assert_eq!(emu.cursor_position(), (2, 0));
    }

    #[test]
    fn fixture_wide_characters_occupy_two_cells() {
        let mut emu = TerminalEmulator::new(4, 20);
        emu.process("中文".as_bytes());
        assert_eq!(emu.grid[0][0].ch, '中');
        assert!(emu.grid[0][0].wide);
        assert!(!emu.grid[0][0].continuation);
        assert!(emu.grid[0][1].continuation);
        assert_eq!(emu.grid[0][2].ch, '文');
        assert!(emu.grid[0][2].wide);
        assert!(emu.grid[0][3].continuation);
        assert_eq!(emu.cursor_position(), (0, 4));
        emu.process(b"A");
        assert_eq!(emu.grid[0][4].ch, 'A');
        assert!(!emu.grid[0][4].wide);
    }

    #[test]
    fn fixture_combining_mark_attaches_to_base_cell() {
        let mut emu = TerminalEmulator::new(4, 20);
        emu.process("e\u{0301}".as_bytes());
        assert_eq!(emu.grid[0][0].ch, 'e');
        assert_eq!(emu.grid[0][0].combining, "\u{0301}");
        assert_eq!(emu.cursor_position(), (0, 1));
        assert_eq!(emu.grid[0][1].ch, ' ');
        let rendered = emu.render_lines()[0].spans[0].content.to_string();
        assert_eq!(rendered, "e\u{0301}");
        // The grapheme is one column of text, not two.
        assert_eq!(emu.extract_text(0, 0, 0, 1).unwrap(), "e\u{0301}");
    }

    #[test]
    fn fixture_resize_grow_and_shrink_preserve_content() {
        let mut emu = TerminalEmulator::new(3, 5);
        emu.process(b"HELLO\r\nWORLD");
        emu.resize(8, 24);
        assert_eq!((emu.visible_rows(), emu.visible_cols()), (8, 24));
        assert_eq!(emu.grid[0][0].ch, 'H');
        assert_eq!(emu.grid[1][0].ch, 'W');
        emu.resize(3, 8);
        assert_eq!((emu.visible_rows(), emu.visible_cols()), (3, 8));
        let (r, c) = emu.cursor_position();
        assert!(r < 3 && c < 8, "cursor {r},{c} must be clamped");
        assert_eq!(emu.grid[0][0].ch, 'H');
        assert_eq!(emu.scroll_region(), (0, 2));
    }

    #[test]
    fn fixture_resize_landing_mid_escape_sequence_keeps_parser_state() {
        let mut emu = TerminalEmulator::new(4, 10);
        emu.process(b"\x1b[1"); // partial CSI
        emu.resize(8, 20); // resize lands inside the sequence
        emu.process(b"HZ"); // completes CSI 1 H (CUP row 1 col 1) then prints Z
        assert_eq!(emu.grid[0][0].ch, 'Z');
        assert_eq!(emu.grid[0][1].ch, ' ');
        assert_eq!(emu.cursor_position(), (0, 1));
    }

    // ---- Phase 9 Task 1 coverage of the adapted emulator surface -----------
    // These exercise the changed production paths directly (cursor movement,
    // erase, insert/delete, SGR matrix, save/restore, reset, scroll-down,
    // degenerate geometry) so the adapted code is not landmined by untested
    // branches.

    #[test]
    fn coverage_empty_input_is_ignored() {
        let mut emu = TerminalEmulator::new(2, 4);
        emu.process(b"");
        assert_eq!(emu.cursor_position(), (0, 0));
        assert!(emu.take_replies().is_empty());
    }

    #[test]
    fn coverage_degenerate_geometry_is_safe() {
        let mut emu = TerminalEmulator::new(0, 0);
        emu.process(b"abc");
        emu.process(b"\x1b[S\x1b[T\x1b[M\x1b[L\n\x1bD\x1bM");
        emu.process(b"\x1b[6n\x1b[c\x1b[?25l\x1b[4 q\x1b[?1049h\x1b[?1049l");
        emu.process(b"\x1bc");
        emu.resize(0, 0);
        assert_eq!((emu.visible_rows(), emu.visible_cols()), (0, 0));
        let mut narrow = TerminalEmulator::new(1, 0);
        narrow.process(b"x");
        assert_eq!(narrow.visible_cols(), 0);
    }

    #[test]
    fn coverage_sgr_full_matrix() {
        let mut emu = TerminalEmulator::new(2, 4);
        emu.process(b"\x1b[m"); // empty params resets
        let codes = [
            "0",
            "1",
            "2",
            "3",
            "4",
            "5",
            "7",
            "8",
            "9",
            "21",
            "22",
            "23",
            "24",
            "25",
            "27",
            "28",
            "29",
            "30",
            "31",
            "32",
            "33",
            "34",
            "35",
            "36",
            "37",
            "39",
            "40",
            "41",
            "42",
            "43",
            "44",
            "45",
            "46",
            "47",
            "49",
            "90",
            "91",
            "92",
            "93",
            "94",
            "95",
            "96",
            "97",
            "100",
            "101",
            "102",
            "103",
            "104",
            "105",
            "106",
            "107",
            "38;5;196",
            "38;2;1;2;3",
            "48;5;20",
            "48;2;4;5;6",
        ];
        for code in codes {
            emu.process(format!("\x1b[{code}m").as_bytes());
            emu.process(b"\rX");
            assert_eq!(emu.grid[0][0].ch, 'X', "SGR {code}");
        }
        assert_eq!(emu.grid[0][0].bg, Color::Rgb(4, 5, 6));
    }

    #[test]
    fn coverage_erase_display_and_line_modes() {
        let mut emu = TerminalEmulator::new(4, 6);
        for seq in [
            b"\x1b[0J".as_slice(),
            b"\x1b[1J",
            b"\x1b[2J",
            b"\x1b[3J",
            b"\x1b[0K",
            b"\x1b[1K",
            b"\x1b[2K",
        ] {
            emu.process(b"abcdef\x1b[1;3H");
            emu.process(seq);
        }
        assert_eq!(emu.visible_rows(), 4);
    }

    #[test]
    fn coverage_cursor_movement_sequences() {
        let mut emu = TerminalEmulator::new(5, 8);
        emu.process(b"\x1b[3;4H"); // CUP
        emu.process(b"\x1b[2A"); // CUU
        emu.process(b"\x1b[1B"); // CUD
        emu.process(b"\x1b[2C"); // CUF
        emu.process(b"\x1b[1D"); // CUB
        emu.process(b"\x1b[2E"); // CNL
        emu.process(b"\x1b[1F"); // CPL
        emu.process(b"\x1b[5G"); // CHA
        assert_eq!(emu.cursor_position(), (2, 4));
        emu.process(b"\x1b[2;2f"); // HVP
        assert_eq!(emu.cursor_position(), (1, 1));
    }

    #[test]
    fn coverage_insert_delete_lines_chars_and_erase() {
        let mut emu = TerminalEmulator::new(4, 8);
        emu.process(b"\x1b[1;1Habcdef");
        emu.process(b"\x1b[1;2H\x1b[2P"); // DCH
        emu.process(b"\x1b[1;2H\x1b[2@"); // ICH
        emu.process(b"\x1b[1;2H\x1b[3X"); // ECH
        emu.process(b"\x1b[2;1H\x1b[1L"); // IL
        emu.process(b"\x1b[2;1H\x1b[1M"); // DL
        assert_eq!(emu.visible_rows(), 4);
    }

    #[test]
    fn coverage_scroll_down_reverse_index_and_margins() {
        let mut emu = TerminalEmulator::new(5, 6);
        emu.process(b"\x1b[2;4r"); // region rows 2..4
        emu.process(b"\x1b[2;1H\x1b[1T"); // SD inside the region
        emu.process(b"\x1b[2;1H\x1bM"); // RI at the top margin
        emu.process(b"\x1b[5;1H\x1bM"); // RI below the margin
        emu.process(b"\x1b[1;1H\x1b[2S"); // SU
        emu.process(b"\x1bD"); // IND
        assert_eq!(emu.scroll_region(), (1, 3));
    }

    #[test]
    fn coverage_save_restore_cursor_and_reset() {
        let mut emu = TerminalEmulator::new(4, 8);
        emu.process(b"\x1b[?1049l"); // leave alt when not in alt
        emu.process(b"\x1b[2;3H\x1b[s\x1b[1;1H\x1b[u"); // CSI s/u
        assert_eq!(emu.cursor_position(), (1, 2));
        emu.process(b"\x1b[3;4H\x1b7\x1b[1;1H\x1b8"); // ESC 7/8
        assert_eq!(emu.cursor_position(), (2, 3));
        emu.process(b"\x1b[?25l\x1b[4 qPRIMARY");
        emu.process(b"\x1bc"); // RIS
        assert!(emu.cursor_visible());
        assert_eq!(emu.cursor_shape(), CursorShape::Block);
        assert_eq!(emu.cursor_position(), (0, 0));
        assert_eq!(emu.grid[0][0].ch, ' ');
        assert_eq!(emu.scroll_region(), (0, 3));
        assert!(emu.take_replies().is_empty());
    }

    #[test]
    fn coverage_alternate_screen_reentry_and_resize() {
        let mut emu = TerminalEmulator::new(4, 8);
        emu.process(b"\x1b[?1049h");
        emu.process(b"\x1b[?1049h"); // re-enter clears and homes
        emu.process(b"\x1b[2;2HA");
        emu.resize(6, 12); // resize while alternate
        assert!(emu.alternate_screen());
        assert_eq!((emu.visible_rows(), emu.visible_cols()), (6, 12));
        emu.process(b"\x1b[?1049l");
        assert!(!emu.alternate_screen());
        assert_eq!(emu.grid.len(), 6);
    }

    #[test]
    fn coverage_render_wrappers_and_wide_extraction() {
        let mut emu = TerminalEmulator::new(3, 5);
        emu.process(b"one\r\ntwo\r\nthree\r\nfour");
        let lines = emu.render_lines_at_offset(1);
        assert_eq!(lines.len(), 3);
        assert!(!emu.scrollback_lines().is_empty());
        // A wide glyph renders its head once and leaves the continuation empty.
        let mut wide = TerminalEmulator::new(2, 4);
        wide.process("中".as_bytes());
        let rendered = wide.render_lines();
        assert_eq!(rendered[0].spans[0].content.to_string(), "中");
        assert_eq!(rendered[0].spans[1].content.to_string(), "");
        assert_eq!(wide.extract_text(0, 0, 0, 3).unwrap(), "中");
    }

    #[test]
    fn coverage_combining_edge_cases() {
        // A combining mark with no base cell is dropped, cursor unchanged.
        let mut emu = TerminalEmulator::new(2, 4);
        emu.process("\u{0301}".as_bytes());
        assert_eq!(emu.cursor_position(), (0, 0));
        // A combining mark after a wide glyph attaches to the base, not the tail.
        let mut wide = TerminalEmulator::new(2, 6);
        wide.process("中\u{0301}".as_bytes());
        assert_eq!(wide.grid[0][0].ch, '中');
        assert_eq!(wide.grid[0][0].combining, "\u{0301}");
        assert!(wide.grid[0][1].continuation);
        // On a one-column grid the wide glyph wraps and the mark clamps.
        let mut narrow = TerminalEmulator::new(2, 1);
        narrow.process("中\u{0301}".as_bytes());
        assert_eq!(narrow.grid[1][0].combining, "\u{0301}");
        // Line wrap when the cursor is at the right edge.
        let mut wrap = TerminalEmulator::new(3, 2);
        wrap.process(b"abcd");
        assert_eq!(wrap.cursor_position(), (1, 2));
    }

    #[test]
    fn coverage_private_modes_da_secondary_and_dsr_unknown() {
        let mut emu = TerminalEmulator::new(2, 4);
        emu.process(b"\x1b[?9999h\x1b[?9999l"); // unknown private modes
        emu.process(b"\x1b[>c"); // secondary DA
        emu.process(b"\x1b[9n"); // unknown DSR
        emu.process(b"\x1b[?c"); // DA with an unknown intermediate
        let replies = emu.take_replies();
        assert!(replies.windows(4).any(|w| w == b"\x1b[>0"));
        assert_eq!(replies.len(), b"\x1b[>0;0;0c".len());
    }
}
