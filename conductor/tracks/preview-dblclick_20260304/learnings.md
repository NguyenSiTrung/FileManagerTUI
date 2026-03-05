# Track Learnings: preview-dblclick_20260304

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Preview view-mode mouse selection reuses the same coordinate-mapping and selection-rendering patterns as terminal and editor panels (from: terminal-mouse-copy_20260302)
- Mouse events only processed in Normal mode — prevents accidental clicks during dialogs (from: config-polish_20260228)
- Must account for border offset (y+1) when mapping mouse click row to flat_items index in bordered widgets (from: config-polish_20260228)
- Store layout `Rect` on App from render → handler uses them for mouse coordinate mapping (from: config-polish_20260228)
- `mouse_to_preview_coord` handles scroll_offset and content_lines bounds automatically (from: terminal-mouse-copy_20260302)

---

## [2026-03-05 08:55] - Phase 1 Task 1+2: Double-click detection state and logic
- **Implemented:** Added `last_preview_click: Option<(Instant, u16, u16)>` to App struct and double-click detection logic in `handle_mouse_event` for the preview panel
- **Files changed:** src/app.rs, src/handler.rs
- **Commit:** 4c1d80d
- **Learnings:**
  - Patterns: Use `Option<(Instant, u16, u16)>` with `.take()` for one-shot state consumption — `.take()` atomically reads and clears in a single step, preventing stale state
  - Patterns: Line content length from `Line<'static>` spans: `l.spans.iter().map(|s| s.content.len()).sum::<usize>()` — accounts for multi-span syntax-highlighted content
  - Gotchas: Double-click detection must use screen coordinates (col, row) not content coordinates — screen coords are stable across scrolls while content coords shift
  - Gotchas: After double-click selection, `last_preview_click` must be set to `None` (consumed) to prevent triple-click from being treated as another double-click
  - Context: `set_anchor` + `set_endpoint` (without `begin_drag`) creates a non-dragging selection — useful for programmatic selections that shouldn't interfere with mouse-up clearing logic

---

## [2026-03-05 08:55] - Phase 2 Task 1+2: Integration tests and CI
- **Implemented:** Added 4 integration tests for scroll offset, selection persistence, single-click clear, and tree isolation
- **Files changed:** src/handler.rs
- **Commit:** 23d77d6
- **Learnings:**
  - Patterns: Test helper `setup_app_with_preview()` sets fake `preview_area` Rect and synthetic `content_lines` — reusable for any preview mouse interaction test
  - Patterns: Simulate scroll via `MouseEvent { kind: MouseEventKind::ScrollDown, ... }` to `handle_mouse_event` — no need for separate scroll functions in tests
  - Context: `MouseUp` at same position as `MouseDown` with no drag triggers the anchor==endpoint auto-clear — important for test flow (click+release+click pattern for double-click)
