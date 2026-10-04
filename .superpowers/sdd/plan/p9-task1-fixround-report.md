# Phase 9 Task 1 fix-round report

Response to review 90d27fef. Six items, each with the evidence produced.
All artifacts live under `.superpowers/sdd/plan/` and are committed this
time (the originals died with the previous VM).

## 1. Measured alacritty_terminal per-fixture row — DONE

`emulator-compare/` (scratch bin) include!s `src/terminal/emulator.rs`
verbatim (modulo `//!`→`//`, see `build.rs`) and drives
`alacritty_terminal 0.26.0::Term` through the same eight fixture byte
streams; `vt100-probe/` runs `vt100 0.16.2` in a second crate because vt100
requires `unicode-width ^0.2.1` while `ratatui 0.29` pins `=0.2.0` — the
two crates cannot share a resolve.

Measured rows for all 8 fixtures in `emulator-compare/REPORT.md`. Every
observable matches our emulator except the DA reply bytes: alacritty sends
`\x1b[?6c` (VT102) where we send `\x1b[?1;2c` (VT100-with-AVO). Both are
VT102-family replies; ours stays as implemented. vt100 has no reply channel
at all — measured, not asserted. pyte (PTY fixtures) cannot express
alt-screen save/restore at all (no buffer swap in `screens.py`); that gap
is documented in the fixture itself and covered by the alacritty/vt100 rows.

## 2. Dead-code accessors documented — DONE

`cursor_shape()`, `alternate_screen()`, `scroll_region()` and
`take_replies()` doc comments now state they are **staged observers**
consumed by Task 2 (PTY write-path drain) and Task 3 (terminal-state
plumbing), asserted today by the Task 1 fixtures. `emulator.rs:177–216`.

## 3. Reply-stall caveat stated — DONE

The `replies` field doc (`emulator.rs:102–106`) now says explicitly:
nothing drains the queue into the PTY writer yet, so a child blocked on a
DSR/DA reply stalls and the queue grows until Task 2 wires the drain.
`take_replies` repeats it at the call site.

## 4. Zero-dimension guards + compiled red-first 0x0 test — DONE

Nine guard sites in `csi_dispatch`/`esc_dispatch`:
`min(rows - 1)`/`min(cols - 1)` → `saturating_sub(1)` on
CUD `'B'`, CUF `'C'`, CUP/HVP `'H'|'f'` (×2), CNL `'E'`, CHA `'G'`,
CSI `'u'` (×2), DECRC `ESC 8` (×2); plus `cursor_row >= grid.len()` early
returns on ED `'J'` and EL `'K'`.

`coverage_degenerate_geometry_is_safe` extended with a 0x0 sequence
covering CUP/HVP, CUD, CUF, CNL, CHA, ED 0–3, EL 0–2, CSI save/restore and
DECSC/DECRC. **Red observed pre-fix**: the test panicked at
`emulator.rs:675` (`rows - 1` underflow) — a genuine subtract-overflow red,
not a neutered one. Green post-fix.

## 5. ED mode-1 branch covered, red-first — DONE

New `coverage_erase_display_mode_one_clears_through_cursor`: fills a 4×6
grid, parks the cursor at row 2 col 3, sends `\x1b[1J`, asserts rows 0–1
cleared, row 2 cleared through col 3, `grid[2][4] == 'C'`, row 3 intact.
The existing erase test parked the cursor at row 0 so `0..cursor_row`
never iterated — that is why the branch was reachable but uncovered.

**Neuter-to-red control**: `for r in 0..self.emu.cursor_row` → `0..0` made
the test fail (1 failed); restored → green.

## 6. Dependency counts corrected — DONE

Measured from the compare crates' `cargo tree` (`--all-targets --edges all`):

- **vt100 0.16.2**: **4 unique transitive deps** (itoa, memchr,
  unicode-width, vte), 5 names including itself. The "11 unique" figure
  does not reproduce under any edge/target selection.
- **alacritty_terminal 0.26.0**: **54 unique dep names** in its full
  closure across all target platforms — 55 including the crate itself
  (Windows-only entries like `windows-sys`, `miow`, `rustix-openpty`
  counted). "57 lock names incl. root/Windows" is off by 2–3.

## Re-verification (post-fix)

| Gate | Result |
| --- | --- |
| `cargo test --locked --offline` | **1354 passed**, 0 failed |
| `cargo clippy --locked --offline -- -D warnings` | clean |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | clean — required fixing **20 pre-existing** `byte_char_slices` lints in test code (`app.rs` 8, `tree.rs` 9, `git.rs` 3), none in `emulator.rs`; auto-applied via `clippy --fix` |
| `cargo build --locked --offline --release` | clean |
| `cargo fmt --check` | clean |
| Coverage, strict literal diff-inclusive basis | **19/19 executable added lines covered, 0 missed**; the 17 no-DA additions are all doc comments; 25 added lines sit inside `#[cfg(test)]` and are excluded |
| PTY fixtures | **11/11 pass** — rebuilt as `pty-env/pty_fixtures.py` driving real PTYs (child emits byte streams; `pyte 0.8.2` is the reference screen model; resize fixtures verify the pty-observable half: `stty size` sees TIOCSWINSZ, SIGWINCH reaches the child) |

Changed files this round: `src/terminal/emulator.rs` (guards, docs, two
tests), `src/app.rs` + `src/components/tree.rs` + `src/git.rs` (mechanical
clippy lint fixes — no behavior change), `.superpowers/sdd/plan/**`
(evidence crates, venv bootstrap, fixtures, reports).
