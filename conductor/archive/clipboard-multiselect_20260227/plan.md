# Plan: Copy / Cut / Paste + Multi-Select

## Phase 1: Clipboard Buffer & Multi-Select Foundation

- [ ] Task 1: Create `fs/clipboard.rs` — ClipboardState struct (operation type enum, paths vec, clear/set methods) with unit tests
- [ ] Task 2: Integrate ClipboardState into App struct — add field, wire up in `app.rs`
- [ ] Task 3: Implement multi-select toggle — `Space` key in handler, toggle index in `TreeState.multi_selected`, `Esc` clears selection in normal mode
- [ ] Task 4: Visual indicator for multi-selected items in `components/tree.rs` — distinct style/marker for selected rows
- [ ] Task 5: Wire `y` (copy) and `x` (cut) keys — populate clipboard from multi-selected or focused item, with unit tests
- [ ] Task 6: Show clipboard state in status bar — "📋 N items copied" / "✂ N items cut" in `components/status_bar.rs`
- [ ] Task: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

## Phase 2: File Copy & Move Operations

- [ ] Task 1: Implement `copy_recursive(src, dest)` in `fs/operations.rs` — recursive directory copy with streaming, name collision handling (`_copy` suffix), unit tests with tempdir
- [ ] Task 2: Implement `move_item(src, dest)` in `fs/operations.rs` — rename-based move with cross-device fallback (copy+delete), name collision handling, unit tests
- [ ] Task 3: Implement paste action in `app.rs` — `p` key triggers paste into current directory, delegates to copy_recursive or move_item, refreshes tree
- [ ] Task: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)

## Phase 3: Async Operations & Progress Dialog

- [ ] Task 1: Create async operation infrastructure — tokio task spawning for file ops, mpsc channel for progress updates back to App event loop
- [ ] Task 2: Create progress dialog widget in `components/dialog.rs` — modal showing "Copying X/Y files...", current filename, cancel support (Esc)
- [ ] Task 3: Add `AppMode::Dialog(DialogKind::Progress)` — state transitions for progress dialog, handle cancel event, completion/error handling
- [ ] Task 4: Wire paste operation through async pipeline — paste spawns async task, sends progress events, dialog updates on each event, dismisses on completion
- [ ] Task: Conductor - User Manual Verification 'Phase 3' (Protocol in workflow.md)

## Phase 4: Undo System

- [ ] Task 1: Create UndoAction enum and UndoState in `app.rs` — store last reversible operation (Rename{from,to}, CopyPaste{created_paths}, MovePaste{moves: Vec<(from,to)>})
- [ ] Task 2: Implement undo logic — `Ctrl+Z` handler: rename-back, delete-copied, move-back, with error handling
- [ ] Task 3: Record undo state on rename, copy-paste, move-paste operations — update undo buffer after each successful operation
- [ ] Task 4: Unit tests for undo — test rename undo, copy-paste undo, move-paste undo, verify single-level (overwrites previous)
- [ ] Task: Conductor - User Manual Verification 'Phase 4' (Protocol in workflow.md)
