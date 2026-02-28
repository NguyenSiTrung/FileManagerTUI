# Track Learnings: file-watcher_20260228

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- crossterm event polling is blocking — must run in spawned tokio task with mpsc channel
- `flatten()` must clear `multi_selected` since flat indices change on re-flatten
- `invalidate_search_cache()` must be called after ALL tree mutations (create, rename, delete, expand, toggle_hidden, paste)
- `TreeState::reload_dir()` reloads a specific directory's children and re-flattens after file ops
- Use `last_previewed_index` to avoid re-loading preview on every render frame
- Handler uses 3-level dispatch: global keys → panel-specific keys (handle_tree_keys/handle_preview_keys) → dialog keys
- Async paste via `tokio::spawn` + `mpsc::unbounded_channel` events (Progress, OperationComplete) integrates with the existing event loop
- Use `Arc<AtomicBool>` for cancel tokens — no need for `tokio_util::CancellationToken`
- Root node must always bypass hidden filter in flatten — tempfile and some paths start with `.` prefix

---

<!-- Learnings from implementation will be appended below -->
