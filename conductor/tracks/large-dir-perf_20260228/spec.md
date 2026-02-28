# Spec: Large Directory Performance

## Overview

When the file manager is used in environments like KubeFlow (web-based terminal) where parent folders can contain 1M+ files, opening a directory causes the UI to block because `load_children()` reads all entries synchronously, and `flatten()` recurses the entire expanded tree. This makes the application unusable for large-scale ML workloads with massive checkpoint/artifact directories.

This track introduces lazy loading with pagination, immediate-children-only counting, and hybrid search to keep the UI responsive regardless of directory size.

## Functional Requirements

### FR-1: Lazy Directory Count Badge
- When a directory is collapsed (not yet expanded), show an immediate children count badge in the tree, e.g., `models/ (42 items)`
- The count uses a fast `read_dir().count()` on **immediate children only** — no recursive walk
- The count is computed lazily: only when the directory first appears in the viewport (or on first flatten)
- Directories that have already been expanded and loaded show their actual loaded child count instead

### FR-2: Paginated Directory Loading
- When expanding a directory, load only the first N entries (default: 1,000)
- If the directory has more entries than N, append a virtual "Load more..." node at the end:
  ```
  ▼ checkpoints/ (150,000 items)
    ├── checkpoint_00001/
    ├── checkpoint_00002/
    ├── ...
    ├── checkpoint_01000/
    └── [▼ Load more... (remaining: ~149,000)]
  ```
- Activating (Enter/Right arrow) the "Load more..." node loads the next page of N entries
- The "Load more..." node is replaced with the new entries + a new "Load more..." if more remain
- Sorting is applied per-page (each loaded page is sorted and merged into the existing children)

### FR-3: Configurable Page Size
- Add `max_entries_per_page` option to TOML config under `[general]` section
- Default value: `1000`
- Example config:
  ```toml
  [general]
  max_entries_per_page = 2000
  ```
- Minimum allowed value: 100, maximum: 50,000

### FR-4: Hybrid Fuzzy Search
- **Phase 1 (instant):** When `Ctrl+P` is pressed, search only entries currently loaded in the tree (already in memory). Zero filesystem I/O. Results appear immediately.
- **Phase 2 (opt-in deep search):** If the user hasn't found their file, show a `[🔍 Search deeper...]` option at the bottom of results. Activating it triggers an async filesystem walk (capped, configurable) in the background via `tokio::spawn`.
- During deep search, show a spinner/progress indicator. Results stream in as they're found.
- Deep search cap: reuse `build_path_index()` with configurable cap (default 10K, config option `search_max_entries`)

### FR-5: Non-Blocking FS Change Handling
- `handle_fs_change()` must not reload directories that exceed the page threshold in a single synchronous call
- For paginated directories, only reload the currently loaded pages (not the entire directory)
- The flatten operation should remain fast since it only walks loaded (paginated) nodes

## Non-Functional Requirements

### NFR-1: UI Responsiveness
- Expanding any directory must return control to the event loop within 100ms
- No operation should block the main thread for more than 200ms regardless of directory size
- The "Load more..." action should also complete within 100ms per page

### NFR-2: Memory Efficiency
- Memory usage should be proportional to loaded entries, not total filesystem size
- A tree with 1M files but only 2,000 loaded entries should use ~2,000 entries worth of memory

### NFR-3: Backward Compatibility
- Directories with fewer than `max_entries_per_page` entries behave exactly as before (no pagination, no virtual nodes)
- All existing keybindings and operations (copy, paste, rename, delete, multi-select) work on paginated entries
- Config is optional — zero-config still works with sensible defaults

## Acceptance Criteria

1. ✅ Opening a directory with 100,000+ entries does NOT block the UI
2. ✅ Tree shows `(N items)` badge for collapsed directories using immediate children count
3. ✅ Expanding a large directory shows first 1,000 entries + "Load more..." node
4. ✅ Clicking "Load more..." loads the next page without UI freeze
5. ✅ `Ctrl+P` search returns instant results from loaded entries
6. ✅ "Search deeper..." triggers async walk and streams results
7. ✅ `max_entries_per_page` is configurable in TOML config
8. ✅ FS watcher events don't cause full re-reads of paginated directories
9. ✅ All existing operations (CRUD, clipboard, multi-select) work on paginated entries
10. ✅ Directories under the threshold behave identically to current behavior

## Out of Scope

- Recursive directory size calculation (too expensive for 1M+ files)
- Virtual scrolling / infinite scroll (tree widget handles this through pagination)
- Async initial `load_children()` (pagination solves the blocking issue without async complexity)
- Server-side search indexing (e.g., `locate`, `mlocate` integration)
