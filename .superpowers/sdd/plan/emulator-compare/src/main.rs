//! Phase 9 Task 1 comparison harness (rebuilt): feeds each canonical fixture
//! sequence through our `TerminalEmulator` and `alacritty_terminal::Term` and
//! prints measured observable state. `vt100` lives in the sibling
//! `vt100-probe` crate because it conflicts on `unicode-width` with ratatui.
//!
//! This is measurement, not a test gate — the report records what each
//! reference emulator actually produced so the task evidence stops relying on
//! API-knowledge assertions.

mod fm {
    include!(concat!(env!("OUT_DIR"), "/emulator_inc.rs"));
}

use fm::TerminalEmulator;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi;

use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
struct Size {
    cols: usize,
    lines: usize,
}

impl Size {
    fn new(rows: usize, cols: usize) -> Self {
        Self { cols, lines: rows }
    }
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

#[derive(Clone, Default)]
struct Listener {
    writes: Arc<Mutex<Vec<String>>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(text) = event {
            self.writes.lock().unwrap().push(text);
        }
    }
}

struct AlacHarness {
    term: Term<Listener>,
    listener: Listener,
    processor: ansi::Processor,
}

impl AlacHarness {
    fn new(rows: usize, cols: usize) -> Self {
        let listener = Listener::default();
        let config = Config::default();
        let term = Term::new(config, &Size::new(rows, cols), listener.clone());
        Self {
            term,
            listener,
            processor: ansi::Processor::new(),
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
    }

    fn resize(&mut self, rows: usize, cols: usize) {
        self.term.resize(Size::new(rows, cols));
    }

    fn cell_ch(&self, row: i32, col: usize) -> char {
        self.term.grid()[Line(row)][Column(col)].c
    }

    fn cell_text(&self, row: i32, col: usize) -> String {
        let cell = &self.term.grid()[Line(row)][Column(col)];
        let mut s = cell.c.to_string();
        for &zw in cell.zerowidth().unwrap_or(&[]) {
            s.push(zw);
        }
        s
    }

    fn row_string(&self, row: i32) -> String {
        let line = &self.term.grid()[Line(row)];
        (0..self.term.columns())
            .map(|c| line[Column(c)].c)
            .collect::<String>()
    }

    fn cursor(&self) -> (i32, usize) {
        let p = self.term.grid().cursor.point;
        (p.line.0, p.column.0)
    }

    fn alt(&self) -> bool {
        self.term
            .mode()
            .contains(alacritty_terminal::term::TermMode::ALT_SCREEN)
    }

    fn replies(&mut self) -> Vec<String> {
        std::mem::take(&mut *self.listener.writes.lock().unwrap())
    }
}

fn fm_row(emu: &TerminalEmulator, row: usize) -> String {
    (0..emu.visible_cols())
        .map(|c| emu.cell_at(row, c).map(|cell| cell.ch).unwrap_or('?'))
        .collect()
}

fn main() {
    println!("== fixture: dsr_and_da_replies_are_ordered ==");
    {
        let mut emu = TerminalEmulator::new(6, 20);
        emu.process(b"AB");
        emu.process(b"\x1b[6n");
        emu.process(b"\x1b[c");
        let r1 = String::from_utf8_lossy(&emu.take_replies()).to_string();
        emu.process(b"\x1b[5n");
        let r2 = String::from_utf8_lossy(&emu.take_replies()).to_string();
        println!("fm:         replies={:?} then {:?}", r1, r2);

        let mut alac = AlacHarness::new(6, 20);
        alac.feed(b"AB");
        alac.feed(b"\x1b[6n");
        alac.feed(b"\x1b[c");
        let r1 = alac.replies();
        alac.feed(b"\x1b[5n");
        let r2 = alac.replies();
        println!("alacritty:  replies={:?} then {:?}", r1, r2);
    }

    println!("== fixture: alternate_screen_enter_and_leave ==");
    {
        let mut emu = TerminalEmulator::new(6, 20);
        emu.process(b"PRIMARY");
        emu.process(b"\x1b[?1049h");
        let in_alt = emu.alternate_screen();
        emu.process(b"\x1b[1;1HALT");
        let alt_cell = emu.cell_at(0, 0).map(|c| c.ch);
        emu.process(b"\x1b[?1049l");
        println!(
            "fm:         alt={} altCell={:?} restored row0={:?} alt_after={}",
            in_alt,
            alt_cell,
            fm_row(&emu, 0),
            emu.alternate_screen()
        );

        let mut alac = AlacHarness::new(6, 20);
        alac.feed(b"PRIMARY");
        alac.feed(b"\x1b[?1049h");
        let in_alt = alac.alt();
        alac.feed(b"\x1b[1;1HALT");
        let alt_cell = alac.cell_ch(0, 0);
        alac.feed(b"\x1b[?1049l");
        println!(
            "alacritty:  alt={} altCell={:?} restored row0={:?} alt_after={}",
            in_alt,
            alt_cell,
            alac.row_string(0),
            alac.alt()
        );
    }

    println!("== fixture: scroll_region_confines_line_feed ==");
    {
        let mut emu = TerminalEmulator::new(6, 6);
        emu.process(b"\x1b[1;1HAAA");
        emu.process(b"\x1b[2;1HBBB");
        emu.process(b"\x1b[3;1HCCC");
        emu.process(b"\x1b[4;1HDDD");
        emu.process(b"\x1b[2;3r");
        let region = emu.scroll_region();
        emu.process(b"\x1b[3;1H\n");
        println!(
            "fm:         region={:?} rows={:?} cursor={:?}",
            region,
            [
                fm_row(&emu, 0),
                fm_row(&emu, 1),
                fm_row(&emu, 2),
                fm_row(&emu, 3)
            ],
            emu.cursor_position()
        );

        let mut alac = AlacHarness::new(6, 6);
        for seq in [
            b"\x1b[1;1HAAA" as &[u8],
            b"\x1b[2;1HBBB",
            b"\x1b[3;1HCCC",
            b"\x1b[4;1HDDD",
            b"\x1b[2;3r",
            b"\x1b[3;1H\n",
        ] {
            alac.feed(seq);
        }
        println!(
            "alacritty:  rows={:?} cursor={:?}",
            [
                alac.row_string(0),
                alac.row_string(1),
                alac.row_string(2),
                alac.row_string(3)
            ],
            alac.cursor()
        );
    }

    println!("== fixture: wide_characters_occupy_two_cells ==");
    {
        let mut emu = TerminalEmulator::new(4, 20);
        emu.process("中文".as_bytes());
        emu.process(b"A");
        println!(
            "fm:         row0={:?} cursor={:?}",
            fm_row(&emu, 0),
            emu.cursor_position()
        );

        let mut alac = AlacHarness::new(4, 20);
        alac.feed("中文".as_bytes());
        alac.feed(b"A");
        println!(
            "alacritty:  row0={:?} cursor={:?}",
            alac.row_string(0),
            alac.cursor()
        );
    }

    println!("== fixture: combining_mark_attaches_to_base_cell ==");
    {
        let mut emu = TerminalEmulator::new(4, 20);
        emu.process("e\u{0301}".as_bytes());
        println!(
            "fm:         cell00={:?} comb={:?} cursor={:?} extract={:?}",
            emu.cell_at(0, 0).map(|c| c.ch),
            emu.cell_at(0, 0).map(|c| c.combining.clone()),
            emu.cursor_position(),
            emu.extract_text(0, 0, 0, 1)
        );

        let mut alac = AlacHarness::new(4, 20);
        alac.feed("e\u{0301}".as_bytes());
        println!(
            "alacritty:  cell00={:?} cursor={:?}",
            alac.cell_text(0, 0),
            alac.cursor()
        );
    }

    println!("== fixture: resize_grow_and_shrink_preserve_content ==");
    {
        let mut emu = TerminalEmulator::new(3, 5);
        emu.process(b"HELLO\r\nWORLD");
        emu.resize(8, 24);
        let grown = (
            emu.visible_rows(),
            emu.visible_cols(),
            fm_row(&emu, 0),
            fm_row(&emu, 1),
        );
        emu.resize(3, 8);
        println!(
            "fm:         grown={:?} shrunk=({}, {}) row0={:?} region={:?} cursor={:?}",
            grown,
            emu.visible_rows(),
            emu.visible_cols(),
            fm_row(&emu, 0),
            emu.scroll_region(),
            emu.cursor_position()
        );

        let mut alac = AlacHarness::new(3, 5);
        alac.feed(b"HELLO\r\nWORLD");
        alac.resize(8, 24);
        let grown0 = alac.row_string(0);
        let grown1 = alac.row_string(1);
        alac.resize(3, 8);
        println!(
            "alacritty:  grown rows {:?} {:?} shrunk row0={:?} cursor={:?}",
            grown0,
            grown1,
            alac.row_string(0),
            alac.cursor()
        );
    }

    println!("== fixture: resize_landing_mid_escape_sequence_keeps_parser_state ==");
    {
        let mut emu = TerminalEmulator::new(4, 10);
        emu.process(b"\x1b[1");
        emu.resize(8, 20);
        emu.process(b"HZ");
        println!(
            "fm:         row0={:?} cursor={:?}",
            fm_row(&emu, 0),
            emu.cursor_position()
        );

        let mut alac = AlacHarness::new(4, 10);
        alac.feed(b"\x1b[1");
        alac.resize(8, 20);
        alac.feed(b"HZ");
        println!(
            "alacritty:  row0={:?} cursor={:?}",
            alac.row_string(0),
            alac.cursor()
        );
    }

    println!("== fixture: cursor_visibility_and_shape_modes ==");
    {
        let mut emu = TerminalEmulator::new(6, 20);
        emu.process(b"\x1b[?25l\x1b[4 q\x1b[6 q\x1b[2 q");
        println!(
            "fm:         visible={} shape={:?}",
            emu.cursor_visible(),
            emu.cursor_shape()
        );

        let mut alac = AlacHarness::new(6, 20);
        alac.feed(b"\x1b[?25l\x1b[4 q\x1b[6 q\x1b[2 q");
        println!(
            "alacritty:  cursor_style={:?} mode_has_show_cursor={}",
            alac.term.cursor_style(),
            alac.term
                .mode()
                .contains(alacritty_terminal::term::TermMode::SHOW_CURSOR)
        );
    }
}
