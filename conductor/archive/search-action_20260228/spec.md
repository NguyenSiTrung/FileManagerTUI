# Search Action Menu

## Overview
After selecting a file from the fuzzy search overlay (Ctrl+P), display a context-aware
action menu that lets the user choose what to do with the selected file, instead of
always navigating to it in the tree.

## Functional Requirements

### FR-1: Two-Step Search Flow
- When the user presses Enter on a search result, transition from the search overlay
  to an action menu overlay (replacing the search view in the same area).
- The action menu displays the selected file's path prominently at the top.
- Pressing Esc in the action menu returns to the search overlay with query and results
  preserved.

### FR-2: Available Actions
The action menu presents single-key shortcuts for the following actions:

| Key     | Action              | Description                                       |
|---------|---------------------|---------------------------------------------------|
| `Enter` | Navigate (Go to)    | Jump to file in tree (original behavior)          |
| `p`     | Preview             | Navigate to file + focus preview panel            |
| `e`     | Edit                | Navigate to file + open inline editor             |
| `y`     | Copy path           | Copy absolute path to system clipboard / status   |
| `r`     | Rename              | Navigate to file + open rename dialog             |
| `d`     | Delete              | Navigate to file + open delete confirmation       |
| `c`     | Copy (clipboard)    | Add file to internal clipboard for paste           |
| `x`     | Cut (clipboard)     | Cut file to internal clipboard for move            |
| `t`     | Open in terminal    | cd to file's parent directory in embedded terminal |

### FR-3: Context-Aware Action Filtering
- **Directories**: Hide "Edit" and "Preview" (they don't apply to directories).
- **Binary files**: Hide "Edit" (binary files cannot be edited).
- **All file types**: "Navigate", "Copy path", "Rename", "Delete", "Copy", "Cut",
  and "Open in terminal" are always available.

### FR-4: Action Execution
- After executing an action, return to Normal mode (close both search and action menu).
- Display a status message confirming the action (e.g., "📋 Path copied: /path/to/file").
- For "Open in terminal": if terminal is hidden, open it first, then send
  `cd <parent_dir>\n` to the PTY.

### FR-5: New AppMode
- Add `AppMode::SearchAction` to the mode enum.
- Store the selected `SearchResult` (path, display name) in a new `SearchActionState`
  struct on `App`.

## Non-Functional Requirements

### NFR-1: Performance
- The action menu must appear instantly (no I/O on open — file type detection uses
  cached data from search results or quick extension check).

### NFR-2: Visual Consistency
- Match existing overlay styling (border color, padding, Clear + centered Block pattern).
- Use the theme system (`ThemeColors`) for all colors.

### NFR-3: Accessibility
- Every action shows its keybinding inline.
- Selected/highlighted action follows cursor (Up/Down navigation optional but not required
  since single keys are sufficient).

## Acceptance Criteria
1. Pressing `Ctrl+P`, typing a query, selecting a result, and pressing Enter opens the
   action menu instead of navigating directly.
2. Each action key executes the correct operation and returns to Normal mode.
3. Binary files do not show "Edit" in the action menu.
4. Directories do not show "Edit" or "Preview" in the action menu.
5. Esc in the action menu returns to the search overlay with query preserved.
6. "Open in terminal" opens the terminal panel if hidden and sends `cd` command.
7. All actions display appropriate status messages.
8. Unit tests cover action dispatch and context-aware filtering.

## Out of Scope
- System clipboard integration for "Copy path" (uses status bar display only for now).
- Custom/configurable action list.
- Mouse interaction with the action menu.
