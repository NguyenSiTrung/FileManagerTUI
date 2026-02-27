# Spec: File Operations + Dialogs

## Overview

Add file operation capabilities (create file, create directory, rename, delete) with a modal dialog system for user input and confirmation, plus a status bar for feedback messages. This builds on the existing tree navigation from Milestone 1.

## Functional Requirements

### Dialog System (`components/dialog.rs`)
- **Input Dialog**: Centered modal overlay for text input (create file, create dir, rename)
  - Full inline editing: cursor left/right, Home/End, character insertion at cursor position, backspace/delete
  - Title reflects the operation (e.g., "Create New File", "Rename")
  - For rename: pre-populate input with the current name
  - Enter to confirm, Esc to cancel
- **Confirmation Dialog**: Centered modal overlay for delete confirmation
  - Lists target items to be deleted
  - `y` to confirm, `n`/Esc to cancel
- **Error Dialog**: Display filesystem errors with a dismiss action (Enter/Esc)
- Dialogs render as `Clear` + `Block` overlay in the center of the terminal

### File Operations (`fs/operations.rs`)
- `create_file(path)` — create an empty file
- `create_dir(path)` — create a new directory
- `rename(from, to)` — rename a file or directory
- `delete(path)` — delete a file, or recursively delete a non-empty directory
- All operations return `Result` using the existing `AppError` type

### App State Transitions (`app.rs`)
- Add `AppMode::Dialog(DialogKind)` variant
- `DialogKind`: `CreateFile`, `CreateDirectory`, `Rename { original: PathBuf }`, `DeleteConfirm { targets: Vec<PathBuf> }`, `Error { message: String }`
- Track dialog input state: input string, cursor position

### Key Bindings (`handler.rs`)
- `a` → open CreateFile dialog (in Normal mode)
- `A` → open CreateDirectory dialog (in Normal mode)
- `r` → open Rename dialog pre-filled with selected item name (in Normal mode)
- `d` → open DeleteConfirm dialog for selected item (in Normal mode)
- In Dialog mode: character input, cursor movement, Enter/Esc, backspace/delete, Home/End

### Status Bar (`components/status_bar.rs`)
- Show success messages (e.g., "Created file: foo.txt") and error messages
- Auto-dismiss after 3 seconds
- Displayed at the bottom of the terminal

### Tree Refresh
- After any successful file operation, reload the affected directory's children and re-flatten the tree

## Non-Functional Requirements
- No `.unwrap()` in non-test code; all fs errors surfaced via error dialog or status bar
- Tests for all `fs/operations.rs` functions using `tempfile::TempDir`
- Dialog widget tests for rendering

## Acceptance Criteria
1. Press `a`, type a filename, press Enter → file is created in the currently selected directory; tree refreshes; status bar shows confirmation
2. Press `A`, type a directory name, press Enter → directory is created; tree refreshes
3. Press `r`, edit the pre-filled name, press Enter → item is renamed; tree refreshes
4. Press `d` on a file → confirmation dialog appears listing the file → press `y` → file is deleted; tree refreshes
5. Press `d` on a non-empty directory → confirmation dialog → press `y` → directory and contents recursively deleted
6. Pressing Esc in any dialog cancels the operation and returns to Normal mode
7. Filesystem errors (permission denied, etc.) display an error dialog or status bar message
8. Status messages auto-dismiss after 3 seconds

## Out of Scope
- Copy/cut/paste (Milestone 4)
- Multi-select delete (Milestone 4 — `d` operates on single selected item for now)
- Undo operations (Milestone 4)
- Fuzzy search (Milestone 5)
