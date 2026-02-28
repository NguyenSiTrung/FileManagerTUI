# Track Learnings: large-dir-perf_20260228

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

---

<!-- Learnings from implementation will be appended below -->
