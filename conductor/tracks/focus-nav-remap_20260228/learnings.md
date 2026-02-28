# Track Learnings: focus-nav-remap_20260228

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Handler uses 3-level dispatch: global keys → panel-specific keys (handle_tree_keys/handle_preview_keys) → dialog keys (from: preview-panel_20260227)
- Terminal input must be routed BEFORE general global keys in handler — `q` should type 'q' in terminal, not quit the app (from: terminal-panel_20260228)
- Tab must be forwarded to PTY for shell autocompletion — do NOT intercept it for focus cycling when terminal is focused (from: terminal-panel_20260228)
- Static const arrays of structs for keybinding data — compile-time, zero allocation at runtime (from: config-polish_20260228)

---

<!-- Learnings from implementation will be appended below -->
