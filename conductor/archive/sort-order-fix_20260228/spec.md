# Spec: Sort Order Lost After File Watcher Refresh

## Overview
**Type:** Bug fix

The tree view initially displays items correctly (directories first, then files, both alphabetically sorted). However, after the file watcher triggers a refresh (typically within a few seconds of navigating), the sort order reverts to raw filesystem order. This is because `handle_fs_change()`, `restore_expanded()`, and `navigate_to_path()` call `load_children()` without applying sorting afterwards.

## Root Cause
`TreeNode::load_children()` intentionally does not sort — sorting is the responsibility of `TreeState`. Three code paths call `load_children()` without follow-up sorting:

1. **`App::handle_fs_change()`** (app.rs:1146) — reloads affected directories on fs events
2. **`TreeState::restore_expanded()`** (tree.rs:540) — re-expands dirs during refresh
3. **`App::navigate_to_path()`** (app.rs:1052) — expands ancestors for search navigation

## Functional Requirements
1. After a file watcher refresh, the tree must maintain the current `sort_by` and `dirs_first` settings
2. After `restore_expanded()` re-loads directory children, they must be sorted
3. After `navigate_to_path()` expands ancestor dirs, they must be sorted
4. Existing sort behavior (cycle_sort, toggle_dirs_first, expand_selected, reload_dir) must remain unchanged

## Acceptance Criteria
- [ ] Directories appear before files after a file watcher refresh
- [ ] Sort order (name/size/modified) is preserved after watcher refresh
- [ ] `navigate_to_path()` produces correctly sorted tree
- [ ] Regression test verifies sort order is maintained after `handle_fs_change()`
- [ ] All existing tests pass
- [ ] `cargo clippy -- -D warnings` clean

## Out of Scope
- Changing the `load_children()` API or TreeNode/TreeState separation
- Adding new sort modes
- Performance optimization of sorting
