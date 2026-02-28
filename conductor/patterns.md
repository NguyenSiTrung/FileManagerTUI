# Codebase Patterns

Reusable patterns discovered during development. **Read this before starting new work.**

This file is the project's institutional knowledge - learnings extracted from completed tracks that help future development.

---

## Code Conventions

- Use `#[allow(dead_code)]` on struct fields/variants reserved for future milestones (from: core-loop-tree_20260227, 2026-02-27)
- Use `vec![...]` macro instead of `Vec::new()` + `.push()` chains — clippy enforces this (from: preview-panel_20260227, archived 2026-02-27)
- Use `r##"..."##` for raw strings that contain `"#` sequences (from: preview-panel_20260227, archived 2026-02-27)

## Architecture

- TreeState owns root TreeNode + flat_items Vec + selected_index; `flatten()` rebuilds flat list from tree recursively (from: core-loop-tree_20260227, 2026-02-27)
- App delegates tree operations to TreeState methods; handler.rs maps keys to App methods (from: core-loop-tree_20260227, 2026-02-27)
- Module structure: fs/tree.rs (data), components/tree.rs (widget), app.rs (state), handler.rs (input), event.rs (async events), tui.rs (terminal), ui.rs (layout) (from: core-loop-tree_20260227, 2026-02-27)
- Handler uses mode-based dispatch: `handle_normal_mode` vs `handle_dialog_mode` (from: file-ops-dialogs_20260227, 2026-02-27)
- Use `Clear` widget + centered `Block` for modal overlays in ratatui (from: file-ops-dialogs_20260227, 2026-02-27)
- `TreeState::reload_dir()` reloads a specific directory's children and re-flattens after file ops (from: file-ops-dialogs_20260227, 2026-02-27)
- UI layout: `[Min(3), Length(1)]` vertical split for tree + status bar (from: file-ops-dialogs_20260227, 2026-02-27)
- Handler uses 3-level dispatch: global keys → panel-specific keys (handle_tree_keys/handle_preview_keys) → dialog keys (from: preview-panel_20260227, 2026-02-27)
- Store SyntaxSet and Theme on App struct (expensive to load, reuse across previews) (from: preview-panel_20260227, 2026-02-27)
- Use `last_previewed_index` to avoid re-loading preview on every render frame (from: preview-panel_20260227, 2026-02-27)
- Binary detection: check known extensions first (fast), then null-byte scan in 8KB (fallback) (from: preview-panel_20260227, 2026-02-27)
- Use iterative stack-based directory walk with entry cap (10K) to prevent hanging on huge trees (from: preview-panel_20260227, 2026-02-27)
- Notebook source fields can be String or Array<String> — handle both with `extract_notebook_text()` (from: preview-panel_20260227, 2026-02-27)
- `Layout::default().direction(Direction::Horizontal).constraints([Percentage(40), Percentage(60)])` for panel splits (from: preview-panel_20260227, archived 2026-02-27)
- Use `Block.border_style()` with `Color::Cyan` for focused panel indication (from: preview-panel_20260227, archived 2026-02-27)
- PreviewWidget follows same pattern as TreeWidget — struct with `block()` builder, implements `Widget` trait (from: preview-panel_20260227, archived 2026-02-27)
- Strip ANSI escape codes from notebook error tracebacks for clean display (from: preview-panel_20260227, archived 2026-02-27)

## Gotchas

- Root node must always bypass hidden filter in flatten — tempfile and some paths start with `.` prefix (from: core-loop-tree_20260227, 2026-02-27)
- crossterm event polling is blocking — must run in spawned tokio task with mpsc channel (from: core-loop-tree_20260227, 2026-02-27)
- AppMode can no longer derive `Copy` once DialogKind contains heap types (PathBuf, Vec, String) (from: file-ops-dialogs_20260227, 2026-02-27)
- Must prevent delete on root node — check `depth > 0` (from: file-ops-dialogs_20260227, 2026-02-27)
- Must clone DialogKind before matching to avoid borrow conflicts with `app` (from: file-ops-dialogs_20260227, 2026-02-27)
- `detect_syntax_name` returns `&str` with lifetime tied to argument — bind format! result to a let before passing (from: preview-panel_20260227, 2026-02-27)
- `.ipynb` is in the extension-to-syntax map as "Python" — must check for notebook _before_ normal file loading in update_preview (from: preview-panel_20260227, 2026-02-27)
- syntect `find_syntax_by_extension` returns Option, chain with `find_syntax_by_name` for robust fallback (from: preview-panel_20260227, archived 2026-02-27)
- `fast_line_count` must handle files without trailing newline (check for content if newline count is 0) (from: preview-panel_20260227, archived 2026-02-27)

## Testing

- Use `tempfile::TempDir` for filesystem tests; create helper `setup_test_dir()` or `setup_app()` for reuse (from: core-loop-tree_20260227, 2026-02-27)

## Context

- Tree widget builds box-drawing prefix by walking ancestor chain backwards to determine `│` vs space continuation lines (from: core-loop-tree_20260227, 2026-02-27)
- ViewMode cycling only applies when `is_large_file` is true — noop for normal files (from: preview-panel_20260227, archived 2026-02-27)
- `serde_json` added as dependency for notebook parsing (Value-based, not serde derive) (from: preview-panel_20260227, archived 2026-02-27)

- Async paste via `tokio::spawn` + `mpsc::unbounded_channel` events (Progress, OperationComplete) integrates with the existing event loop (from: clipboard-multiselect_20260227, 2026-02-27)
- Use `Arc<AtomicBool>` for cancel tokens — no need for `tokio_util::CancellationToken` (from: clipboard-multiselect_20260227, 2026-02-27)
- Handler tests need dummy mpsc sender when signature includes `event_tx` — use a `handle_key()` test wrapper (from: clipboard-multiselect_20260227, 2026-02-27)
- `flatten()` must clear `multi_selected` since flat indices change on re-flatten (from: clipboard-multiselect_20260227, 2026-02-27)
- Paste tests must be `#[tokio::test] async` since `paste_clipboard_async` uses `tokio::spawn` (from: clipboard-multiselect_20260227, 2026-02-27)

---

- Use `SkimMatcherV2` from `fuzzy-matcher` for fuzzy search — returns `(score, Vec<usize>)` indices for highlighting (from: fuzzy-search_20260228, 2026-02-28)
- `invalidate_search_cache()` must be called after ALL tree mutations (create, rename, delete, expand, toggle_hidden, paste) (from: fuzzy-search_20260228, 2026-02-28)
- `flatten_node_filtered` recurses children first to decide parent inclusion — parent appears only if it or a descendant matches (from: fuzzy-search_20260228, 2026-02-28)
- `fuzzy_matcher::FuzzyMatcher` trait must be imported for `fuzzy_indices` method (from: fuzzy-search_20260228, 2026-02-28)

- `notify-debouncer-mini` v0.5 callback type is `Result<Vec<DebouncedEvent>, notify::Error>` (not `Vec<Error>`) — must annotate closure explicitly for type inference (from: file-watcher_20260228, archived 2026-02-28)
- `FsWatcher` uses `Arc<AtomicBool>` for pause/resume — keeps inotify watches alive, avoids expensive re-registration (from: file-watcher_20260228, archived 2026-02-28)
- State preservation on tree refresh: capture (selected path, scroll, expanded set) → reload subtrees → restore_expanded → flatten → restore selection by path lookup → clamp scroll (from: file-watcher_20260228, archived 2026-02-28)
- `handle_fs_change()` deduplicates parent directories before reloading to avoid redundant I/O (from: file-watcher_20260228, archived 2026-02-28)
- Watcher sync in main loop: compare `app.watcher_active` vs `watcher.is_active()` each iteration to keep them in sync (from: file-watcher_20260228, archived 2026-02-28)
- Graceful degradation for optional subsystems: wrap initialization in match, set state flag to false, show status message on error (from: file-watcher_20260228, archived 2026-02-28)

- All config fields use `Option<T>` so partial configs from different sources compose cleanly via `.or()` merge (from: config-polish_20260228, archived 2026-02-28)
- `#[serde(default)]` on both struct and fields ensures TOML parsing tolerates missing sections (from: config-polish_20260228, archived 2026-02-28)
- Widget builder pattern: `WidgetName::new(state, theme).block(block)` — theme is always the last constructor parameter (from: config-polish_20260228, archived 2026-02-28)
- Clone ThemeColors at render start to avoid borrow checker conflicts with `app` mutation during rendering (from: config-polish_20260228, archived 2026-02-28)
- Static const arrays of structs for keybinding data — compile-time, zero allocation at runtime (from: config-polish_20260228, archived 2026-02-28)
- Store layout `Rect` on App from render → handler uses them for mouse coordinate mapping (from: config-polish_20260228, archived 2026-02-28)
- Mouse events only processed in Normal mode — prevents accidental clicks during dialogs (from: config-polish_20260228, archived 2026-02-28)
- Panic hook should also disable mouse capture to avoid terminal corruption (from: config-polish_20260228, archived 2026-02-28)
- Separate sorting from `load_children` into `TreeState::sort_children_of` — sort concerns belong to TreeState, not TreeNode (from: config-polish_20260228, archived 2026-02-28)
- Clone sort fields (sort_by, dirs_first) before `find_node_mut` to avoid borrow checker conflict on `&mut self` (from: config-polish_20260228, archived 2026-02-28)
- `SortBy::next()` enum cycling — clean pattern without index arithmetic for mode cycling (from: config-polish_20260228, archived 2026-02-28)
- Must account for border offset (y+1) when mapping mouse click row to flat_items index in bordered widgets (from: config-polish_20260228, archived 2026-02-28)

Last refreshed: 2026-02-28 (config-polish_20260228 archived)
