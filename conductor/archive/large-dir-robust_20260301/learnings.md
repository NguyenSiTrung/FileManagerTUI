# Track Learnings: large-dir-robust_20260301

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- `load_children()` reads immediate children only — lazy loading by design (from: core-loop-tree_20260227)
- Use iterative stack-based directory walk with entry cap (10K) to prevent hanging on huge trees (from: preview-panel_20260227)
- TreeState owns root TreeNode + flat_items Vec + selected_index; `flatten()` rebuilds flat list from tree recursively (from: core-loop-tree_20260227)
- `invalidate_search_cache()` must be called after ALL tree mutations (from: fuzzy-search_20260228)
- State preservation on tree refresh: capture (selected path, scroll, expanded set) → reload subtrees → restore_expanded → flatten → restore selection by path lookup → clamp scroll (from: file-watcher_20260228)
- `handle_fs_change()` deduplicates parent directories before reloading to avoid redundant I/O (from: file-watcher_20260228)
- All config fields use `Option<T>` so partial configs from different sources compose cleanly via `.or()` merge (from: config-polish_20260228)
- `#[serde(default)]` on both struct and fields ensures TOML parsing tolerates missing sections (from: config-polish_20260228)
- Every `load_children()` call MUST be followed by `sort_children_of()` — this is the canonical pattern (from: sort-order-fix_20260228)
- Root node must always bypass hidden filter in flatten — tempfile and some paths start with `.` prefix (from: core-loop-tree_20260227)
- Async paste via `tokio::spawn` + `mpsc::unbounded_channel` events (Progress, OperationComplete) integrates with the existing event loop (from: clipboard-multiselect_20260227)
- Use `Arc<AtomicBool>` for cancel tokens — no need for `tokio_util::CancellationToken` (from: clipboard-multiselect_20260227)
- `flatten()` must clear `multi_selected` since flat indices change on re-flatten (from: clipboard-multiselect_20260227)

## Patterns from Previous large-dir-perf Track (archived)

- `load_children_paged()` does TWO `read_dir()` calls: one for count, one to load — identified as the key bottleneck for 1M+ dirs
- `load_next_page()` is O(n²) because it iterates from start of `read_dir()` and uses HashSet dedup
- `handle_fs_change()` re-triggers full `load_children_paged()` on paginated directories — causes freezes
- `get_child_count()` blocks on `read_dir().count()` for uncounted directories
- `build_path_index()` walks unloaded directories synchronously on main thread
- `load_directory_summary()` is synchronous with 10K cap but still blocks on large dirs

---

<!-- Learnings from implementation will be appended below -->
