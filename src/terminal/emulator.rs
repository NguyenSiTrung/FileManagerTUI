//! Terminal emulator: ANSI escape sequence parser + screen buffer.
//!
//! Uses the `vte` crate (from Alacritty) to parse ANSI sequences and
//! maintains a grid of cells that map to ratatui styled spans for rendering.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// A single character cell in the terminal grid.
#[derive(Debug, Clone)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub modifiers: Modifier,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Color::Reset,
            bg: Color::Reset,
            modifiers: Modifier::empty(),
        }
    }
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
    /// Cursor column (0-based).
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
        }
    }

    /// Process raw bytes from the PTY through the VTE parser.
    pub fn process(&mut self, data: &[u8]) {
        for &byte in data {
            // The VTE parser calls methods on Perform trait via advance()
            // We need to use a separate performer to avoid borrowing issues.
            let mut performer = Performer {
                grid: &mut self.grid,
                scrollback: &mut self.scrollback,
                max_scrollback: self.max_scrollback,
                cursor_row: &mut self.cursor_row,
                cursor_col: &mut self.cursor_col,
                rows: self.rows,
                cols: self.cols,
                current_fg: &mut self.current_fg,
                current_bg: &mut self.current_bg,
                current_modifiers: &mut self.current_modifiers,
                saved_cursor: &mut self.saved_cursor,
            };
            self.parser.advance(&mut performer, byte);
        }
    }

    /// Resize the emulator grid.
    pub fn resize(&mut self, new_rows: usize, new_cols: usize) {
        let mut new_grid = vec![vec![Cell::default(); new_cols]; new_rows];
        // Copy existing content that fits
        for (r, row) in self.grid.iter().enumerate() {
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
        self.grid = new_grid;
        self.rows = new_rows;
        self.cols = new_cols;
        // Clamp cursor
        self.cursor_row = self.cursor_row.min(new_rows.saturating_sub(1));
        self.cursor_col = self.cursor_col.min(new_cols.saturating_sub(1));
    }

    /// Render the visible grid as ratatui Lines (for the widget).
    pub fn render_lines(&self) -> Vec<Line<'static>> {
        self.grid
            .iter()
            .map(|row| {
                let spans: Vec<Span<'static>> = row
                    .iter()
                    .map(|cell| {
                        let style = Style::default()
                            .fg(cell.fg)
                            .bg(cell.bg)
                            .add_modifier(cell.modifiers);
                        Span::styled(cell.ch.to_string(), style)
                    })
                    .collect();
                Line::from(spans)
            })
            .collect()
    }

    /// Total lines including scrollback.
    #[allow(dead_code)]
    pub fn total_lines(&self) -> usize {
        self.scrollback.len() + self.rows
    }

    /// Get scrollback lines for rendering (oldest first).
    #[allow(dead_code)]
    pub fn scrollback_lines(&self) -> Vec<Line<'static>> {
        self.scrollback
            .iter()
            .map(|row| {
                let spans: Vec<Span<'static>> = row
                    .iter()
                    .map(|cell| {
                        let style = Style::default()
                            .fg(cell.fg)
                            .bg(cell.bg)
                            .add_modifier(cell.modifiers);
                        Span::styled(cell.ch.to_string(), style)
                    })
                    .collect();
                Line::from(spans)
            })
            .collect()
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
}

/// Internal performer struct that receives VTE callbacks.
/// Separated from TerminalEmulator to avoid borrow-checker issues with the parser.
struct Performer<'a> {
    grid: &'a mut Vec<Vec<Cell>>,
    scrollback: &'a mut Vec<Vec<Cell>>,
    max_scrollback: usize,
    cursor_row: &'a mut usize,
    cursor_col: &'a mut usize,
    rows: usize,
    cols: usize,
    current_fg: &'a mut Color,
    current_bg: &'a mut Color,
    current_modifiers: &'a mut Modifier,
    saved_cursor: &'a mut Option<(usize, usize)>,
}

impl<'a> Performer<'a> {
    /// Scroll the grid up by one line, moving the top line to scrollback.
    fn scroll_up(&mut self) {
        if !self.grid.is_empty() {
            let line = self.grid.remove(0);
            self.scrollback.push(line);
            // Trim scrollback
            if self.scrollback.len() > self.max_scrollback {
                self.scrollback.remove(0);
            }
            self.grid.push(vec![Cell::default(); self.cols]);
        }
    }

    fn current_cell(&self) -> Cell {
        Cell {
            ch: ' ',
            fg: *self.current_fg,
            bg: *self.current_bg,
            modifiers: *self.current_modifiers,
        }
    }
}

impl<'a> vte::Perform for Performer<'a> {
    /// Handle printable characters.
    fn print(&mut self, c: char) {
        if *self.cursor_col >= self.cols {
            // Line wrap
            *self.cursor_col = 0;
            *self.cursor_row += 1;
            if *self.cursor_row >= self.rows {
                self.scroll_up();
                *self.cursor_row = self.rows - 1;
            }
        }
        if *self.cursor_row < self.rows && *self.cursor_col < self.cols {
            self.grid[*self.cursor_row][*self.cursor_col] = Cell {
                ch: c,
                fg: *self.current_fg,
                bg: *self.current_bg,
                modifiers: *self.current_modifiers,
            };
        }
        *self.cursor_col += 1;
    }

    /// Handle control characters.
    fn execute(&mut self, byte: u8) {
        match byte {
            // Carriage Return
            b'\r' => {
                *self.cursor_col = 0;
            }
            // Line Feed / Newline
            b'\n' => {
                *self.cursor_row += 1;
                if *self.cursor_row >= self.rows {
                    self.scroll_up();
                    *self.cursor_row = self.rows - 1;
                }
            }
            // Backspace
            0x08 => {
                if *self.cursor_col > 0 {
                    *self.cursor_col -= 1;
                }
            }
            // Tab
            b'\t' => {
                let tab_stop = (*self.cursor_col + 8) & !7;
                *self.cursor_col = tab_stop.min(self.cols - 1);
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
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        let params_vec: Vec<u16> = params.iter().flat_map(|sub| sub.iter().copied()).collect();

        match action {
            // Cursor Up (CUU)
            'A' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                *self.cursor_row = self.cursor_row.saturating_sub(n);
            }
            // Cursor Down (CUD)
            'B' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                *self.cursor_row = (*self.cursor_row + n).min(self.rows - 1);
            }
            // Cursor Forward (CUF)
            'C' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                *self.cursor_col = (*self.cursor_col + n).min(self.cols - 1);
            }
            // Cursor Back (CUB)
            'D' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                *self.cursor_col = self.cursor_col.saturating_sub(n);
            }
            // Cursor Position (CUP) / Horizontal Vertical Position (HVP)
            'H' | 'f' => {
                let row = params_vec.first().copied().unwrap_or(1).max(1) as usize - 1;
                let col = params_vec.get(1).copied().unwrap_or(1).max(1) as usize - 1;
                *self.cursor_row = row.min(self.rows - 1);
                *self.cursor_col = col.min(self.cols - 1);
            }
            // Erase in Display (ED)
            'J' => {
                let mode = params_vec.first().copied().unwrap_or(0);
                match mode {
                    0 => {
                        // Clear from cursor to end of screen
                        let blank = self.current_cell();
                        for c in *self.cursor_col..self.cols {
                            self.grid[*self.cursor_row][c] = blank.clone();
                        }
                        for r in (*self.cursor_row + 1)..self.rows {
                            for c in 0..self.cols {
                                self.grid[r][c] = blank.clone();
                            }
                        }
                    }
                    1 => {
                        // Clear from start to cursor
                        let blank = self.current_cell();
                        for r in 0..*self.cursor_row {
                            for c in 0..self.cols {
                                self.grid[r][c] = blank.clone();
                            }
                        }
                        for c in 0..=*self.cursor_col {
                            if c < self.cols {
                                self.grid[*self.cursor_row][c] = blank.clone();
                            }
                        }
                    }
                    2 | 3 => {
                        // Clear entire screen
                        let blank = self.current_cell();
                        for r in 0..self.rows {
                            for c in 0..self.cols {
                                self.grid[r][c] = blank.clone();
                            }
                        }
                    }
                    _ => {}
                }
            }
            // Erase in Line (EL)
            'K' => {
                let mode = params_vec.first().copied().unwrap_or(0);
                let blank = self.current_cell();
                match mode {
                    0 => {
                        // Clear from cursor to end of line
                        for c in *self.cursor_col..self.cols {
                            self.grid[*self.cursor_row][c] = blank.clone();
                        }
                    }
                    1 => {
                        // Clear from start to cursor
                        for c in 0..=*self.cursor_col {
                            if c < self.cols {
                                self.grid[*self.cursor_row][c] = blank.clone();
                            }
                        }
                    }
                    2 => {
                        // Clear entire line
                        for c in 0..self.cols {
                            self.grid[*self.cursor_row][c] = blank.clone();
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
                *self.cursor_row = (*self.cursor_row + n).min(self.rows - 1);
                *self.cursor_col = 0;
            }
            // Cursor Previous Line (CPL)
            'F' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                *self.cursor_row = self.cursor_row.saturating_sub(n);
                *self.cursor_col = 0;
            }
            // Cursor Horizontal Absolute (CHA)
            'G' => {
                let col = params_vec.first().copied().unwrap_or(1).max(1) as usize - 1;
                *self.cursor_col = col.min(self.cols - 1);
            }
            // Scroll Up (SU)
            'S' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                for _ in 0..n {
                    self.scroll_up();
                }
            }
            // Delete characters (DCH)
            'P' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                let row = *self.cursor_row;
                let col = *self.cursor_col;
                if row < self.rows {
                    let blank = self.current_cell();
                    for i in col..self.cols {
                        if i + n < self.cols {
                            self.grid[row][i] = self.grid[row][i + n].clone();
                        } else {
                            self.grid[row][i] = blank.clone();
                        }
                    }
                }
            }
            // Insert characters (ICH)
            '@' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                let row = *self.cursor_row;
                let col = *self.cursor_col;
                if row < self.rows {
                    let blank = self.current_cell();
                    // Shift right
                    for i in (col..self.cols).rev() {
                        if i + n < self.cols {
                            // shift existing cell
                        }
                        if i >= col + n {
                            self.grid[row][i] = self.grid[row][i - n].clone();
                        } else {
                            self.grid[row][i] = blank.clone();
                        }
                    }
                }
            }
            // Insert Lines (IL)
            'L' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                let row = *self.cursor_row;
                for _ in 0..n {
                    if row < self.rows {
                        self.grid.pop(); // remove last line
                        self.grid
                            .insert(row, vec![Cell::default(); self.cols]);
                    }
                }
            }
            // Delete Lines (DL)
            'M' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                let row = *self.cursor_row;
                for _ in 0..n {
                    if row < self.rows && self.grid.len() > row {
                        self.grid.remove(row);
                        self.grid.push(vec![Cell::default(); self.cols]);
                    }
                }
            }
            // Device Status Report (DSR) — respond with cursor position
            'n' => {
                // We don't have a writer to respond, ignore
            }
            // Set Mode / Reset Mode (for cursor visibility, etc.)
            'h' | 'l' => {
                // Ignore mode changes for now (cursor visibility, etc.)
            }
            // Save/Restore cursor (DECSC/DECRC via CSI)
            's' => {
                *self.saved_cursor = Some((*self.cursor_row, *self.cursor_col));
            }
            'u' => {
                if let Some((r, c)) = *self.saved_cursor {
                    *self.cursor_row = r.min(self.rows - 1);
                    *self.cursor_col = c.min(self.cols - 1);
                }
            }
            // Erase Characters (ECH)
            'X' => {
                let n = params_vec.first().copied().unwrap_or(1).max(1) as usize;
                let blank = self.current_cell();
                for i in 0..n {
                    let c = *self.cursor_col + i;
                    if c < self.cols && *self.cursor_row < self.rows {
                        self.grid[*self.cursor_row][c] = blank.clone();
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
                *self.saved_cursor = Some((*self.cursor_row, *self.cursor_col));
            }
            // Restore cursor (DECRC)
            b'8' => {
                if let Some((r, c)) = *self.saved_cursor {
                    *self.cursor_row = r.min(self.rows - 1);
                    *self.cursor_col = c.min(self.cols - 1);
                }
            }
            // Reset (RIS)
            b'c' => {
                // Full reset
                *self.current_fg = Color::Reset;
                *self.current_bg = Color::Reset;
                *self.current_modifiers = Modifier::empty();
                *self.cursor_row = 0;
                *self.cursor_col = 0;
                let blank = Cell::default();
                for r in 0..self.rows {
                    for c in 0..self.cols {
                        self.grid[r][c] = blank.clone();
                    }
                }
            }
            // Index (IND) - move cursor down, scroll if needed
            b'D' => {
                *self.cursor_row += 1;
                if *self.cursor_row >= self.rows {
                    self.scroll_up();
                    *self.cursor_row = self.rows - 1;
                }
            }
            // Reverse index (RI) - move cursor up, scroll if needed
            b'M' => {
                if *self.cursor_row == 0 {
                    // Insert a blank line at top, push bottom out
                    self.grid.pop();
                    self.grid.insert(0, vec![Cell::default(); self.cols]);
                } else {
                    *self.cursor_row -= 1;
                }
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
            *self.current_fg = Color::Reset;
            *self.current_bg = Color::Reset;
            *self.current_modifiers = Modifier::empty();
            return;
        }

        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => {
                    *self.current_fg = Color::Reset;
                    *self.current_bg = Color::Reset;
                    *self.current_modifiers = Modifier::empty();
                }
                1 => *self.current_modifiers |= Modifier::BOLD,
                2 => *self.current_modifiers |= Modifier::DIM,
                3 => *self.current_modifiers |= Modifier::ITALIC,
                4 => *self.current_modifiers |= Modifier::UNDERLINED,
                5 => *self.current_modifiers |= Modifier::SLOW_BLINK,
                7 => *self.current_modifiers |= Modifier::REVERSED,
                8 => *self.current_modifiers |= Modifier::HIDDEN,
                9 => *self.current_modifiers |= Modifier::CROSSED_OUT,
                // Reset attributes
                21 | 22 => {
                    *self.current_modifiers -= Modifier::BOLD;
                    *self.current_modifiers -= Modifier::DIM;
                }
                23 => *self.current_modifiers -= Modifier::ITALIC,
                24 => *self.current_modifiers -= Modifier::UNDERLINED,
                25 => *self.current_modifiers -= Modifier::SLOW_BLINK,
                27 => *self.current_modifiers -= Modifier::REVERSED,
                28 => *self.current_modifiers -= Modifier::HIDDEN,
                29 => *self.current_modifiers -= Modifier::CROSSED_OUT,
                // Standard foreground colors (30-37)
                30 => *self.current_fg = Color::Black,
                31 => *self.current_fg = Color::Red,
                32 => *self.current_fg = Color::Green,
                33 => *self.current_fg = Color::Yellow,
                34 => *self.current_fg = Color::Blue,
                35 => *self.current_fg = Color::Magenta,
                36 => *self.current_fg = Color::Cyan,
                37 => *self.current_fg = Color::White,
                // Extended foreground: 38;5;N (256-color) or 38;2;R;G;B (truecolor)
                38 => {
                    if i + 2 < params.len() && params[i + 1] == 5 {
                        // 256-color
                        *self.current_fg = Color::Indexed(params[i + 2] as u8);
                        i += 2;
                    } else if i + 4 < params.len() && params[i + 1] == 2 {
                        // Truecolor
                        *self.current_fg = Color::Rgb(
                            params[i + 2] as u8,
                            params[i + 3] as u8,
                            params[i + 4] as u8,
                        );
                        i += 4;
                    }
                }
                39 => *self.current_fg = Color::Reset,
                // Standard background colors (40-47)
                40 => *self.current_bg = Color::Black,
                41 => *self.current_bg = Color::Red,
                42 => *self.current_bg = Color::Green,
                43 => *self.current_bg = Color::Yellow,
                44 => *self.current_bg = Color::Blue,
                45 => *self.current_bg = Color::Magenta,
                46 => *self.current_bg = Color::Cyan,
                47 => *self.current_bg = Color::White,
                // Extended background: 48;5;N (256-color) or 48;2;R;G;B (truecolor)
                48 => {
                    if i + 2 < params.len() && params[i + 1] == 5 {
                        *self.current_bg = Color::Indexed(params[i + 2] as u8);
                        i += 2;
                    } else if i + 4 < params.len() && params[i + 1] == 2 {
                        *self.current_bg = Color::Rgb(
                            params[i + 2] as u8,
                            params[i + 3] as u8,
                            params[i + 4] as u8,
                        );
                        i += 4;
                    }
                }
                49 => *self.current_bg = Color::Reset,
                // Bright foreground colors (90-97)
                90 => *self.current_fg = Color::DarkGray,
                91 => *self.current_fg = Color::LightRed,
                92 => *self.current_fg = Color::LightGreen,
                93 => *self.current_fg = Color::LightYellow,
                94 => *self.current_fg = Color::LightBlue,
                95 => *self.current_fg = Color::LightMagenta,
                96 => *self.current_fg = Color::LightCyan,
                97 => *self.current_fg = Color::Gray,
                // Bright background colors (100-107)
                100 => *self.current_bg = Color::DarkGray,
                101 => *self.current_bg = Color::LightRed,
                102 => *self.current_bg = Color::LightGreen,
                103 => *self.current_bg = Color::LightYellow,
                104 => *self.current_bg = Color::LightBlue,
                105 => *self.current_bg = Color::LightMagenta,
                106 => *self.current_bg = Color::LightCyan,
                107 => *self.current_bg = Color::Gray,
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
}
