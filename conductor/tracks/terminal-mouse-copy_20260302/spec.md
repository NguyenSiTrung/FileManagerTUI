# Spec: Terminal Panel Mouse Actions and Copy

## Overview

Add first-class mouse interaction in the embedded terminal panel, including text selection and copy-to-system-clipboard. Users should be able to select terminal output with the mouse and copy it without leaving FileManagerTUI.

## Functional Requirements

### FR-1: Terminal Mouse Interaction
- Mouse events inside the terminal panel must be handled by terminal-specific logic when `FocusedPanel::Terminal` is active.
- Left click inside the terminal panel sets terminal focus and places/clears selection anchor.
- Mouse wheel inside the terminal panel scrolls terminal scrollback in both directions.
- Mouse drag with left button updates terminal text selection bounds.

### FR-2: Terminal Text Selection Model
- Terminal state must track selection anchor and active cursor endpoint using terminal-local coordinates.
- Selection must support forward and backward drags (normalize start/end when extracting text).
- A click without drag clears any previous terminal selection.
- Pressing `Esc` while terminal is focused clears terminal selection before changing focus.

### FR-3: Copy Terminal Selection
- Add a copy action for terminal selection using `Ctrl+Shift+C` while terminal is focused.
- Copied content must use the existing system clipboard helper (`copy_to_system_clipboard`).
- If no selection exists, show a non-error status hint instead of failing silently.
- On successful copy, show a status message confirming bytes/lines copied.

### FR-4: Selection Rendering
- Selected terminal cells must render with a distinct highlight style that works in both light and dark themes.
- Selection rendering must work with scrollback offsets (visible viewport is not always live bottom).
- Cursor rendering remains visible and does not corrupt highlighted cells.

### FR-5: Help/Keybinding Discoverability
- Help overlay terminal keybindings must document terminal copy and mouse selection behavior.
- Existing terminal shortcuts (`Esc`, scroll, focus controls) must continue to behave as before.

### FR-6: Compatibility and Safety
- Existing tree/preview mouse behavior must remain unchanged.
- Terminal copy must degrade gracefully in headless environments where clipboard tools are unavailable.
- No regressions to terminal key forwarding and PTY lifecycle.

## Non-Functional Requirements

### NFR-1: Performance
- Mouse selection updates must not block rendering or PTY input.
- Selection state updates should be O(1) per event, and extraction should be linear in selected region.

### NFR-2: Reliability
- Selection and copy logic must be deterministic and covered by unit tests for edge cases.
- Clipboard failures must surface actionable status messages.

## Acceptance Criteria

1. Users can drag-select text in the terminal panel with the mouse.
2. `Ctrl+Shift+C` copies selected terminal text to the system clipboard.
3. Selection highlight is visible and stable while scrolling terminal scrollback.
4. `Esc` clears selection and keeps current terminal focus behavior intact.
5. Tree and preview mouse actions still work exactly as before.
6. `cargo test` includes coverage for terminal mouse selection + copy paths.

## Out of Scope

- OSC52 clipboard protocol passthrough
- Multi-cursor or block selection in terminal
- Persisting terminal selections across app restart
- Full mouse reporting passthrough for terminal apps (future enhancement)
