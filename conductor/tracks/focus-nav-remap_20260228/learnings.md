# Track Learnings: focus-nav-remap_20260228

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Handler uses 3-level dispatch: global keys → panel-specific keys (handle_tree_keys/handle_preview_keys) → dialog keys (from: preview-panel_20260227)
- Terminal input must be routed BEFORE general global keys in handler — `q` should type 'q' in terminal, not quit the app (from: terminal-panel_20260228)
- Tab must be forwarded to PTY for shell autocompletion — do NOT intercept it for focus cycling when terminal is focused (from: terminal-panel_20260228)
- Static const arrays of structs for keybinding data — compile-time, zero allocation at runtime (from: config-polish_20260228)

---

<!-- Learnings from implementation will be appended below -->

## [2026-02-28 14:10] - Phase 1 Tasks 1-3: Directional focus + resize remap + help
- **Implemented:** Added `focus_left/right/up/down` methods to App; remapped Ctrl+Arrow to directional focus and Ctrl+Shift+Arrow to terminal resize in handler; updated help overlay entries
- **Files changed:** `src/app.rs`, `src/handler.rs`, `src/components/help.rs`
- **Commit:** 96dcb6e
- **Learnings:**
  - Patterns: Modifier checks require `contains()` with explicit `!contains(SHIFT)` to distinguish Ctrl+Arrow from Ctrl+Shift+Arrow — crossterm CONTROL|SHIFT is a combined bitflag
  - Gotchas: All Ctrl+Arrow and Ctrl+Shift+Arrow must be intercepted BEFORE the terminal key forwarding check in `handle_normal_mode`, otherwise they get forwarded as PTY input
  - Context: The reserved-keys block at the top of `handle_normal_mode` is the correct place for global intercepts since it runs before the terminal focus check
---
