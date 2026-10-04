//! vt100 reference-emulator probe for the Phase 9 Task 1 fixture sequences.
//! Kept in its own crate because vt100 0.16 requires unicode-width ^0.2.1,
//! which conflicts with ratatui 0.29's `=0.2.0` pin in the main compare crate.

fn row_string(screen: &vt100::Screen, row: u16) -> String {
    let mut s = String::new();
    for c in 0..screen.size().1 {
        if let Some(cell) = screen.cell(row, c) {
            s.push_str(&cell.contents());
        }
    }
    s
}

fn main() {
    println!("== fixture: dsr_and_da_replies_are_ordered ==");
    {
        let mut p = vt100::Parser::new(6, 20, 0);
        p.process(b"AB");
        p.process(b"\x1b[6n");
        p.process(b"\x1b[c");
        p.process(b"\x1b[5n");
        // vt100's Screen has no reply/write-back channel; query bytes are
        // consumed by the parser with no observable output.
        println!(
            "vt100:      no reply channel (cursor at {:?}, row0={:?})",
            p.screen().cursor_position(),
            row_string(p.screen(), 0)
        );
    }

    println!("== fixture: alternate_screen_enter_and_leave ==");
    {
        let mut p = vt100::Parser::new(6, 20, 0);
        p.process(b"PRIMARY");
        p.process(b"\x1b[?1049h");
        let in_alt = p.screen().alternate_screen();
        p.process(b"\x1b[1;1HALT");
        let alt_row = row_string(p.screen(), 0);
        p.process(b"\x1b[?1049l");
        println!(
            "vt100:      alt={} altRow0={:?} restored row0={:?} alt_after={}",
            in_alt,
            alt_row,
            row_string(p.screen(), 0),
            p.screen().alternate_screen()
        );
    }

    println!("== fixture: scroll_region_confines_line_feed ==");
    {
        let mut p = vt100::Parser::new(6, 6, 0);
        for seq in [
            b"\x1b[1;1HAAA" as &[u8],
            b"\x1b[2;1HBBB",
            b"\x1b[3;1HCCC",
            b"\x1b[4;1HDDD",
            b"\x1b[2;3r",
            b"\x1b[3;1H\n",
        ] {
            p.process(seq);
        }
        println!(
            "vt100:      rows={:?} cursor={:?}",
            [
                row_string(p.screen(), 0),
                row_string(p.screen(), 1),
                row_string(p.screen(), 2),
                row_string(p.screen(), 3)
            ],
            p.screen().cursor_position()
        );
    }

    println!("== fixture: wide_characters_occupy_two_cells ==");
    {
        let mut p = vt100::Parser::new(4, 20, 0);
        p.process("中文".as_bytes());
        p.process(b"A");
        println!(
            "vt100:      row0={:?} cursor={:?}",
            row_string(p.screen(), 0),
            p.screen().cursor_position()
        );
    }

    println!("== fixture: combining_mark_attaches_to_base_cell ==");
    {
        let mut p = vt100::Parser::new(4, 20, 0);
        p.process("e\u{0301}".as_bytes());
        println!(
            "vt100:      cell00={:?} row0={:?} cursor={:?}",
            p.screen().cell(0, 0).map(|c| c.contents()),
            row_string(p.screen(), 0),
            p.screen().cursor_position()
        );
    }

    println!("== fixture: resize_grow_and_shrink_preserve_content ==");
    {
        let mut p = vt100::Parser::new(3, 5, 0);
        p.process(b"HELLO\r\nWORLD");
        p.screen_mut().set_size(8, 24);
        let grown0 = row_string(p.screen(), 0);
        let grown1 = row_string(p.screen(), 1);
        p.screen_mut().set_size(3, 8);
        println!(
            "vt100:      grown rows {:?} {:?} shrunk row0={:?} cursor={:?}",
            grown0,
            grown1,
            row_string(p.screen(), 0),
            p.screen().cursor_position()
        );
    }

    println!("== fixture: resize_landing_mid_escape_sequence_keeps_parser_state ==");
    {
        let mut p = vt100::Parser::new(4, 10, 0);
        p.process(b"\x1b[1");
        p.screen_mut().set_size(8, 20);
        p.process(b"HZ");
        println!(
            "vt100:      row0={:?} cursor={:?}",
            row_string(p.screen(), 0),
            p.screen().cursor_position()
        );
    }

    println!("== fixture: cursor_visibility_and_shape_modes ==");
    {
        let mut p = vt100::Parser::new(6, 20, 0);
        p.process(b"\x1b[?25l\x1b[4 q\x1b[6 q\x1b[2 q");
        println!(
            "vt100:      hide_cursor={} cursor_state={:?}",
            p.screen().hide_cursor(),
            p.screen().cursor_state_formatted()
        );
    }
}
