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

## [2026-02-27] - Phase 1-4: Full clipboard & undo implementation
- **Implemented:** ClipboardState (fs/clipboard.rs), multi-select (Space/Esc), y/x/p keys, async paste with progress dialog, single-level undo (Ctrl+Z)
- **Files changed:** fs/clipboard.rs (new), fs/operations.rs, fs/mod.rs, app.rs, handler.rs, event.rs, main.rs, components/tree.rs, components/status_bar.rs, components/dialog.rs
- **Learnings:**
  - Patterns: handler tests need dummy mpsc sender when handler signature includes event_tx — use `handle_key()` wrapper
  - Patterns: async paste via tokio::spawn + mpsc events (Progress, OperationComplete) integrates cleanly with existing event loop
  - Patterns: AtomicBool is sufficient for cancel tokens (no need for tokio_util CancellationToken)
  - Gotchas: `tokio_util` was NOT in deps — used Arc<AtomicBool> instead
  - Gotchas: paste tests must be #[tokio::test] async since paste_clipboard_async uses tokio::spawn
  - Gotchas: flatten() must clear multi_selected since indices change
  - Context: DialogKind::Progress added with {message, current, total} fields for progress dialog
---
