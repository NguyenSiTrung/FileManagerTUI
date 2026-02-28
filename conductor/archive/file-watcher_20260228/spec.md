# Specification: File Watcher + Auto-Refresh

## Overview

Add filesystem watching to FileManagerTUI so the tree view automatically reflects
external changes (files created, deleted, renamed, or modified outside the TUI).
Uses the `notify` crate with debouncing, ignore patterns, and flood protection
for environments with extremely large directory trees (1M+ files).

## Functional Requirements

### FR1: Filesystem Watcher Core
- Watch the root directory recursively using `notify` crate (v7) with
  `notify-debouncer-mini` for event debouncing.
- React to ALL filesystem event types: create, delete, rename, modify.
- Default debounce interval: 300ms (configurable via `config.toml`).
- Pipe debounced events into the existing `Event` channel used by the main
  event loop.

### FR2: Ignore Patterns
- Default ignore patterns: `.git`, `node_modules`, `__pycache__`, `venv`,
  `.venv`, `.tox`, `.mypy_cache`, `.pytest_cache`, `target` (Rust build dir).
- Configurable via `[watcher] ignore_patterns` in `config.toml`.
- Events from ignored paths are silently dropped before processing.

### FR3: Event Flood Protection
- Track event count within each debounce window.
- If events exceed a configurable threshold (default: 100 events per debounce
  window), collapse all events into a single full-subtree refresh instead of
  processing each individually.
- This prevents UI stutter when a subprocess creates/deletes thousands of
  files at once (e.g., `rm -rf checkpoints/` or a training run writing many
  checkpoint files).

### FR4: Smart Tree Refresh
- On receiving filesystem events, refresh only the affected subtrees (reload
  the parent directory of changed files) rather than the entire tree.
- After refresh, preserve:
  - **Selected path**: Restore selection to the same path. If deleted, move to
    the nearest surviving sibling (prefer next, fallback to previous, then parent).
  - **Scroll position**: Maintain viewport offset relative to the selected item.
  - **Expanded directories**: Re-expand all previously expanded directories that
    still exist.
- Multi-select is cleared on refresh (existing pattern — flat indices change).
- Search/filter cache is invalidated on refresh (existing pattern).

### FR5: Runtime Toggle
- Keybinding `Ctrl+R` to pause/resume the filesystem watcher at runtime.
- Status bar indicator: show "👁 Watching" or "⏸ Paused" in the status bar
  when watcher is active or paused.

### FR6: Manual Refresh
- `F5` key to force a full tree refresh regardless of watcher state.
- Works even when watcher is disabled or paused.
- Reloads the entire tree from the root, preserving state per FR4.

### FR7: Configuration
- `[watcher]` section in `config.toml`:
  ```toml
  [watcher]
  enabled = true
  debounce_ms = 300
  flood_threshold = 100
  ignore_patterns = [".git", "node_modules", "__pycache__", "target"]
  ```
- CLI flags: `--no-watcher` to disable (already specified in PLAN.md).

## Non-Functional Requirements

### NFR1: Performance
- Watcher must not cause UI lag — events processed asynchronously.
- Debouncing prevents re-render storms during rapid filesystem changes.
- Flood protection prevents O(n) individual refreshes when n >> 100.

### NFR2: Resource Usage
- Single `notify::RecommendedWatcher` instance (OS-level, uses inotify on Linux).
- inotify watch descriptors are finite (~65K default on Linux) — ignore patterns
  reduce descriptor usage for large trees.

### NFR3: Compatibility
- Must work in KubeFlow/Jupyter web terminals (inotify-based, no FSEvents).
- Must handle NFS/FUSE filesystems gracefully — if `notify` cannot watch
  (returns error), log a warning and disable watcher silently.

## Acceptance Criteria

1. Creating a file via `touch newfile.txt` in another terminal causes it to
   appear in the tree within ~300ms (debounce interval).
2. Deleting a file externally removes it from the tree; if it was selected,
   selection moves to the nearest sibling.
3. Renaming a file externally updates the tree without collapsing expanded dirs.
4. Rapidly creating 1000 files (e.g., `for i in $(seq 1000); do touch f$i; done`)
   triggers flood protection and performs a single batch refresh.
5. `F5` forces a full refresh even with watcher disabled (`--no-watcher`).
6. `Ctrl+R` toggles watcher on/off; status bar reflects current state.
7. Directories matching ignore patterns do not trigger refresh events.
8. Watcher gracefully degrades (disables itself) on unsupported filesystems.

## Out of Scope

- Watching multiple disjoint root directories.
- Watching network drives with polling fallback (future enhancement).
- File content change detection for preview auto-refresh (separate feature).
- Custom per-directory ignore patterns (`.fmignore` file).
