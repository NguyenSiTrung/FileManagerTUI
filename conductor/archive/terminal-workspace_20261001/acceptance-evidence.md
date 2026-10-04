# Acceptance Evidence — terminal-workspace_20261001

Final checkpoint for the terminal-workspace track. Every requirement is
covered by **automated** evidence only — this track has no manual
verification gate. Gate output reproduced from the actual run at
2026-10-04 (commit `acd705d`).

## Mandatory gate results

| Gate | Command | Result |
|---|---|---|
| Format | `cargo fmt --check` | PASS |
| Clippy | `cargo clippy -- -D warnings` | PASS |
| Clippy (all targets) | `cargo clippy --all-targets -- -D warnings` | PASS |
| Unit/integration tests | `cargo test` | PASS — 1524/1524 |
| Release build | `cargo build --release` | PASS |
| Binary size | stat `target/release/fm` | PASS — 4,034,400 B ≈ 3.85 MiB (< 10 MiB NFR) |
| PTY acceptance matrix | `python3 scripts/test-terminal-workspace.py` | PASS — 13/13 scenarios |
| Browser-terminal suite | `npm --prefix tools/terminal-tests test` | PASS — 7/7 tests |
| Coverage evidence | `cargo llvm-cov --summary-only` | PASS — 94.6% lines, 94.8% functions |
| Static musl build | `cargo build --release --target x86_64-unknown-linux-musl` | PASS — 4,158,072 B statically-linked ELF |
| macOS/Windows builds | `release-builds` matrix job in `ci.yml` | AUTOMATED on tag/dispatch — not run locally (no macOS/Windows hardware here); the check is wired and validated, evidence lands on the next tag run |
| Combined runner | `scripts/check-terminal-workspace.sh` | PASS — "all mandatory gates passed", exit 0 |

## PTY scenarios (scripts/test-terminal-workspace.py — 13/13)

`smoke`, `workflow`, `resize`, `term_variants`, `nested_tmux`,
`feature_flags_off`, `missing_lsp_executable`, `fake_lsp_diagnostics`,
`external_save`, `missing_git`, `git_markers`,
`crash_then_recovery_prompt`, `corrupt_recovery_is_ignored` — each with
deadline-bounded reads and clean-exit/reap asserts.

## Browser tests (tools/terminal-tests/workspace.spec.mjs — 7/7)

web-profile command route; clean quit + teardown process sweep;
browser-reserved Ctrl+P exclusion + Alt+G o; full workflow with exact
file-byte assert; page-driven PTY resize; browser copy fallback without
fm mouse protocol; feature-off flags.

## Requirement → test map

| Requirement | Automated evidence |
|---|---|
| FR-1 Collision-Safe File Operations | `fs/operations.rs` tests (create/rename refuse collisions); `workspace::documents` lifecycle tests; PTY `workflow` create/edit/save path |
| FR-2 Safe Saves and External Changes | `fs/save.rs` tests (revision validation, private mode/ownership preserved, symlink/hardlink/read-only refusals); `workspace::documents` external-mark + revision tests; PTY `external_save` (polling watcher → `! [pin]`), `workflow` exact-byte save + dirty-quit cancel |
| FR-3 Editing, Paste, Clipboard | `editor.rs`, `text.rs`, `app_jobs_clipboard_*` tests; PTY `workflow` bracketed YAML paste → exact bytes; browser `browser copy fallback` test; CR-paste normalization regression test (`bracketed_paste_cr_line_endings_normalize_to_lf`) |
| FR-4 Documents Independent of Focus | `workspace::documents` local-state/dedup/lifecycle tests; `workspace::focus` routing tests; PTY `workflow` document switching retains buffer/cursor |
| FR-5 Adaptive Workspace Layout | `workspace::layout` geometry/bounds tests; `ui::adaptive_*` tests; PTY `resize` (80x24 → 120x40 → 60x20), `term_variants`; browser `pty resize` |
| FR-6 Commands and Browser Profiles | `keymap.rs`, `commands.rs` tests; `command_menu`/`settings` component tests; browser `web profile`, `browser-reserved Ctrl+P` tests; PTY Alt+G routes in every scenario |
| FR-7 Navigation, Search, Background Work | `search.rs`, `app_jobs_*` generation/cancellation tests; PTY Quick-Open (`▸` candidate waits) in workflow/missing-lsp/crash/corrupt scenarios |
| FR-8 Sessions and Private Recovery | `recovery.rs`, `session.rs` tests + `recovery_integration_tests`; PTY `crash_then_recovery_prompt` (SIGKILL → prompt → decline + restore), `corrupt_recovery_is_ignored` (bad/foreign records refused, disk bytes opened) |
| FR-9 Read-Only Git Indicators | `git.rs` porcelain parser tests; `ui::git_indicators_*` tests; PTY `git_markers` (zeta-branch on status bar), `missing_git` (silent degradation); `READ_ONLY_ARGUMENTS` whitelist — no write verbs exist in code |
| FR-10 Installed-Server LSP and Diagnostics | `lsp/{client,config,transport,features,positions,mod}` tests incl. fake-peer e2e; `diagnostics.rs` versioned apply tests; PTY `fake_lsp_diagnostics` (`E:1` at 120 cols), `missing_lsp_executable`; trust model via `lsp::config` tests |
| FR-11 Embedded-Terminal Correctness | `terminal/{pty,emulator,mod}` tests (input ordering, bracketed paste, replies, resize, bounded shutdown); `transport_loop_tests::*terminal*`; PTY `workflow` shell `echo shell-ok`, `nested_tmux` |
| AC-1 | PTY `workflow` — 80x24 two-file edit/browse/shell/paste-YAML/save with exact bytes (`readFixtureFile` equivalent: direct file read) + retained dirty state; browser `full workflow` mirrors it |
| AC-2 | `fs/operations` + `fs/save` collision/refusal tests; `workspace::documents` injected-loader-failure tests |
| AC-3 | PTY `workflow` dirty-quit cancel; `crash_then_recovery_prompt`, `corrupt_recovery_is_ignored`; `workspace::documents` rename/delete/revision tests |
| AC-4 | `text.rs` grapheme/tab/wide-char tests; `editor.rs` selection/undo tests |
| AC-5 | `workspace::layout` rect-bounds tests; `ui::adaptive_*`/`tiny_*` tests; PTY `resize` |
| AC-6 | `app_jobs_*` delayed/cancelled-generation matrix; `workspace::documents` injected-loader-failure tests |
| AC-7 | `git.rs` temp-repo fixture tests (clean/dirty/untracked/unborn); `ui::git_indicators_*`; PTY `git_markers`, `missing_git` |
| AC-8 | `lsp/*` fake-server tests + `scripts/fake-lsp-server.py` peer in PTY `fake_lsp_diagnostics`, `missing_lsp_executable`; client transport framing/fault tests |
| AC-9 | `terminal/pty.rs` transport tests (input ordering, replies, paste, resize, EOF, bounded shutdown); PTY `smoke`/`workflow`/`nested_tmux`/`term_variants` |
| AC-10 | Browser suite 7/7 — controlled local xterm.js fixture, no saved auth; explicitly **not** a live SSH/Jupyter/Kubeflow deployment claim |
| AC-11 | PTY `feature_flags_off`, `missing_git`, `missing_lsp_executable`, `external_save`; `round1_binary/large/notebook/readonly` UI tests; `s3/*` read-only tests |
| AC-12 | The gate table above — all mandatory checks pass, coverage/build evidence included |

## Known boundaries (recorded, not claimed)

- Live SSH/Jupyter/Kubeflow deployment is untested — controlled PTY/tmux
  transport evidence only (AC-10 wording).
- macOS/Windows build checks are wired into `release-builds` in ci.yml but
  run only on tag/manual dispatch — no local hardware evidence this run.
- `main.rs`/`tui.rs` retain lower coverage (72%/42%) — the interactive
  `run()` loop is exercised by the PTY suite instead of unit coverage.
- OSC52 clipboard transport is unconfirmed against real clipboard targets.

## Status

Phases 1–11 closed; Phase 12 tasks 1–3 closed. All mandatory checks pass.
No criterion is satisfied by manual testing.
