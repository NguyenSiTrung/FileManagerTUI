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

- [ ] Task 1: Add `handle_fs_change()` method to `App`
  - [ ] Accept `Vec<PathBuf>` of changed paths
  - [ ] Capture current state: selected path, scroll offset, set of expanded directory paths
  - [ ] Call `reload_dir()` for each unique parent of changed paths
  - [ ] After reload, restore state using Task 2's restoration logic
  - [ ] Call `invalidate_search_cache()` after refresh
  - [ ] Reset `last_previewed_index` to force preview refresh
  - [ ] Write integration test: create file externally → call `handle_fs_change` → verify tree updated

- [ ] Task 2: Implement selection and state restoration after refresh
  - [ ] Add helper `TreeState::find_index_by_path(&self, path: &Path) -> Option<usize>` — scan `flat_items` for matching path
  - [ ] After re-flatten, restore `selected_index` by finding previous selected path
  - [ ] If selected path no longer exists: find nearest sibling (next → previous → parent)
  - [ ] Restore `scroll_offset` relative to new `selected_index` (keep same visual position)
  - [ ] Re-expand all previously expanded directories that still exist
  - [ ] Clear `multi_selected` (existing pattern)
  - [ ] Write tests for: path found → index restored; path deleted → nearest sibling; parent deleted → grandparent

- [ ] Task 3: Wire `handle_fs_change` into `main.rs` event loop
  - [ ] Replace stub `FsChange` handler with `app.handle_fs_change(paths)`
  - [ ] Initialize `FsWatcher` in `main.rs` after `App::new()`, pass `event_tx.clone()`
  - [ ] Store `FsWatcher` in a local variable (dropped on quit to clean up inotify watches)
  - [ ] Handle watcher initialization error gracefully: log warning, continue without watcher
  - [ ] Test by running app and creating files in another terminal

## Phase 3: Runtime Controls & Manual Refresh

- [ ] Task 1: Add watcher state to `App` and implement toggle
  - [ ] Add `watcher_active: bool` field to `App` struct (reflects current watcher state)
  - [ ] Add `toggle_watcher()` method that flips `watcher_active` and sends control message
  - [ ] Add `WatcherControl` channel: `main.rs` sends pause/resume commands to `FsWatcher`
  - [ ] `FsWatcher::pause()` / `FsWatcher::resume()` — stop/start forwarding events (keep watcher alive to avoid re-creating inotify watches)
  - [ ] Update status message on toggle: "👁 Watcher resumed" / "⏸ Watcher paused"
  - [ ] Write tests for toggle state transitions

- [ ] Task 2: Add `Ctrl+R` keybinding for watcher toggle
  - [ ] In `handle_normal_mode()`, match `Ctrl+R` → call `toggle_watcher()`
  - [ ] Works in both Tree and Preview focused panels (global key)
  - [ ] Write handler test for `Ctrl+R` key dispatch

- [ ] Task 3: Add `F5` manual refresh keybinding
  - [ ] Add `full_refresh()` method to `App` — reloads entire tree from root with state preservation
  - [ ] In `handle_normal_mode()`, match `F5` → call `app.full_refresh()`
  - [ ] Works regardless of watcher state (even if `--no-watcher`)
  - [ ] Show status message: "🔄 Tree refreshed"
  - [ ] Write handler tests for `F5` key dispatch and `full_refresh()` logic

- [ ] Task 4: Add watcher indicator to status bar
  - [ ] Add `watcher_status: Option<&'a str>` field to `StatusBarWidget`
  - [ ] Add `watcher_status()` builder method
  - [ ] Render watcher indicator (e.g., "👁" or "⏸") in the status bar between clipboard info and key hints
  - [ ] In `ui.rs`, pass watcher status based on `app.watcher_active`
  - [ ] Write widget tests for watcher indicator rendering

## Phase 4: Configuration Integration

- [ ] Task 1: Add watcher config fields to App/config
  - [ ] Add watcher-related fields to `App` struct or a `WatcherConfig` struct:
    - `watcher_enabled: bool` (default: true)
    - `debounce_ms: u64` (default: 300)
    - `flood_threshold: usize` (default: 100)
    - `ignore_patterns: Vec<String>` (defaults from spec)
  - [ ] Pass config values to `FsWatcher::new()` during initialization
  - [ ] Write test: config values correctly propagate to watcher

- [ ] Task 2: Add `--no-watcher` CLI flag
  - [ ] Add `--no-watcher` flag to `Cli` struct in `main.rs` (clap derive)
  - [ ] When `--no-watcher` is set, skip `FsWatcher` initialization entirely
  - [ ] Set `app.watcher_active = false` when watcher is disabled
  - [ ] `F5` manual refresh still works even with `--no-watcher`
  - [ ] Write test: `--no-watcher` flag prevents watcher creation

- [ ] Task 3: Graceful degradation for unsupported filesystems
  - [ ] Catch `notify::Error` during watcher initialization
  - [ ] If error (e.g., NFS, FUSE, inotify limit reached): set status message warning, continue without watcher
  - [ ] Set `app.watcher_active = false` on error
  - [ ] Write test: watcher error → app continues normally
