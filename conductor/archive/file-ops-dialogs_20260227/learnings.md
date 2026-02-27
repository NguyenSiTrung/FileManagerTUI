# Track Learnings: file-ops-dialogs_20260227

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Use `#[allow(dead_code)]` on struct fields/variants reserved for future milestones
- TreeState owns root TreeNode + flat_items Vec + selected_index; `flatten()` rebuilds flat list from tree recursively
- App delegates tree operations to TreeState methods; handler.rs maps keys to App methods
- Module structure: fs/tree.rs (data), components/tree.rs (widget), app.rs (state), handler.rs (input), event.rs (async events), tui.rs (terminal), ui.rs (layout)
- Root node must always bypass hidden filter in flatten — tempfile and some paths start with `.` prefix
- crossterm event polling is blocking — must run in spawned tokio task with mpsc channel
- Use `tempfile::TempDir` for filesystem tests; create helper `setup_test_dir()` or `setup_app()` for reuse

---

## [2026-02-27] - Phase 1 Task 1: fs/operations.rs CRUD functions
Thread: T-019c9e83-4bbb-72fc-8518-7fffcda86792
- **Implemented:** `create_file`, `create_dir`, `rename`, `delete` in `fs/operations.rs`
- **Files changed:** src/fs/operations.rs, src/fs/mod.rs
- **Commit:** 967a6f3
- **Learnings:**
  - Patterns: `fs::File::create` overwrites existing files (idempotent); `fs::create_dir` fails on duplicates
  - Gotchas: All fs operations auto-convert to AppError via `#[from] std::io::Error`

---

## [2026-02-27] - Phase 2 Task 1: Dialog data model & app state
Thread: T-019c9e83-4bbb-72fc-8518-7fffcda86792
- **Implemented:** DialogKind enum, AppMode::Dialog variant, DialogState, 11 dialog methods on App
- **Files changed:** src/app.rs
- **Commit:** 4f5cea7
- **Learnings:**
  - Gotchas: AppMode can no longer derive `Copy` once DialogKind contains heap types (PathBuf, Vec, String)
  - Patterns: Cursor position tracks byte offset, not char count — use `char.len_utf8()` for proper Unicode handling
  - Context: `current_dir()` returns parent for files, path itself for directories — needed for file creation target

---

## [2026-02-27] - Phase 3 Task 1: Dialog widget
Thread: T-019c9e83-4bbb-72fc-8518-7fffcda86792
- **Implemented:** `components/dialog.rs` with input, confirmation, and error dialog rendering
- **Files changed:** src/components/dialog.rs, src/components/mod.rs, src/ui.rs
- **Commit:** 2e12c3d
- **Learnings:**
  - Patterns: Use `Clear` widget + centered `Block` for modal overlays in ratatui
  - Patterns: `DialogWidget::centered_rect()` utility calculates centered Rect within area
  - Context: Dialog renders on top of entire frame area as overlay, not within tree area

---

## [2026-02-27] - Phase 4 Task 1: Status bar widget
Thread: T-019c9e83-4bbb-72fc-8518-7fffcda86792
- **Implemented:** `components/status_bar.rs` with path, file info, key hints, status message overlay
- **Files changed:** src/components/status_bar.rs, src/components/mod.rs
- **Commit:** 351c302
- **Learnings:**
  - Patterns: Status bar uses 3-section layout: left path, center info, right key hints
  - Patterns: Error messages use red bg/white fg, success uses green fg

---

## [2026-02-27] - Phase 5 Tasks 1-3: Handler integration & wiring
Thread: T-019c9e83-4bbb-72fc-8518-7fffcda86792
- **Implemented:** Modal handler dispatch (Normal vs Dialog mode), file operation execution, tree refresh
- **Files changed:** src/handler.rs, src/fs/tree.rs
- **Commit:** 1c962ad
- **Learnings:**
  - Patterns: Handler uses mode-based dispatch: `handle_normal_mode` vs `handle_dialog_mode`
  - Patterns: `TreeState::reload_dir()` reloads a specific directory's children and re-flattens
  - Gotchas: Must prevent delete on root node (check `depth > 0`)
  - Gotchas: Must clone DialogKind before matching to avoid borrow conflicts with `app`
  - Context: UI layout split to `[Min(3), Length(1)]` for tree + status bar
