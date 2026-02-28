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

- [ ] Task 3: Implement paginated `load_children()`
  <!-- files: src/fs/tree.rs -->
  <!-- depends: task2 -->
  - [ ] Rename current `load_children()` to `load_children_all()` (internal, for small dirs)
  - [ ] Create new `load_children_paged(&mut self, page_size: usize)` that reads only N entries via `read_dir()` iterator
  - [ ] Store the directory's total immediate child count via a fast `read_dir().count()` pre-scan
  - [ ] If total <= page_size, call `load_children_all()` (no pagination needed — backward compatible)
  - [ ] If total > page_size, load first page, set `has_more_children = true`
  - [ ] Add a synthetic "Load more" `TreeNode` child when `has_more_children` is true
  - [ ] Add unit tests: small dir (no pagination), large dir (pagination triggers), edge cases

- [ ] Task 4: Implement `load_next_page()`
  <!-- files: src/fs/tree.rs, src/app.rs -->
  <!-- depends: task3 -->
  - [ ] Add `TreeState::load_next_page(&mut self, parent_path: &Path, page_size: usize)` method
  - [ ] Load next N entries from the directory, append to existing children
  - [ ] Sort the newly loaded entries and merge into existing sorted children
  - [ ] Update/remove the "Load more" node based on remaining entries
  - [ ] Re-flatten the tree after loading
  - [ ] Add unit tests: sequential page loads, final page (no more remaining)

- [ ] Task: Conductor - User Manual Verification 'Core Pagination Infrastructure' (Protocol in workflow.md)

## Phase 2: Tree UI — Count Badge & Load More
<!-- execution: parallel -->

- [ ] Task 1: Render immediate children count badge
  <!-- files: src/components/tree.rs -->
  - [ ] In `components/tree.rs`, for collapsed directory nodes, append ` (N items)` to the display name
  - [ ] Use `total_child_count` from TreeNode if available; otherwise compute lazily via `read_dir().count()`
  - [ ] Cache the count on the TreeNode to avoid re-scanning on every render
  - [ ] Style the badge with dimmed/gray color from theme
  - [ ] Add visual tests / manual verification

- [ ] Task 2: Render "Load more..." virtual node
  <!-- files: src/components/tree.rs -->
  - [ ] In `components/tree.rs`, detect `NodeType::LoadMore` items in `flat_items`
  - [ ] Render as `[▼ Load more... (remaining: ~N)]` with a distinct style (e.g., italic, dimmed cyan)
  - [ ] The node should appear at the correct tree depth with proper indentation/box-drawing
  - [ ] Add visual tests

- [ ] Task 3: Handle "Load more..." activation in handler
  <!-- files: src/handler.rs -->
  - [ ] In `handler.rs`, when Enter or Right arrow is pressed on a `LoadMore` flat item:
    - Extract the parent directory path from the LoadMore item
    - Call `tree_state.load_next_page(parent_path, page_size)`
    - Re-flatten and maintain selection position
  - [ ] Prevent other operations on LoadMore nodes (delete, rename, copy, etc.)
  - [ ] Add handler tests

- [ ] Task: Conductor - User Manual Verification 'Tree UI — Count Badge & Load More' (Protocol in workflow.md)

## Phase 3: Hybrid Fuzzy Search
<!-- execution: sequential -->

- [ ] Task 1: Refactor search to use loaded entries first
  - [ ] Modify `build_path_index()` to collect paths only from loaded tree nodes (in-memory walk of `TreeNode` children, not filesystem)
  - [ ] Rename to `build_loaded_path_index()` for clarity
  - [ ] This is instant — no `fs::read_dir()` calls
  - [ ] Existing fuzzy scoring logic remains unchanged
  - [ ] Add unit tests

- [ ] Task 2: Add "Search deeper..." option
  - [ ] When search results are displayed, append a virtual `[🔍 Search deeper...]` entry at the bottom
  - [ ] When activated, trigger async deep filesystem walk via `tokio::spawn`
  - [ ] Deep walk uses the original `build_path_index()` logic (iterative stack-based, capped by `search_max_entries`)
  - [ ] Show a spinner in the search overlay while deep search runs

- [ ] Task 3: Stream deep search results
  - [ ] Use `mpsc::unbounded_channel` to stream discovered paths from the async walk to the main loop
  - [ ] Merge deep results into the existing search results, re-scoring with fuzzy matcher
  - [ ] Update the results display as new entries arrive
  - [ ] Add a status line: "Deep search: found N files..." while running
  - [ ] Add integration tests

- [ ] Task: Conductor - User Manual Verification 'Hybrid Fuzzy Search' (Protocol in workflow.md)

## Phase 4: FS Watcher & Flatten Compatibility
<!-- execution: sequential -->

- [ ] Task 1: Update `handle_fs_change()` for paginated directories
  - [ ] When reloading a paginated directory, only reload the currently loaded pages (not the full dir)
  - [ ] Preserve pagination state (`loaded_child_count`, `has_more_children`) across reloads
  - [ ] If a changed file is beyond the loaded page range, skip it (it will appear when the user loads more)
  - [ ] Add unit tests for FS change in paginated dirs

- [ ] Task 2: Update `flatten()` and `restore_expanded()` for pagination
  - [ ] `flatten_node()` must emit the `LoadMore` FlatItem when `has_more_children` is true
  - [ ] `restore_expanded()` must use paginated loading when restoring large directories
  - [ ] Ensure `collect_expanded_paths()` works correctly with paginated dirs
  - [ ] Add unit tests

- [ ] Task: Conductor - User Manual Verification 'FS Watcher & Flatten Compatibility' (Protocol in workflow.md)

## Phase 5: Integration, Edge Cases & Polish
<!-- execution: parallel -->

- [ ] Task 1: CRUD operations compatibility
  <!-- files: src/handler.rs, src/app.rs -->
  - [ ] Verify create/rename/delete work correctly in paginated directories
  - [ ] After creating a file in a paginated dir, it should appear in the loaded range (or at the correct sorted position)
  - [ ] After deleting a file from a paginated dir, update counts and pagination state
  - [ ] Multi-select must skip LoadMore nodes
  - [ ] Clipboard paste into paginated dirs works correctly
  - [ ] Add integration tests

- [ ] Task 2: Filter mode compatibility
  <!-- files: src/app.rs -->
  - [ ] Ensure filter mode (`/`) works with paginated entries (filters only loaded entries)
  - [ ] Filter should skip LoadMore virtual nodes
  - [ ] Add tests

- [ ] Task 3: Edge cases and polish
  <!-- files: src/fs/tree.rs, src/components/tree.rs, src/components/help.rs -->
  - [ ] Handle permission denied on `read_dir().count()` gracefully (show `(? items)`)
  - [ ] Handle race conditions: directory contents change between count and first page load
  - [ ] Ensure `build_path_index` deep search respects hidden file toggle
  - [ ] Update help panel with info about pagination behavior
  - [ ] Update status bar to show pagination info when a paginated dir is selected
  - [ ] Add tests for edge cases

- [ ] Task: Conductor - User Manual Verification 'Integration, Edge Cases & Polish' (Protocol in workflow.md)
