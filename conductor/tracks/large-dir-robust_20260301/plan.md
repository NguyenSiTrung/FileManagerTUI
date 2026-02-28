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

- [ ] Task 1: Smart watcher reload for paginated directories
  - [ ] Add `is_stale: bool` field to TreeNode
  - [ ] In `handle_fs_change()`: for paginated dirs, set `is_stale = true` instead of full reload
  - [ ] On user interaction (expand, "Load more"): if stale, re-scan snapshot async
  - [ ] For non-paginated dirs: keep current immediate reload behavior
  - [ ] Targeted check for current page: verify entries exist, detect new entries
  - [ ] Write tests: stale marking, re-scan on interact, non-paginated unchanged

- [ ] Task 2: Non-blocking search index
  - [ ] Split `build_path_index()` into instant phase (in-memory) + async phase (filesystem walk)
  - [ ] Instant phase: return loaded paths immediately, start async walk for unloaded dirs
  - [ ] Async walk sends `Event::SearchIndexUpdate { paths: Vec<PathBuf> }` incrementally
  - [ ] Search overlay shows results from instant phase immediately
  - [ ] Show spinner indicator while async walk is in progress
  - [ ] Write tests: instant results appear, async results stream in, cap respected

## Phase 4: Edge Cases & Robustness (FR-8, FR-9, FR-10, FR-11, FR-12)

- [ ] Task 1: Flatten performance guard
  - [ ] Add a fast path in `flatten()` for high node counts (>50K)
  - [ ] Track total loaded node count on TreeState
  - [ ] For fast path: only flatten currently visible pages + one buffer page
  - [ ] OR: maintain incremental flat list updated on page load
  - [ ] Write tests: flatten with 50K+ nodes completes in <10ms, correctness preserved

- [ ] Task 2: Symlink loop detection
  - [ ] Create `VisitedDirs` tracker using `HashSet<(u64, u64)>` (dev+inode on Unix)
  - [ ] Pass tracker through snapshot collection, recursive walks (delete/copy/search/preview)
  - [ ] If symlink resolves to visited dir: skip, add "(symlink loop)" indicator
  - [ ] Add `is_symlink_loop: bool` to FlatItem for rendering
  - [ ] Write tests: create symlink loop, verify detection and skip

- [ ] Task 3: Memory pressure protection
  - [ ] Add `snapshot_max_entries` config option (default 500K, min 10K, max 5M)
  - [ ] Cap `DirSnapshot::collect()` at `snapshot_max_entries`
  - [ ] If capped: set `snapshot_capped: bool` flag on snapshot
  - [ ] Show warning badge: "(showing 500K of ~1.2M items)"
  - [ ] Add global snapshot memory tracking on TreeState
  - [ ] Write tests: cap enforcement, warning display, config validation

- [ ] Task 4: Permission-denied directory UX
  - [ ] Detect `PermissionDenied` from `read_dir()` in snapshot collection
  - [ ] Set `permission_denied: bool` flag on TreeNode
  - [ ] Tree widget shows 🔒 icon for permission-denied directories
  - [ ] Count badge shows "(permission denied)" instead of count
  - [ ] Expand attempt shows status bar message
  - [ ] Track skipped entries during snapshot collection
  - [ ] Write tests: permission-denied dir shows lock icon, expand shows message

- [ ] Task 5: Network filesystem timeout handling
  - [ ] Add `fs_timeout_secs` config option (default 5, min 1, max 60)
  - [ ] For async paths: wrap in `tokio::time::timeout`
  - [ ] For sync paths: check `Instant::elapsed()` periodically during iteration
  - [ ] On timeout: abort operation, show "⏱ Timeout" status message
  - [ ] Mark directory with timeout indicator, allow manual retry
  - [ ] Write tests: simulated slow FS (mock), timeout triggers, retry works

## Phase 5: Integration, Config & Polish

- [ ] Task 1: Config integration and validation
  - [ ] Add `snapshot_max_entries` to `GeneralConfig`
  - [ ] Add `fs_timeout_secs` to `GeneralConfig`
  - [ ] Implement `.or()` merge for new fields
  - [ ] Add clamping validation (min/max bounds)
  - [ ] Add CLI flags: `--snapshot-max`, `--fs-timeout`
  - [ ] Write tests: config parsing, merge, clamping, defaults

- [ ] Task 2: Integration testing
  - [ ] Test: expand large dir → async scan → load page → "Load more" → sort change
  - [ ] Test: FS change during async scan → stale handling
  - [ ] Test: search while async scan in progress
  - [ ] Test: delete large dir → progress → cancel → partial delete cleanup
  - [ ] Test: permission-denied + timeout in same tree
  - [ ] Test: symlink loop inside paginated directory

- [ ] Task 3: Polish and edge cases
  - [ ] Verify all `invalidate_search_cache()` calls include snapshot invalidation
  - [ ] Verify "Load more" node skips multi-select correctly (already done, verify)
  - [ ] Verify copy/paste into paginated directory updates snapshot
  - [ ] Verify rename in paginated directory updates snapshot
  - [ ] Update help overlay with any new indicators (🔒, ⏱, symlink loop)
  - [ ] Review and update status bar messages for consistency

- [ ] Task: Conductor - User Manual Verification 'Integration, Config & Polish' (Protocol in workflow.md)
