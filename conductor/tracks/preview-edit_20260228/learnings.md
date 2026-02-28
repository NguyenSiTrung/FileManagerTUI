# Track Learnings: preview-edit_20260228

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Handler uses 3-level dispatch: global keys → panel-specific keys (handle_tree_keys/handle_preview_keys) → dialog keys
- PreviewWidget follows builder pattern: `WidgetName::new(state, theme).block(block)`
- Store SyntaxSet and Theme on App struct (expensive to load, reuse across previews)
- Use `last_previewed_index` to avoid re-loading preview on every render frame
- Binary detection: check known extensions first (fast), then null-byte scan in 8KB (fallback)
- `AppMode` can no longer derive `Copy` once DialogKind contains heap types (PathBuf, Vec, String)
- Must clone DialogKind before matching to avoid borrow conflicts with `app`
- Clone ThemeColors at render start to avoid borrow checker conflicts with `app` mutation during rendering
- Terminal input must be routed BEFORE general global keys in handler — 'q' should type 'q' in terminal, not quit the app
- Reserved-keys block at the top of `handle_normal_mode` is the correct place for global intercepts
- Every `load_children()` call MUST be followed by `sort_children_of()` — canonical pattern
- VTE `Performer` struct must be separated from owner to avoid borrow checker issues

---

<!-- Learnings from implementation will be appended below -->
