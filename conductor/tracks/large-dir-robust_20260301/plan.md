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

- [ ] Task 1: Add async events for directory scanning
  - [ ] Add `Event::DirScanComplete { path: PathBuf, snapshot: DirSnapshot }` to event.rs
  - [ ] Add `Event::DirCountComplete { path: PathBuf, count: usize }` to event.rs
  - [ ] Add `Event::DirSummaryUpdate { path: PathBuf, files: u64, dirs: u64, size: u64, capped: bool }` to event.rs
  - [ ] Handle new events in main.rs event loop
  - [ ] Write unit tests for event construction

- [ ] Task 2: Async snapshot collection for large directories
  - [ ] In `expand_selected()`: if entry count > page_size, spawn `tokio::spawn_blocking` for snapshot
  - [ ] Mark node as "loading" state (new `is_loading: bool` field on TreeNode)
  - [ ] Show "⏳ Scanning directory..." as a virtual child node during loading
  - [ ] On `Event::DirScanComplete`: attach snapshot to node, load first page, flatten
  - [ ] Support cancel via `Arc<AtomicBool>` (Esc while loading)
  - [ ] Write tests: async expand triggers scan, cancel aborts, completion loads page

- [ ] Task 3: Async child count badges
  - [ ] Modify `get_child_count()`: return cached count or `None` (never block)
  - [ ] When `flatten()` encounters an uncounted directory, queue async count
  - [ ] Batch async count requests to avoid spawning thousands of tasks
  - [ ] On `Event::DirCountComplete`: update TreeNode, re-render (no flatten needed)
  - [ ] Tree widget renders `(...)` for `None`, `(N items)` for `Some(N)`
  - [ ] Write tests: uncounted shows "...", count arrives and updates badge

- [ ] Task 4: Async directory preview summary
  - [ ] Refactor `load_directory_summary()` to spawn `tokio::spawn_blocking`
  - [ ] Show "Scanning..." placeholder immediately
  - [ ] On completion event: update preview with final counts
  - [ ] Keep 10K entry cap in the async walk
  - [ ] Write tests: preview triggers async scan, result updates display

- [ ] Task 5: Async recursive delete with progress
  - [ ] Implement `delete_recursive_async()` using manual walk + per-file delete
  - [ ] Report progress via `Event::Progress` (reuse existing pattern)
  - [ ] Support cancel token (existing `Arc<AtomicBool>` pattern)
  - [ ] Pre-scan for count estimate: quick `read_dir` count for progress denominator
  - [ ] Single file deletion stays synchronous
  - [ ] Modify `handle_delete_confirm()` to use async path for directories
  - [ ] Write tests: delete progress events, cancel mid-delete, error handling

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
