# Plan: Large Directory Performance

## Phase 1: Core Pagination Infrastructure
<!-- execution: parallel -->

- [x] Task 1: Add pagination config options
  <!-- files: src/config.rs -->
  - [x] Add `max_entries_per_page: Option<u32>` to `GeneralConfig` in `config.rs` (default: 1000)
  - [x] Add `search_max_entries: Option<u32>` to `GeneralConfig` (default: 10000)
  - [x] Add validation: clamp page size to 100..50000 range
  - [x] Add tests for config parsing with new fields

- [x] Task 2: Extend TreeNode with pagination state
  <!-- files: src/fs/tree.rs -->
  - [x] Add fields to `TreeNode`: `total_child_count: Option<usize>`, `loaded_child_count: usize`, `has_more_children: bool`
  - [x] Add `NodeType::LoadMore` variant for the virtual "Load more" node
  - [x] Add `FlatItem` support for `NodeType::LoadMore` (carries parent path + remaining count)
  - [x] Add unit tests for new TreeNode fields

- [x] Task 3: Implement paginated `load_children()`
  <!-- files: src/fs/tree.rs -->
  <!-- depends: task2 -->
  - [x] Rename current `load_children()` to `load_children_all()` (internal, for small dirs)
  - [x] Create new `load_children_paged(&mut self, page_size: usize)` that reads only N entries via `read_dir()` iterator
  - [x] Store the directory's total immediate child count via a fast `read_dir().count()` pre-scan
  - [x] If total <= page_size, call `load_children_all()` (no pagination needed — backward compatible)
  - [x] If total > page_size, load first page, set `has_more_children = true`
  - [x] Add a synthetic "Load more" `TreeNode` child when `has_more_children` is true
  - [x] Add unit tests: small dir (no pagination), large dir (pagination triggers), edge cases

- [x] Task 4: Implement `load_next_page()`
  <!-- files: src/fs/tree.rs, src/app.rs -->
  <!-- depends: task3 -->
  - [x] Add `TreeState::load_next_page(&mut self, parent_path: &Path, page_size: usize)` method
  - [x] Load next N entries from the directory, append to existing children
  - [x] Sort the newly loaded entries and merge into existing sorted children
  - [x] Update/remove the "Load more" node based on remaining entries
  - [x] Re-flatten the tree after loading
  - [x] Add unit tests: sequential page loads, final page (no more remaining)

- [x] Task: Conductor - User Manual Verification 'Core Pagination Infrastructure' (Protocol in workflow.md)

## Phase 2: Tree UI — Count Badge & Load More
<!-- execution: parallel -->

- [x] Task 1: Render immediate children count badge
  <!-- files: src/components/tree.rs -->
  - [x] In `components/tree.rs`, for collapsed directory nodes, append ` (N items)` to the display name
  - [x] Use `total_child_count` from TreeNode if available; otherwise compute lazily via `read_dir().count()`
  - [x] Cache the count on the TreeNode to avoid re-scanning on every render
  - [x] Style the badge with dimmed/gray color from theme
  - [x] Add visual tests / manual verification

- [x] Task 2: Render "Load more..." virtual node
  <!-- files: src/components/tree.rs -->
  - [x] In `components/tree.rs`, detect `NodeType::LoadMore` items in `flat_items`
  - [x] Render as `[▼ Load more... (remaining: ~N)]` with a distinct style (e.g., italic, dimmed cyan)
  - [x] The node should appear at the correct tree depth with proper indentation/box-drawing
  - [x] Add visual tests

- [x] Task 3: Handle "Load more..." activation in handler
  <!-- files: src/handler.rs -->
  - [x] In `handler.rs`, when Enter or Right arrow is pressed on a `LoadMore` flat item:
    - Extract the parent directory path from the LoadMore item
    - Call `tree_state.load_next_page(parent_path, page_size)`
    - Re-flatten and maintain selection position
  - [x] Prevent other operations on LoadMore nodes (delete, rename, copy, etc.)
  - [x] Add handler tests

- [x] Task: Conductor - User Manual Verification 'Tree UI — Count Badge & Load More' (Protocol in workflow.md)

## Phase 3: Hybrid Fuzzy Search
<!-- execution: sequential -->

- [x] Task 1: Refactor search to use loaded entries first
  - [x] Modify `build_path_index()` to use hybrid approach: loaded tree nodes first, then bounded FS walk
  - [x] Preserved old implementation as `build_deep_path_index()` for future use
  - [x] Uses configurable `search_max_entries` instead of hardcoded cap
  - [x] Existing fuzzy scoring logic remains unchanged
  - [x] Updated tests to reload tree (matches FS watcher behavior)

- [ ] Task 2: Add "Search deeper..." option *(deferred — async deep search)*
  - [ ] When search results are displayed, append a virtual `[🔍 Search deeper...]` entry at the bottom
  - [ ] When activated, trigger async deep filesystem walk via `tokio::spawn`
  - [ ] Deep walk uses `build_deep_path_index()` logic (iterative stack-based, capped by `search_max_entries`)
  - [ ] Show a spinner in the search overlay while deep search runs

- [ ] Task 3: Stream deep search results *(deferred — async deep search)*
  - [ ] Use `mpsc::unbounded_channel` to stream discovered paths from the async walk to the main loop
  - [ ] Merge deep results into the existing search results, re-scoring with fuzzy matcher
  - [ ] Update the results display as new entries arrive
  - [ ] Add a status line: "Deep search: found N files..." while running
  - [ ] Add integration tests

- [x] Task: Conductor - User Manual Verification 'Hybrid Fuzzy Search' (Protocol in workflow.md)

## Phase 4: FS Watcher & Flatten Compatibility
<!-- execution: sequential -->

- [x] Task 1: Update `handle_fs_change()` for paginated directories
  - [x] When reloading a paginated directory, uses `load_children_paged()` to preserve pagination
  - [x] Preserve pagination state (`loaded_child_count`, `has_more_children`) across reloads
  - [x] `restore_expanded()` uses paginated loading
  - [x] Already wired in Phase 1

- [x] Task 2: Update `flatten()` and `restore_expanded()` for pagination
  - [x] `flatten_node()` emits the `LoadMore` FlatItem when `has_more_children` is true
  - [x] `restore_expanded()` uses paginated loading when restoring large directories
  - [x] `collect_expanded_paths()` works correctly with paginated dirs
  - [x] Already done in Phase 1

- [x] Task: Conductor - User Manual Verification 'FS Watcher & Flatten Compatibility' (Protocol in workflow.md)

## Phase 5: Integration, Edge Cases & Polish
<!-- execution: parallel -->

- [x] Task 1: CRUD operations compatibility
  <!-- files: src/handler.rs, src/app.rs -->
  - [x] Verify create/rename/delete work correctly in paginated directories
  - [x] After creating a file in a paginated dir, it should appear in the loaded range (or at the correct sorted position)
  - [x] After deleting a file from a paginated dir, update counts and pagination state
  - [x] Multi-select must skip LoadMore nodes
  - [x] Clipboard paste into paginated dirs works correctly
  - [x] Add integration tests

- [x] Task 2: Filter mode compatibility
  <!-- files: src/app.rs -->
  - [x] Ensure filter mode (`/`) works with paginated entries (filters only loaded entries)
  - [x] Filter correctly skips LoadMore virtual nodes (uses flatten_node_filtered)
  - [x] Verified via existing tests

- [x] Task 3: Edge cases and polish
  <!-- files: src/fs/tree.rs, src/components/tree.rs, src/components/help.rs -->
  - [x] Handle permission denied on `read_dir().count()` gracefully (returns None)
  - [x] Handle race conditions: directory contents change between count and first page load
  - [x] Ensure `build_path_index` hybrid search respects pagination
  - [x] Update help panel with info about pagination behavior
  - [x] Update status bar to show pagination info when a paginated dir is selected
  - [x] Multi-select skips LoadMore nodes

- [x] Task: Conductor - User Manual Verification 'Integration, Edge Cases & Polish' (Protocol in workflow.md)
