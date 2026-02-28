# Implementation Plan: File Watcher + Auto-Refresh

## Phase 1: Watcher Core Module

- [x] Task 1: Create `fs/watcher.rs` — `FsWatcher` struct with `notify` integration (3840b76)
  - [x] Add `notify` and `notify-debouncer-mini` to `Cargo.toml` (already listed in tech-stack)
  - [x] Create `FsWatcher` struct wrapping `notify::RecommendedWatcher` with debouncer
  - [x] Accept root path, debounce duration, ignore patterns, and `mpsc::UnboundedSender<Event>`
  - [x] Implement `new()` that creates recursive watcher and starts watching root
  - [x] Implement `stop()` to drop the watcher
  - [x] Filter events from ignored directories before forwarding
  - [x] Export module in `fs/mod.rs`
  - [x] Write unit tests for ignore pattern filtering logic

- [x] Task 2: Add `FsEvent` variant to `Event` enum and wire into main loop (3840b76)
  - [x] Add `FsChange(Vec<PathBuf>)` variant to `Event` enum in `event.rs`
  - [x] In watcher callback, collect debounced event paths and send as `FsChange`
  - [x] Add `Event::FsChange` match arm in `main.rs` event loop (stub handler for now)
  - [x] Write test verifying `FsChange` events are constructable and sendable

- [x] Task 3: Implement flood protection in `FsWatcher` (3840b76)
  - [x] Track event count within debounce window
  - [x] If count exceeds `flood_threshold` (default: 100), emit a single `FsChange` with root path (full refresh) instead of individual paths
  - [x] Reset counter after each debounce flush
  - [x] Write tests for threshold logic (below threshold → individual paths, above → root path)

## Phase 2: Smart Tree Refresh with State Preservation

- [x] Task 1: Add `handle_fs_change()` method to `App` (d995cb5)
  - [x] Accept `Vec<PathBuf>` of changed paths
  - [x] Capture current state: selected path, scroll offset, set of expanded directory paths
  - [x] Call `reload_dir()` for each unique parent of changed paths
  - [x] After reload, restore state using Task 2's restoration logic
  - [x] Call `invalidate_search_cache()` after refresh
  - [x] Reset `last_previewed_index` to force preview refresh
  - [x] Write integration test: create file externally → call `handle_fs_change` → verify tree updated

- [x] Task 2: Implement selection and state restoration after refresh (d995cb5)
  - [x] Add helper `TreeState::find_index_by_path(&self, path: &Path) -> Option<usize>`
  - [x] After re-flatten, restore `selected_index` by finding previous selected path
  - [x] If selected path no longer exists: find nearest sibling (next → previous → parent)
  - [x] Restore `scroll_offset` relative to new `selected_index`
  - [x] Re-expand all previously expanded directories that still exist
  - [x] Clear `multi_selected` (existing pattern)
  - [x] Write tests for: path found → index restored; path deleted → nearest sibling

- [x] Task 3: Wire `handle_fs_change` into `main.rs` event loop (d995cb5)
  - [x] Replace stub `FsChange` handler with `app.handle_fs_change(paths)`
  - [x] Initialize `FsWatcher` in `main.rs` after `App::new()`, pass `event_tx.clone()`
  - [x] Store `FsWatcher` in a local variable (dropped on quit to clean up inotify watches)
  - [x] Handle watcher initialization error gracefully: log warning, continue without watcher

## Phase 3: Runtime Controls & Manual Refresh

- [x] Task 1: Add watcher state to `App` and implement toggle (d995cb5)
  - [x] Add `watcher_active: bool` field to `App` struct
  - [x] Add `toggle_watcher()` method that flips `watcher_active` and shows status
  - [x] `FsWatcher::pause()` / `FsWatcher::resume()` — stop/start forwarding events
  - [x] Update status message on toggle: "👁 Watcher resumed" / "⏸ Watcher paused"
  - [x] Write tests for toggle state transitions

- [x] Task 2: Add `Ctrl+R` keybinding for watcher toggle (d995cb5)
  - [x] In `handle_normal_mode()`, match `Ctrl+R` → call `toggle_watcher()`
  - [x] Works in both Tree and Preview focused panels (global key)
  - [x] Write handler test for `Ctrl+R` key dispatch

- [x] Task 3: Add `F5` manual refresh keybinding (d995cb5)
  - [x] Add `full_refresh()` method to `App`
  - [x] In `handle_normal_mode()`, match `F5` → call `app.full_refresh()`
  - [x] Works regardless of watcher state (even if `--no-watcher`)
  - [x] Show status message: "🔄 Tree refreshed"
  - [x] Write handler tests for `F5` key dispatch and `full_refresh()` logic

- [x] Task 4: Add watcher indicator to status bar (d995cb5)
  - [x] Add `watcher_status: Option<&'a str>` field to `StatusBarWidget`
  - [x] Add `watcher_status()` builder method
  - [x] Render watcher indicator (👁/⏸) in the status bar
  - [x] In `ui.rs`, pass watcher status based on `app.watcher_active`

## Phase 4: Configuration Integration

- [x] Task 1: Add watcher config fields to App/config (d995cb5)
  - [x] Default constants in `fs/watcher.rs`: `DEFAULT_DEBOUNCE_MS`, `DEFAULT_FLOOD_THRESHOLD`, `DEFAULT_IGNORE_PATTERNS`
  - [x] Pass config values to `FsWatcher::new()` during initialization

- [x] Task 2: Add `--no-watcher` CLI flag (d995cb5)
  - [x] Add `--no-watcher` flag to `Cli` struct in `main.rs` (clap derive)
  - [x] When `--no-watcher` is set, skip `FsWatcher` initialization entirely
  - [x] Set `app.watcher_active = false` when watcher is disabled
  - [x] `F5` manual refresh still works even with `--no-watcher`

- [x] Task 3: Graceful degradation for unsupported filesystems (d995cb5)
  - [x] Catch `notify::Error` during watcher initialization
  - [x] If error: set status message warning, continue without watcher
  - [x] Set `app.watcher_active = false` on error
