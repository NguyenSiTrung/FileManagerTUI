# Plan: File Operations + Dialogs

## Phase 1: File Operations Backend
<!-- execution: sequential -->

- [x] Task 1: Create `fs/operations.rs` with CRUD functions
  <!-- files: src/fs/operations.rs, src/fs/mod.rs -->
  - [x] Add `fs/operations.rs` module and register in `fs/mod.rs`
  - [x] Implement `create_file(path: &Path) -> Result<()>`
  - [x] Implement `create_dir(path: &Path) -> Result<()>`
  - [x] Implement `rename(from: &Path, to: &Path) -> Result<()>`
  - [x] Implement `delete(path: &Path) -> Result<()>` with recursive directory support
  - [x] Write unit tests for all four operations using `tempfile::TempDir`
  - [x] Test error cases: duplicate name, permission denied, non-existent path

- [x] Task 2: Conductor - User Manual Verification 'File Operations Backend' (Protocol in workflow.md)

## Phase 2: Dialog Data Model & App State
<!-- execution: sequential -->
<!-- depends: -->

- [x] Task 1: Add dialog types and app state transitions
  <!-- files: src/app.rs -->
  - [x] Add `DialogKind` enum to `app.rs`: `CreateFile`, `CreateDirectory`, `Rename { original: PathBuf }`, `DeleteConfirm { targets: Vec<PathBuf> }`, `Error { message: String }`
  - [x] Update `AppMode` enum with `Dialog(DialogKind)` variant
  - [x] Add `DialogState` struct: `input: String`, `cursor_position: usize`
  - [x] Add `dialog_state: DialogState` and `status_message: Option<(String, Instant)>` fields to `App`
  - [x] Implement dialog open/close methods on `App`: `open_dialog(kind)`, `close_dialog()`, `dialog_input_char()`, `dialog_delete_char()`, `dialog_move_cursor_left()`, `dialog_move_cursor_right()`, `dialog_cursor_home()`, `dialog_cursor_end()`
  - [x] Implement `set_status_message(&mut self, msg: String)` and `clear_expired_status(&mut self)` (3s timeout)
  - [x] Add method to determine current directory for file creation (parent of selected item if file, selected item if directory)
  - [x] Write unit tests for dialog state transitions and cursor movement

- [x] Task 2: Conductor - User Manual Verification 'Dialog Data Model & App State' (Protocol in workflow.md)

## Phase 3: Dialog Widget
<!-- execution: sequential -->
<!-- depends: phase2 -->

- [x] Task 1: Create `components/dialog.rs` widget
  <!-- files: src/components/dialog.rs, src/components/mod.rs, src/ui.rs -->
  - [x] Add `components/dialog.rs` module and register in `components/mod.rs`
  - [x] Implement input dialog rendering: centered `Block` with `Clear`, title, input line with visible cursor
  - [x] Implement confirmation dialog rendering: centered block listing target paths, `[y] Yes / [n/Esc] Cancel` footer
  - [x] Implement error dialog rendering: centered block with error message, `[Enter/Esc] Dismiss` footer
  - [x] Integrate dialog rendering into `ui.rs` — render dialog overlay on top of tree when `AppMode::Dialog`
  - [x] Write render tests verifying dialog layout

- [x] Task 2: Conductor - User Manual Verification 'Dialog Widget' (Protocol in workflow.md)

## Phase 4: Status Bar Widget
<!-- execution: sequential -->
<!-- depends: -->

- [x] Task 1: Create `components/status_bar.rs` widget
  <!-- files: src/components/status_bar.rs, src/components/mod.rs, src/ui.rs -->
  - [x] Add `components/status_bar.rs` module and register in `components/mod.rs`
  - [x] Implement status bar widget: left (path), center (size | type | perms), right (key hints)
  - [x] Render status message when present (overrides normal content, styled for success/error)
  - [x] Integrate into `ui.rs` layout — bottom bar below tree panel
  - [x] Write render tests

- [x] Task 2: Conductor - User Manual Verification 'Status Bar Widget' (Protocol in workflow.md)

## Phase 5: Handler Integration & Wiring
<!-- execution: sequential -->
<!-- depends: phase1, phase2, phase3, phase4 -->

- [x] Task 1: Wire Normal mode keys to dialog openers
  <!-- files: src/handler.rs -->
  - [x] `a` → `app.open_dialog(DialogKind::CreateFile)`
  - [x] `A` → `app.open_dialog(DialogKind::CreateDirectory)`
  - [x] `r` → `app.open_dialog(DialogKind::Rename { original })` with pre-filled name
  - [x] `d` → `app.open_dialog(DialogKind::DeleteConfirm { targets })`
  - [x] Write handler tests for each key binding

- [x] Task 2: Wire Dialog mode key handling
  <!-- files: src/handler.rs -->
  <!-- depends: task1 -->
  - [x] Character input → `dialog_input_char()`
  - [x] Backspace → `dialog_delete_char()`
  - [x] Left/Right → cursor movement
  - [x] Home/End → cursor jump
  - [x] Esc → `close_dialog()` (return to Normal)
  - [x] Enter → execute operation based on `DialogKind`, refresh tree, set status message, close dialog
  - [x] `y`/`n` in DeleteConfirm → confirm or cancel
  - [x] On fs error → set error status message
  - [x] Write handler tests for dialog key handling

- [x] Task 3: Tree refresh after operations
  <!-- files: src/app.rs, src/fs/tree.rs -->
  <!-- depends: task2 -->
  - [x] After create/rename/delete, reload affected parent directory children and re-flatten
  - [x] Ensure selected_index remains valid after tree refresh
  - [x] Write integration tests: key sequence → verify tree state changes

- [x] Task 4: Conductor - User Manual Verification 'Handler Integration & Wiring' (Protocol in workflow.md)
