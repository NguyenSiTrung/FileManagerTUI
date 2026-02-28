# Spec: Preview Panel Edit Mode

## Overview

Add a full-featured text editing mode to the preview panel, transforming it from a read-only
viewer into a capable editor. Users toggle between View mode (existing behavior) and Edit mode
using the `e` key when the preview panel is focused. Edit mode provides syntax-highlighted
editing with line numbers, undo/redo, find & replace, clipboard operations, auto-indent, and
indent/dedent — making the file manager self-sufficient for quick file edits without leaving
the TUI.

## Functional Requirements

### FR-1: Mode Toggle
- Press `e` when preview panel is focused and a text file is selected to enter Edit mode.
- Press `Esc` to exit Edit mode:
  - If buffer is **unmodified**: return to View mode immediately.
  - If buffer is **modified**: show a confirmation dialog — "Save changes? (Y)es / (N)o / (C)ancel".
    - `Y`: save to disk, return to View mode.
    - `N`: discard changes, return to View mode.
    - `C` / `Esc`: stay in Edit mode.
- Binary files: show status message "Cannot edit binary files" and stay in View mode.
- Large files (`is_large_file` == true): show confirmation dialog "This is a large file, editing
  may be slow. Continue? (y/n)" before entering Edit mode. If confirmed, load full file content.
- Directories: `e` is a no-op (cannot edit directories).

### FR-2: Editor Buffer
- On entering Edit mode, load the raw file content into an in-memory buffer (`Vec<String>` of lines).
- Track cursor position as (line, column) — 0-indexed internally, displayed as 1-indexed.
- Track a `modified` (dirty) flag — set to `true` on any buffer mutation, reset on save.
- Buffer operations: insert char, delete char (Backspace/Delete), insert newline (Enter),
  join lines (Backspace at line start / Delete at line end).

### FR-3: Navigation
- Arrow keys: move cursor up/down/left/right.
- `Home` / `End`: jump to start/end of current line.
- `Ctrl+Home` / `Ctrl+End`: jump to first/last line.
- `Page Up` / `Page Down`: scroll by visible height.
- Cursor stays within valid buffer bounds at all times (clamp column to line length).

### FR-4: Saving
- `Ctrl+S`: write buffer contents to disk, reset `modified` flag, show status message "File saved".
- Handle write errors gracefully (show error in status bar, keep buffer intact).
- After save, trigger preview reload to keep tree watcher state consistent.

### FR-5: Line Numbers
- Display line numbers in a gutter on the left side of the editing area.
- Gutter width adapts to the number of digits in total line count.
- Current line number highlighted with a different style.

### FR-6: Syntax Highlighting
- Reuse existing `SyntaxSet` and `Theme` from App state.
- Re-highlight visible lines after each edit operation.
- Performance: only re-highlight the visible viewport, not the entire file.

### FR-7: Undo/Redo
- `Ctrl+Z`: undo the last edit operation.
- `Ctrl+Y`: redo the last undone operation.
- Undo stack stores individual edit actions (insert char, delete char, insert line, delete line).
- Group rapid consecutive character inserts/deletes into single undo entries (debounce grouping).
- Undo stack is cleared when exiting Edit mode.

### FR-8: Find & Replace
- `Ctrl+F`: open find bar at the top of the editor area.
  - Type query, press `Enter` / `Shift+Enter` to jump to next/previous match.
  - Matches highlighted in the buffer.
  - `Esc` closes find bar.
- `Ctrl+H`: open find & replace bar.
  - Two input fields: find pattern, replacement string.
  - `Enter` on replacement field: replace current match and jump to next.
  - `Ctrl+A` in replace mode: replace all matches (with confirmation count).
  - `Esc` closes replace bar.

### FR-9: Editor Clipboard
- `Ctrl+C`: copy current line (or selection if selection support is added later).
- `Ctrl+X`: cut current line (remove from buffer, add to editor clipboard).
- `Ctrl+V`: paste editor clipboard content at cursor position.
- Editor clipboard is separate from file manager clipboard to avoid conflicts.

### FR-10: Dirty Indicator
- When buffer is modified, show `[modified]` or `●` in the preview panel title/border.
- Panel title format in Edit mode: `" Edit: filename.ext ●"` (modified) or `" Edit: filename.ext"` (clean).

### FR-11: Auto-indent
- On `Enter` (new line): copy leading whitespace from the current line to the new line.
- Preserve existing indentation character (spaces or tabs) from the file.

### FR-12: Tab Indent/Dedent
- `Tab`: insert indentation at cursor position (respect file's existing indent style, default 4 spaces).
- `Shift+Tab`: remove one level of indentation from the current line's beginning.

## Non-Functional Requirements

### NFR-1: Performance
- Editor must remain responsive for files up to 10,000 lines.
- Syntax highlighting is viewport-only — do not re-highlight the entire buffer on each keystroke.
- Undo stack capped at 1,000 entries to bound memory.

### NFR-2: Integration
- Edit mode must coexist cleanly with existing View mode — no regressions to preview scrolling,
  head+tail mode, or binary metadata display.
- File watcher should pause auto-refresh for the edited file to prevent buffer conflicts.
- After exiting Edit mode, preview state should reflect the saved content.

### NFR-3: Architecture
- Editor state (`EditorState`) is a new struct on `App`, similar to `PreviewState`.
- Editor widget (`EditorWidget`) follows the existing widget builder pattern.
- Handler dispatch adds `handle_editor_keys` alongside `handle_preview_keys`.
- Edit mode is represented as a new variant in `AppMode` enum or as a sub-state of preview focus.

## Acceptance Criteria

1. User can press `e` on a text file in preview to enter Edit mode with cursor blinking.
2. User can type, delete, navigate, and see syntax-highlighted content with line numbers.
3. `Ctrl+S` saves the file and shows confirmation in the status bar.
4. `Esc` on modified buffer shows save confirmation dialog (Y/N/C).
5. `Ctrl+Z` / `Ctrl+Y` undo/redo works across multiple edit operations.
6. `Ctrl+F` opens find with match highlighting; `Ctrl+H` adds replace functionality.
7. `Ctrl+C/X/V` copy/cut/paste lines within the editor.
8. Tab/Shift+Tab indent/dedent lines correctly.
9. Auto-indent preserves whitespace on new lines.
10. Binary files and directories cannot enter Edit mode.
11. Large files show warning before entering Edit mode.
12. Panel title shows "Edit: filename ●" when modified.
13. All existing preview View mode functionality is unaffected.
14. `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` all pass.

## Out of Scope

- Multi-file editing / tabs
- Mouse-based text selection in editor
- External editor integration (`$EDITOR`)
- Syntax-aware auto-completion
- Multi-cursor editing
- Line wrapping toggle in edit mode (always no-wrap for editing)
- Git diff indicators in gutter
