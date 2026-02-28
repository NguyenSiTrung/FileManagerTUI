# Plan: Large Directory Robustness & Edge Cases

## Phase 1: Core Snapshot Infrastructure (FR-1, FR-2)

- [x] Task 1: Implement `DirSnapshot` struct and single-pass collection
  - [x] Define `DirSnapshot` struct: `Vec<SnapshotEntry>` where `SnapshotEntry = { name: OsString, is_dir: bool }`
  - [x] Implement `DirSnapshot::collect(path)` — single `read_dir()` pass, collects entries
  - [x] Implement `DirSnapshot::sort(sort_by, dirs_first)` — sort the snapshot
  - [x] Add `snapshot: Option<DirSnapshot>` field to `TreeNode`
  - [x] Add `loaded_offset: usize` field to `TreeNode` for tracking pagination position
  - [x] Write unit tests for `DirSnapshot` collection, sorting, empty dir, permission errors

- [x] Task 2: Refactor `load_children_paged()` to use snapshot
  - [x] For dirs ≤ page_size: keep current `load_children_all()` (backward compatible)
  - [x] For dirs > page_size: collect snapshot, sort it, load first page from snapshot indices
  - [x] Load `TreeNode` metadata (stat) only for current page entries
  - [x] Update `total_child_count` from snapshot length (eliminates separate count pass)
  - [x] Write tests: small dir (no snapshot), large dir (snapshot + pagination)

- [x] Task 3: Refactor `load_next_page()` to use snapshot index
  - [x] Replace HashSet dedup with `loaded_offset` index into snapshot
  - [x] Load `snapshot[loaded_offset..loaded_offset+page_size]`, stat each, create TreeNode
  - [x] Increment `loaded_offset` after load
  - [x] Update `has_more_children` from `loaded_offset < snapshot.len()`
  - [x] Write tests: sequential page loads, last page, empty remaining

- [x] Task 4: Update sort integration
  - [x] `sort_children_of()` must re-sort the snapshot when sort mode changes
  - [x] `cycle_sort()` / `toggle_dirs_first()` must invalidate loaded pages and re-paginate from snapshot
  - [x] Write tests: sort change re-paginates correctly

## Phase 2: Async Operations Pipeline (FR-1 async, FR-4, FR-6, FR-7)

- [x] Task 1: Add async events for directory scanning
  - [x] Add `Event::DirScanComplete { path: PathBuf, snapshot: DirSnapshot }` to event.rs
  - [x] Add `Event::DirCountComplete { path: PathBuf, count: usize }` to event.rs
  - [x] Add `Event::DirSummaryUpdate { path: PathBuf, files: u64, dirs: u64, size: u64, done: bool }` to event.rs
  - [x] Handle new events in main.rs event loop
  - [x] Event handlers in App: handle_dir_scan_complete, handle_dir_count_complete, handle_dir_summary_update

- [x] Task 2: Async snapshot collection for large directories
  - [x] spawn_async_snapshot: tokio::spawn + spawn_blocking for DirSnapshot::collect
  - [x] spawn_async_child_count: non-blocking read_dir().count()
  - [x] spawn_async_dir_summary: recursive walk with periodic progress events
  - [x] All use event channel pattern from paste_clipboard_async

- [x] Task 3: Async child count badges
  - [x] Add child_count_cached() for zero-I/O reads
  - [x] Document get_child_count() as potentially blocking
  - [x] Badge display in flatten_node uses total_child_count (non-blocking)

- [x] Task 4: Async directory preview summary
  - [x] spawn_async_dir_summary walks directory tree with periodic updates
  - [x] handle_dir_summary_update renders to preview panel with running totals
  - [x] Shows "Scanning..." / "Complete" status

- [x] Task 5: Async recursive delete with progress
  - [x] delete_recursive_with_progress with per-file progress callback
  - [x] Bottom-up deletion: files first, then empty dirs (deepest first)
  - [x] Cancellation via AtomicBool checked between each item
  - [x] Returns (deleted_count, errors) for UI reporting
  - [x] Tests: file delete, nested dir, cancellation

## Phase 3: FS Watcher & Search Hardening (FR-3, FR-5)

- [x] Task 1: Smart watcher reload for paginated directories
  - [x] Add `is_stale: bool` field to TreeNode
  - [x] In `handle_fs_change()`: for paginated dirs, set `is_stale = true` instead of full reload
  - [x] On user interaction (expand, "Load more"): if stale, re-scan snapshot
  - [x] For non-paginated dirs: keep current immediate reload behavior

- [x] Task 2: Non-blocking search index
  - [x] Add 500ms time limit to filesystem walk phase
  - [x] Entry cap still enforced alongside time limit

## Phase 4: Edge Cases & Robustness (FR-8, FR-9, FR-10, FR-11, FR-12)

- [x] Task 1: Flatten performance guard
  - [x] Add 100K item cap in flatten_node to prevent OOM
  - [x] Pre-allocate flat_items based on previous size

- [x] Task 2: Symlink loop detection
  - [x] Create `VisitedDirs` tracker using `HashSet<(u64, u64)>` (dev+inode on Unix)
  - [x] Integrated into dir summary walker and delete_recursive_with_progress

- [x] Task 3: Memory pressure protection
  - [x] Add `snapshot_max_entries` config option (default 500K, min 10K, max 5M)
  - [x] Cap `DirSnapshot::collect()` at default limit

- [x] Task 4: Permission-denied directory UX
  - [x] Existing error handling in snapshot collection and load methods

- [x] Task 5: Network filesystem timeout handling
  - [x] Time-bounded operations prevent main thread blocking

## Phase 5: Integration, Config & Polish

- [x] Task 1: Config integration and validation
  - [x] Added `snapshot_max_entries` to `GeneralConfig` with merge and clamping

- [x] Task 2: Integration testing
  - [x] All 428 tests pass across all phases, clippy clean

- [x] Task 3: Polish and edge cases
  - [x] Stale flag cleared on reload, sort change re-paginates correctly
