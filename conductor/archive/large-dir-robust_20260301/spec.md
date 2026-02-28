# Spec: Large Directory Robustness & Edge Cases

## Overview

When the file manager is used in environments like KubeFlow (web-based terminal) where parent folders can contain 1M+ files, several performance bottlenecks cause UI freezes, O(n²) degradation, and missing feedback for long-running operations. Additionally, edge cases around symlinks, memory pressure, permission errors, and network filesystems are not handled robustly.

This track performs a comprehensive audit and fix pass across the tree loading pipeline, filesystem watcher, search indexing, directory preview, file operations, and error handling paths.

## Functional Requirements

### FR-1: Single-Pass Snapshot Pagination (replaces double `read_dir`)

- Replace the current two-pass `load_children_paged()` (count pass + load pass) with a single-pass snapshot approach:
  - One `read_dir()` iteration collects lightweight entry data (`OsString` name + `is_dir` flag)
  - The snapshot is sorted once, then paginated by index
  - `TreeNode` metadata (full `stat()`) is loaded only for the current visible page
- For directories under `page_size` threshold: load all entries synchronously (backward compatible, zero overhead)
- For directories above threshold: trigger **async snapshot collection** via `tokio::spawn_blocking`
  - Show the directory node as "⏳ Loading..." while scanning
  - Send `Event::DirScanComplete` when done
  - Support cancellation via `Arc<AtomicBool>` cancel token (Esc key)
- "Load more..." becomes O(1): index `snapshot[offset..offset+page_size]` + stat each entry

### FR-2: O(1) Page Navigation (replaces O(n²) `load_next_page`)

- Replace the current `load_next_page()` HashSet dedup approach with direct index-based access into the snapshot
- Track `loaded_offset: usize` on each `TreeNode` — the snapshot index of the next unloaded entry
- "Load more" loads `snapshot[loaded_offset..loaded_offset+page_size]`, incrementing the offset
- Each page load is constant-time relative to the snapshot (no iteration, no HashSet)

### FR-3: Smart FS Watcher Reload for Paginated Directories

- When `handle_fs_change()` triggers for a paginated directory (one with a snapshot):
  - Do NOT re-scan the entire directory
  - Instead, invalidate the snapshot and mark the directory as "stale"
  - On next user interaction with that directory (expand, scroll, "Load more"), re-scan asynchronously
  - For the currently loaded page, do a targeted check: verify loaded entries still exist, add new entries that sort into the current page range
- Flood protection: if >100 events arrive for a single directory in one debounce window, just mark it stale (no immediate I/O)

### FR-4: Non-Blocking `get_child_count()` with Cached/Estimated Counts

- `get_child_count()` should never block the UI thread for large directories
- Behavior:
  - If count is cached: return it immediately
  - If not cached: return `None` (display "..." instead of a count badge)
  - Trigger an async count in the background; update the badge when complete
- Tree widget renders `(...)` for `None`, `(N items)` for `Some(N)`

### FR-5: Non-Blocking Search Index (`build_path_index`)

- `build_path_index()` must not block the main thread when encountering large unloaded directories
- Split into two phases:
  - **Instant phase**: Collect paths from loaded tree nodes (in-memory, zero I/O)
  - **Async phase**: Walk unloaded directories via `tokio::spawn_blocking`, stream results back via mpsc channel
- Show a spinner in the search overlay during the async phase
- Results from the instant phase appear immediately; async results stream in as found
- Cap total entries at `search_max_entries` (default 10K)

### FR-6: Async Directory Preview Summary

- `load_directory_summary()` (recursive walk capped at 10K) must be non-blocking
- Trigger via `tokio::spawn_blocking`, show "Scanning..." placeholder in preview
- Stream partial results: show counts as they accumulate
- Keep the 10K cap but make the walk non-blocking

### FR-7: Async Recursive Delete with Progress

- Replace `fs::remove_dir_all()` for directory deletion with manual recursive walk
- Show per-file progress: "Deleting 45,230 / ~1,000,000"
- Support cancellation via cancel token (Esc key during progress dialog)
- Use the existing `paste_clipboard_async` pattern (`tokio::spawn` + `Event::Progress` + `Event::OperationComplete`)
- Single file deletion remains synchronous (instant)

### FR-8: Flatten Performance Guard

- Add a guard in `flatten()`: if loaded node count exceeds a threshold (e.g., 50,000), skip deep recursion and use a fast path that only flattens visible pages
- Alternatively, maintain an incremental flat list that is updated on page load rather than rebuilt from scratch
- Goal: `flatten()` should complete in <10ms regardless of loaded entry count

### FR-9: Symlink Loop Detection

- During directory snapshot collection and recursive walks (delete, copy, search index, preview summary):
  - Track visited directories by device+inode (Unix) or canonical path (cross-platform)
  - If a symlink resolves to an already-visited directory, skip it and log a warning
  - Display "(symlink loop)" indicator in the tree for such entries
- Prevent infinite recursion and unbounded memory growth

### FR-10: Memory Pressure Protection

- Cap the snapshot size per directory at a configurable maximum (default: 500,000 entries)
- If a directory exceeds the cap:
  - Store only the first `snapshot_max_entries` entries in the snapshot
  - Show a warning badge: `(showing 500K of ~1.2M items)`
  - The user can still navigate the loaded portion
- Add a global memory estimate: `snapshot_count * ~50 bytes` — warn if total exceeds 100MB
- Config option: `snapshot_max_entries` under `[general]` (default 500,000, min 10,000, max 5,000,000)

### FR-11: Permission-Denied Directory UX

- When `read_dir()` returns `PermissionDenied`:
  - Show a 🔒 icon in the tree next to the directory name
  - Display "(permission denied)" as the child count badge
  - If the user tries to expand: show a status bar message "🔒 Permission denied: <dir_name>"
  - Do NOT silently skip — make the error visible
- When individual entry `stat()` fails during snapshot collection:
  - Skip the entry but increment a "skipped" counter
  - Show "(N entries skipped — permission denied)" in the "Load more" node

### FR-12: Network Filesystem Timeout Handling

- Add a configurable timeout for directory operations (default: 5 seconds)
- Wrap `read_dir()` and `metadata()` calls in `tokio::time::timeout` (for async paths) or check elapsed time periodically (for sync paths)
- If a timeout occurs:
  - Abort the current operation
  - Show status message: "⏱ Timeout reading <dir_name> — network filesystem may be slow"
  - Mark the directory with a ⏱ indicator
  - Allow the user to retry manually (Enter to re-expand)
- Config option: `fs_timeout_secs` under `[general]` (default 5, min 1, max 60)

## Non-Functional Requirements

### NFR-1: UI Responsiveness
- No operation on the main thread should block for more than 100ms, regardless of directory size
- All potentially long I/O operations must be async with progress/cancel support

### NFR-2: Memory Efficiency
- Snapshot memory must be bounded by `snapshot_max_entries` config
- Memory usage proportional to loaded entries, not total filesystem size

### NFR-3: Backward Compatibility
- Directories under `page_size` threshold behave exactly as before
- All existing keybindings and operations work unchanged
- Zero-config works with sensible defaults
- New config options are optional with documented defaults

### NFR-4: Error Resilience
- Permission, timeout, and I/O errors are displayed to the user, never silently swallowed
- Operations degrade gracefully: partial results are shown when possible

## Acceptance Criteria

1. ✅ Expanding a 1M-entry directory does NOT freeze the UI (async snapshot)
2. ✅ "Load more" is instant (<10ms) using index-based pagination
3. ✅ FS watcher events for large directories don't trigger full re-scans
4. ✅ Child count badges load asynchronously (show "..." then actual count)
5. ✅ Ctrl+P search shows instant results from loaded entries + async deep results
6. ✅ Directory preview summary loads asynchronously
7. ✅ Deleting a large directory shows per-file progress and supports cancel
8. ✅ `flatten()` completes in <10ms even with 50K loaded nodes
9. ✅ Symlink loops are detected and displayed, not followed infinitely
10. ✅ Snapshot size is capped with clear user warning
11. ✅ Permission-denied directories show 🔒 icon and clear error message
12. ✅ Network filesystem timeouts are handled with retry option
13. ✅ All new config options have sensible defaults and validation

## Out of Scope

- Async initial tree construction at app startup (root is typically small)
- Server-side search indexing (locate/mlocate integration)
- Virtual scrolling in the tree widget (pagination handles this)
- Background pre-fetching of adjacent directory snapshots
- Parallel multi-directory snapshot collection
