# Track Learnings: clipboard-multiselect_20260227

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Handler uses 3-level dispatch: global keys → panel-specific keys → dialog keys
- TreeState owns root TreeNode + flat_items Vec + selected_index; `flatten()` rebuilds flat list
- App delegates tree operations to TreeState methods; handler.rs maps keys to App methods
- `TreeState::reload_dir()` reloads a specific directory's children and re-flattens after file ops
- Use `Clear` widget + centered `Block` for modal overlays in ratatui
- Must clone DialogKind before matching to avoid borrow conflicts with `app`
- crossterm event polling is blocking — must run in spawned tokio task with mpsc channel
- Use `tempfile::TempDir` for filesystem tests

---

<!-- Learnings from implementation will be appended below -->
