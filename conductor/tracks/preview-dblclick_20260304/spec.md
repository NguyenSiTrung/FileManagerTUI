# Spec: Double-Click Line Selection in Preview Panel

## Overview

Add double-click mouse support to the preview panel content area. When the user
double-clicks on a line, the entire line text is selected (highlighted) using the
existing `TerminalSelection` mechanism. This provides a quick way to select a full
line of content — similar to line-select behavior in editors like VS Code — without
requiring manual click-and-drag across the entire line width.

## Functional Requirements

1. **Double-click detection in preview panel:**
   - Detect a double-click (two `MouseEventKind::Down(Left)` events on the same
     position within a short time window, e.g. 300–500ms) in the preview panel
     area during `AppMode::Normal`.
   - The detection must be scoped to the preview panel — double-clicks on tree or
     terminal panels are unaffected.

2. **Line selection via existing `TerminalSelection`:**
   - On double-click, compute the clicked line index using the existing
     `mouse_to_preview_coord` function.
   - Set `app.preview_selection` anchor to `(line, 0)` and endpoint to
     `(line, line_length)` where `line_length` is the character count of that
     content line (from `app.preview_state.content_lines`).
   - This reuses the existing selection highlight rendering in `PreviewWidget::render`
     and the existing copy workflow (right-click / Ctrl+Shift+C).

3. **Visual feedback:**
   - The selected line is highlighted using the same selection background style
     already used by drag selections in the preview panel.
   - The highlight spans the full content width of the line.

4. **Interaction with existing behaviors:**
   - A subsequent single-click clears the double-click selection (existing behavior
     from `begin_drag`).
   - A subsequent drag replaces the double-click selection (standard editor behavior).
   - Right-click after double-click copies the selected line text (existing
     `copy_preview_selection` pathway).
   - Scrolling does not clear the selection (existing behavior).

## Non-Functional Requirements

- No new dependencies or crate additions.
- No new App state fields — only a last-click timestamp + position for double-click
  detection (minimal state, can live on App or be local to handler).
- Performance: double-click detection is O(1) — just a timestamp comparison.

## Acceptance Criteria

- [ ] Double-clicking a line in the preview panel selects the entire line visually.
- [ ] The selection can be copied via right-click or Ctrl+Shift+C.
- [ ] Single-click after double-click clears the full-line selection.
- [ ] Double-click outside the preview panel has no effect on preview selection.
- [ ] Works correctly with scrolled content (respects `scroll_offset`).
- [ ] No regressions in existing drag-to-select behavior.
- [ ] Tests cover double-click detection logic and line selection.

## Out of Scope

- Word-level selection (double-click selects word).
- Triple-click or multi-click patterns.
- Double-click in editor mode (Edit mode has its own mouse handler).
- Double-click in terminal panel.
