# Track Learnings: preview-dblclick_20260304

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Preview view-mode mouse selection reuses the same coordinate-mapping and selection-rendering patterns as terminal and editor panels (from: terminal-mouse-copy_20260302)
- Mouse events only processed in Normal mode — prevents accidental clicks during dialogs (from: config-polish_20260228)
- Must account for border offset (y+1) when mapping mouse click row to flat_items index in bordered widgets (from: config-polish_20260228)
- Store layout `Rect` on App from render → handler uses them for mouse coordinate mapping (from: config-polish_20260228)
- `mouse_to_preview_coord` handles scroll_offset and content_lines bounds automatically (from: terminal-mouse-copy_20260302)

---

<!-- Learnings from implementation will be appended below -->
