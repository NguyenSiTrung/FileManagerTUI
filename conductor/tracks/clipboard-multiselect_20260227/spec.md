# Spec: Copy / Cut / Paste + Multi-Select

## Overview

Add clipboard operations (copy, cut, paste) and multi-select capability to the file manager, enabling users to move and copy files/directories within the tree. Includes async file operations with a progress dialog, visual indicators for selected/clipboard items, and single-level undo for reversible operations.

## Functional Requirements

### 1. Clipboard Buffer (`fs/clipboard.rs`)
- Internal clipboard buffer holding a list of file paths and an operation type (Copy or Cut).
- `y` key copies selected/multi-selected items to clipboard.
- `x` key cuts selected/multi-selected items to clipboard.
- Clipboard persists until overwritten by a new copy/cut or cleared explicitly.
- If no items are multi-selected, operate on the currently focused item.

### 2. Multi-Select (`Space` toggle, persistent)
- `Space` toggles the currently focused item in/out of the multi-selection set.
- Navigating with `j`/`k` does NOT clear the selection.
- `Esc` clears all multi-selections (when not in a dialog/search mode).
- Visual indicator: multi-selected items shown with a distinct highlight/marker in the tree.
- Multi-selected items are used as targets for `y`, `x`, and `d` (delete) operations.

### 3. File Operations (`fs/operations.rs`)
- `copy_recursive(src, dest)` — recursively copy files and directories.
- `move_item(src, dest)` — move (rename across directories) files and directories.
- Handle name collisions: if destination exists, append `_copy` suffix (or `_copy2`, etc.).
- Operations are performed asynchronously to avoid freezing the UI.

### 4. Paste Operation
- `p` key pastes clipboard contents into the directory of the currently focused item (or the focused directory itself).
- If clipboard operation is Copy: `copy_recursive` each item.
- If clipboard operation is Cut: `move_item` each item, then clear clipboard on success.
- After paste, refresh the affected directory subtrees and re-flatten.

### 5. Async Operations with Progress Dialog
- Copy/move operations run asynchronously via tokio tasks.
- A modal progress dialog is shown during operations:
  - Displays current file being processed and progress (e.g., "Copying 2/5 files...").
  - Provides a cancel option (Esc or Cancel button) to abort remaining operations.
- On completion: dismiss dialog, show success message in status bar.
- On error: show error in dialog, allow user to acknowledge.

### 6. Status Bar Clipboard State
- When clipboard is non-empty, status bar shows clipboard info: e.g., "📋 N items copied" or "✂ N items cut".
- Cleared when clipboard is emptied.

### 7. Single-Level Undo (Reversible Operations Only)
- Undo (`Ctrl+Z`) reverts the last reversible operation.
- Reversible operations: rename, copy (paste from copy), move (paste from cut).
- NOT reversible (excluded): delete (permanent with confirmation dialog).
- Undo for rename: rename back to original name.
- Undo for copy-paste: delete the copied files.
- Undo for move-paste: move files back to original locations.
- Only one level of undo is stored (last operation overwrites previous).

## Non-Functional Requirements
- Async operations must not block the UI event loop.
- Copy/move of large files (multi-GB model files) must stream data, not load into memory.
- Operations on 100+ files should show progress, not appear frozen.

## Acceptance Criteria
- [ ] Can select multiple files with `Space`, visual indicator shown.
- [ ] `y` copies selected items to clipboard, status bar reflects state.
- [ ] `x` cuts selected items to clipboard, status bar reflects state.
- [ ] `p` pastes clipboard contents into current directory.
- [ ] Copy/move of a directory works recursively.
- [ ] Progress dialog shown during multi-file operations with cancel support.
- [ ] Name collisions are handled automatically (append suffix).
- [ ] `Ctrl+Z` undoes last rename, copy-paste, or move-paste.
- [ ] `Esc` clears multi-selection in normal mode.
- [ ] All operations update the tree view correctly after completion.

## Out of Scope
- System clipboard integration (OS copy/paste).
- Multi-level undo history.
- Trash/recycle bin for delete operations.
- Drag-and-drop (mouse-based move).
- Cross-instance clipboard (between multiple fm processes).
