# Plan: Preview Panel Edit Mode

## Phase 1: Editor State & Mode Infrastructure

- [ ] Task 1: Create `EditorState` struct
  - [ ] Create `src/editor.rs` module with `EditorState` struct
  - [ ] Fields: `buffer: Vec<String>`, `cursor_line: usize`, `cursor_col: usize`, `modified: bool`, `file_path: PathBuf`, `scroll_offset: usize`
  - [ ] Add `EditorState` field to `App` struct (`editor_state: Option<EditorState>`)
  - [ ] Register `mod editor` in `main.rs`
  - [ ] Write unit tests for `EditorState::new()`, `EditorState::from_file()`
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task 2: Add Edit mode variant to `AppMode`
  - [ ] Add `AppMode::Edit` variant to the enum in `app.rs`
  - [ ] Update all `match` arms on `AppMode` to handle `Edit` (handler.rs, ui.rs, any exhaustive matches)
  - [ ] Write test: entering Edit mode sets `AppMode::Edit`
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task 3: Enter/exit edit mode toggle
  - [ ] In `handle_preview_keys`: map `e` key to `app.enter_edit_mode()`
  - [ ] Implement `App::enter_edit_mode()`: load file into `EditorState`, set `AppMode::Edit`
  - [ ] Implement `App::exit_edit_mode()`: clear `editor_state`, set `AppMode::Normal`
  - [ ] Guard: skip binary files (status message), skip directories (no-op)
  - [ ] Write tests: `e` on text file enters Edit, `e` on binary shows message, `e` on dir is no-op
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task: Conductor - User Manual Verification 'Editor State & Mode Infrastructure' (Protocol in workflow.md)

## Phase 2: Basic Text Editing & Input

- [ ] Task 1: Character input and line operations
  - [ ] Implement `handle_editor_keys()` in `handler.rs`
  - [ ] Character insert: insert char at `(cursor_line, cursor_col)`, advance cursor
  - [ ] Backspace: delete char before cursor, join lines if at line start
  - [ ] Delete: delete char at cursor, join lines if at line end
  - [ ] Enter: split current line at cursor, insert new line, move cursor down
  - [ ] Write tests for each operation including edge cases (empty lines, first/last line)
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task 2: Cursor navigation
  - [ ] Arrow keys: move cursor up/down/left/right with bounds clamping
  - [ ] Home/End: jump to start/end of current line
  - [ ] Ctrl+Home / Ctrl+End: jump to first/last line of buffer
  - [ ] Page Up / Page Down: scroll by visible height, move cursor accordingly
  - [ ] Clamp cursor column to line length when moving between lines of different lengths
  - [ ] Write tests for navigation bounds and clamping
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task 3: Viewport scrolling
  - [ ] Track visible height from render area
  - [ ] Auto-scroll viewport to keep cursor visible (scroll up/down as needed)
  - [ ] Maintain scroll margin (cursor stays ≥2 lines from viewport edge when possible)
  - [ ] Write tests for scroll-follow-cursor logic
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task 4: Save to disk
  - [ ] `Ctrl+S` in `handle_editor_keys`: call `app.save_editor_buffer()`
  - [ ] `App::save_editor_buffer()`: write `buffer` lines to `file_path`, reset `modified` flag
  - [ ] Handle write errors: show error in status bar, keep buffer intact
  - [ ] After save: invalidate `last_previewed_index` so preview reloads on next view
  - [ ] Write tests: save writes correct content, error handling on read-only file
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task 5: Exit edit mode with save confirmation
  - [ ] `Esc` in Edit mode: check `modified` flag
  - [ ] If not modified: call `exit_edit_mode()` directly
  - [ ] If modified: show `DialogKind::SaveConfirm` dialog ("Save changes? Y/N/C")
  - [ ] Add `DialogKind::SaveConfirm` variant
  - [ ] Handle dialog: Y → save + exit, N → discard + exit, C/Esc → cancel (stay in edit)
  - [ ] Write tests for all three dialog outcomes
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task: Conductor - User Manual Verification 'Basic Text Editing & Input' (Protocol in workflow.md)

## Phase 3: Editor Widget & Rendering

- [ ] Task 1: Create `EditorWidget` with line numbers
  - [ ] Create `src/components/editor.rs` with `EditorWidget` struct
  - [ ] Follow builder pattern: `EditorWidget::new(state, theme).block(block)`
  - [ ] Render line number gutter with adaptive width (based on total lines digit count)
  - [ ] Highlight current line number with distinct style
  - [ ] Register `pub mod editor` in `src/components/mod.rs`
  - [ ] Write tests: line number rendering, gutter width adaptation
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task 2: Syntax highlighting in editor viewport
  - [ ] Reuse `App.syntax_set` and `App.syntax_theme` for highlighting
  - [ ] Highlight only visible lines (viewport range based on scroll_offset + height)
  - [ ] Re-highlight after each buffer mutation (visible lines only for performance)
  - [ ] Write tests: highlighted output contains styled spans
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task 3: Cursor rendering and dirty indicator
  - [ ] Render cursor position with reversed style (or underscore) at `(cursor_line, cursor_col)`
  - [ ] Panel title in Edit mode: `" Edit: {filename} ●"` if modified, `" Edit: {filename}"` if clean
  - [ ] Differentiate edit panel border style from view mode (e.g., different border color)
  - [ ] Write tests: dirty indicator presence, title formatting
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task 4: UI layout integration
  - [ ] In `ui.rs`: when `AppMode::Edit`, render `EditorWidget` instead of `PreviewWidget`
  - [ ] Pass `EditorState` and theme to `EditorWidget`
  - [ ] Ensure preview area rect is used for editor area (same layout split)
  - [ ] Store visible height on editor state for scroll/page calculations
  - [ ] Write integration test: edit mode renders editor widget, view mode renders preview
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task: Conductor - User Manual Verification 'Editor Widget & Rendering' (Protocol in workflow.md)

## Phase 4: Undo/Redo & Editor Clipboard

- [ ] Task 1: Undo stack with action grouping
  - [ ] Create `EditorAction` enum: `InsertChar`, `DeleteChar`, `InsertLine`, `DeleteLine`, `JoinLine`, `SplitLine`
  - [ ] Add `undo_stack: Vec<EditorAction>` and `undo_index: usize` to `EditorState`
  - [ ] Record each buffer mutation as an `EditorAction`
  - [ ] Group consecutive character inserts/deletes within 500ms into compound actions
  - [ ] `Ctrl+Z`: pop from undo stack, apply reverse action
  - [ ] Cap undo stack at 1000 entries
  - [ ] Write tests: undo single char, undo grouped chars, undo newline insert
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task 2: Redo support
  - [ ] `Ctrl+Y`: reapply action from redo portion of stack
  - [ ] Clear redo stack on any new edit action (standard redo behavior)
  - [ ] Write tests: redo after undo, redo stack cleared on new edit
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task 3: Editor clipboard (copy/cut/paste line)
  - [ ] Add `editor_clipboard: Vec<String>` to `EditorState`
  - [ ] `Ctrl+C`: copy current line to editor clipboard
  - [ ] `Ctrl+X`: cut current line (copy + delete line from buffer)
  - [ ] `Ctrl+V`: paste clipboard lines at cursor position
  - [ ] Separate from file-manager clipboard (no conflict with tree panel copy/paste)
  - [ ] Write tests: copy+paste, cut+paste, paste multiple times
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task: Conductor - User Manual Verification 'Undo/Redo & Editor Clipboard' (Protocol in workflow.md)

## Phase 5: Auto-indent & Tab Control

- [ ] Task 1: Auto-indent on Enter
  - [ ] On Enter (split line): detect leading whitespace of current line
  - [ ] Prepend same whitespace to the new line
  - [ ] Place cursor after the indentation on the new line
  - [ ] Preserve tab vs space indentation style from the source line
  - [ ] Write tests: auto-indent with spaces, tabs, mixed, empty line
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task 2: Tab insert and Shift+Tab dedent
  - [ ] `Tab`: insert indent unit at cursor position (detect file indent: 4 spaces default, or tab char)
  - [ ] `Shift+Tab`: remove one indent level from beginning of current line
  - [ ] Handle partial indent removal (e.g., 2 spaces when indent is 4)
  - [ ] Record as undo-able actions
  - [ ] Write tests: tab inserts spaces, shift-tab removes indent, edge cases
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task: Conductor - User Manual Verification 'Auto-indent & Tab Control' (Protocol in workflow.md)

## Phase 6: Find & Replace

- [ ] Task 1: Find bar with match highlighting
  - [ ] Add `EditorFind` state: `query: String`, `matches: Vec<(usize, usize)>`, `current_match: usize`, `active: bool`
  - [ ] `Ctrl+F` in editor: open find bar (rendered at top of editor area)
  - [ ] Search buffer for occurrences as user types (incremental search)
  - [ ] Highlight all matches in the viewport with a distinct background style
  - [ ] Highlight current match with a different style
  - [ ] `Esc` in find bar: close find bar and return to editor
  - [ ] Write tests: find matches, incremental search, no matches
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task 2: Next/previous match navigation
  - [ ] `Enter` in find bar: jump to next match, scroll viewport to show it
  - [ ] `Shift+Enter` in find bar: jump to previous match
  - [ ] Wrap around when reaching end/beginning of buffer
  - [ ] Show match count in find bar: "N of M matches"
  - [ ] Write tests: next/prev navigation, wraparound
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task 3: Replace bar and replace all
  - [ ] `Ctrl+H` in editor: open find + replace bar (two input fields)
  - [ ] Tab to switch between find and replace input fields
  - [ ] `Enter` on replace field: replace current match, jump to next
  - [ ] `Ctrl+A` in replace mode: replace all matches, show count in status bar
  - [ ] Record replacements as undoable actions
  - [ ] `Esc`: close replace bar
  - [ ] Write tests: single replace, replace all, undo replace
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task: Conductor - User Manual Verification 'Find & Replace' (Protocol in workflow.md)

## Phase 7: Integration, Guards & Polish

- [ ] Task 1: Large file warning dialog
  - [ ] When `e` pressed on a file with `is_large_file == true`: show confirmation dialog
  - [ ] Add `DialogKind::LargeFileEditConfirm` variant
  - [ ] On confirm: load full file content (bypass head+tail), enter Edit mode
  - [ ] On cancel: stay in View mode
  - [ ] Write tests: large file dialog shown, confirm enters edit, cancel stays in view
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task 2: File watcher pause during editing
  - [ ] On entering Edit mode: pause watcher for the edited file's parent directory
  - [ ] On exiting Edit mode: resume watcher
  - [ ] Prevent buffer conflicts from external file changes during editing
  - [ ] Write tests: watcher paused/resumed on mode transitions
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task 3: Help overlay and documentation update
  - [ ] Add Edit mode keybindings section to `HelpOverlay` data
  - [ ] Update `README.md` with Edit mode documentation
  - [ ] Update `PLAN.md` if applicable
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task 4: Final integration testing
  - [ ] Manual testing: full edit workflow (open, edit, save, exit)
  - [ ] Verify no regressions to View mode preview
  - [ ] Verify binary/directory guards work
  - [ ] Verify file watcher integration
  - [ ] Verify terminal panel coexistence
  - [ ] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

- [ ] Task: Conductor - User Manual Verification 'Integration, Guards & Polish' (Protocol in workflow.md)
