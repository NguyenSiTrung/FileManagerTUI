# Plan: Sort Order Lost After File Watcher Refresh

## Phase 1: Fix Sort Order in All Code Paths + Regression Test

- [x] Task 1: Add sorting after `load_children()` in `handle_fs_change()`
  - In `app.rs`, after the loop that reloads affected directories, apply `TreeState::sort_children_of()` to each reloaded node
  - Verify manually with `cargo run`

- [x] Task 2: Add sorting after `load_children()` in `restore_expanded()`
  - In `tree.rs`, after `node.load_children()` in `restore_expanded()`, apply sorting
  - Need to capture `sort_by` and `dirs_first` before the loop (same pattern as `expand_selected`)

- [x] Task 3: Add sorting after `load_children()` in `navigate_to_path()`
  - In `app.rs`, after `node.load_children()` in the ancestor expansion loop, apply sorting via `TreeState::sort_children_of()`

- [x] Task 4: Add regression test for sort order after `handle_fs_change()`
  - Create test `handle_fs_change_preserves_sort_order` in `app.rs` tests
  - Setup: create dirs + files, trigger `handle_fs_change`, assert dirs-first order is maintained
  - Run `cargo test` to verify all tests pass

- [x] Task 5: Final verification
  - Run `cargo clippy -- -D warnings`
  - Run `cargo fmt --check`
  - Run `cargo test`

- [ ] Task: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)
