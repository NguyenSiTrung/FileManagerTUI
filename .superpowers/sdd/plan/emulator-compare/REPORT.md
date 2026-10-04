# Emulator comparison harness — measured results

Phase 9 Task 1 fix-round item 1. The original report asserted the
`alacritty_terminal` row from API knowledge; this harness now **measures** both
reference emulators against the eight canonical fixture sequences that the
`fixture_*` unit tests encode, and prints observable state for each.

## Layout

- `emulator-compare/` — binary crate that `include!`s `src/terminal/emulator.rs`
  (verbatim modulo `//!` → `//`, see `build.rs`) and drives
  `alacritty_terminal 0.26.0` `Term` through the same byte sequences.
- `vt100-probe/` — separate binary crate for `vt100 0.16.2`. It cannot live in
  the same dependency tree: `ratatui 0.29` pins `unicode-width =0.2.0` while
  `vt100 0.16` requires `^0.2.1`.

Run: `cargo run` in each crate.

## Measured per-fixture rows (cargo run, 2026-10-04)

### fixture_dsr_and_da_replies_are_ordered

- fm: `replies="\x1b[1;3R\x1b[?1;2c"` then `"\x1b[0n"`
- alacritty: `replies=["\x1b[1;3R", "\x1b[?6c"]` then `["\x1b[0n"]`
- vt100: **no reply channel** — query bytes are consumed with no observable
  output (cursor stayed at `(0, 2)`).

Difference measured, not a defect: both DA replies are VT102-family
(`?1;2` = VT100-with-AVO vs `?6` = VT102). Ordering of DSR-then-DA is identical.

### fixture_alternate_screen_enter_and_leave

- fm: `alt=true altCell='A' restored row0="PRIMARY             " alt_after=false`
- alacritty: identical.
- vt100: identical.

### fixture_scroll_region_confines_line_feed

- fm: `region=(1, 2) rows=["AAA   ", "CCC   ", "      ", "DDD   "] cursor=(2, 0)`
- alacritty: `rows=["AAA   ", "CCC   ", "      ", "DDD   "] cursor=(2, 0)`
- vt100: `rows=["AAA", "CCC", "", "DDD"] cursor=(2, 0)`

All three: line feed at the region bottom scrolls only inside DECSTBM bounds.

### fixture_wide_characters_occupy_two_cells

- fm: `row0="中 文 A               " cursor=(0, 5)`
- alacritty: identical.
- vt100: `row0="中文A" cursor=(0, 5)` — same cursor; its `contents()` rendering
  collapses the spacer cell where fm/alacritty store a blank continuation.

### fixture_combining_mark_attaches_to_base_cell

- fm: `cell00='e' comb="́" cursor=(0, 1) extract="é"`
- alacritty: `cell00="é" cursor=(0, 1)`
- vt100: `cell00="é" row0="é" cursor=(0, 1)`

fm stores the combining mark in a side field; alacritty/vt100 fold it into the
cell's grapheme. Observable text is identical.

### fixture_resize_grow_and_shrink_preserve_content

- fm: grow `8×24` keeps `HELLO`/`WORLD`; shrink to `3×8` keeps `HELLO`,
  `region=(0, 2)`, `cursor=(1, 5)`
- alacritty: identical rows and cursor `(1, 5)`.
- vt100: identical rows and cursor `(1, 5)`.

### fixture_resize_landing_mid_escape_sequence_keeps_parser_state

- fm: `row0="Z                   " cursor=(0, 1)`
- alacritty: identical.
- vt100: identical.

### fixture_cursor_visibility_and_shape_modes

- fm: `visible=false shape=Block` (after `?25l`, `4 q`, `6 q`, `2 q`)
- alacritty: `cursor_style={shape: Block, blinking: false}`,
  `SHOW_CURSOR` cleared by `?25l` — identical semantics.
- vt100: `hide_cursor=true` (cursor-state bytes show the `?25l` mode record).

## Dependency counts — measured

Commands (from each crate's dir):

```bash
cargo tree -p vt100 --all-targets --edges all --prefix none | sort -u
cargo tree -p alacritty_terminal --all-targets --edges all --prefix none | sort -u
```

- **vt100 0.16.2**: **4 unique transitive dependencies** — `itoa`, `memchr`,
  `unicode-width`, `vte` (5 names including vt100 itself), across all target
  platforms and edge kinds. The earlier figure "11 unique" does not reproduce
  for 0.16.2 under any edge/target selection.
- **alacritty_terminal 0.26.0**: **54 unique dependency names** in its full
  closure across all target platforms, **55 including the crate itself**
  (normal+build+dev edges; Windows-only entries like `windows-sys`,
  `windows_x86_64_msvc`, `miow`, `rustix-openpty` counted). The earlier figure
  "57 lock names incl. root/Windows" is off by 2–3; the measured lock-closure
  is 54/55.

Non-target deps of alacritty_terminal in the lock file (101 total packages in
the harness lock, including ratatui/vte/unicode-width for the fm include):
the closure list is the `cargo tree` output above, which is the accurate
"adoption cost" number.
