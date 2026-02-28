# Track Learnings: sort-order-fix_20260228

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Separate sorting from `load_children` into `TreeState::sort_children_of` — sort concerns belong to TreeState, not TreeNode (from: config-polish_20260228)
- Clone sort fields (sort_by, dirs_first) before `find_node_mut` to avoid borrow checker conflict on `&mut self` (from: config-polish_20260228)
- State preservation on tree refresh: capture (selected path, scroll, expanded set) → reload subtrees → restore_expanded → flatten → restore selection by path lookup → clamp scroll (from: file-watcher_20260228)
- `handle_fs_change()` deduplicates parent directories before reloading to avoid redundant I/O (from: file-watcher_20260228)

---

<!-- Learnings from implementation will be appended below -->

## [2026-02-28 10:55] - Phase 1 Tasks 1-5: Fix sort order in all code paths + regression tests
- **Implemented:** Added `sort_children_of` calls after every `load_children()` in three code paths: `handle_fs_change()`, `restore_expanded()`, and `navigate_to_path()`. Added `sort_children_of_pub` public wrapper. Added 2 regression tests.
- **Files changed:** `src/app.rs`, `src/fs/tree.rs`, `conductor/tracks/sort-order-fix_20260228/plan.md`
- **Commits:** `54bea21`, `389e448`
- **Learnings:**
  - Patterns: Every `load_children()` call MUST be followed by a `sort_children_of()` call — this is the canonical pattern. The `sort_children_of_pub` wrapper enables callers outside `TreeState` to apply sorting.
  - Gotchas: The clone-before-borrow pattern is essential when sort fields (`sort_by`, `dirs_first`) are on the same struct being mutably borrowed for `find_node_mut`.
  - Context: Three code paths were missing sorting: `handle_fs_change`, `restore_expanded`, `navigate_to_path`. All existing paths like `expand_selected`, `reload_dir`, `sort_all_children` already had it correct.
---
