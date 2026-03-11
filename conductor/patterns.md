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
- Every `load_children()` call MUST be followed by `sort_children_of()` — this is the canonical pattern; `sort_children_of_pub` enables callers outside TreeState (from: sort-order-fix_20260228, 2026-02-28)

- `portable-pty` for cross-platform PTY creation; `MasterPty` trait allows resize and writer/reader cloning (from: terminal-panel_20260228, archived 2026-02-28)
- PTY reader must use `spawn_blocking` (not `spawn`) because `Read` is blocking I/O — then bridge to async via mpsc channel (from: terminal-panel_20260228, archived 2026-02-28)
- VTE `Performer` struct must be separated from `TerminalEmulator` to avoid borrow checker issues — `vte::Parser::advance()` needs `&mut` on both parser and performer simultaneously (from: terminal-panel_20260228, archived 2026-02-28)
- Terminal input must be routed BEFORE general global keys in handler — `q` should type 'q' in terminal, not quit the app (from: terminal-panel_20260228, archived 2026-02-28)
- `key_event_to_bytes()` converts crossterm KeyEvents to VT100/xterm byte sequences for PTY input (from: terminal-panel_20260228, archived 2026-02-28)
- Tab must be forwarded to PTY for shell autocompletion — do NOT intercept it for focus cycling when terminal is focused (from: terminal-panel_20260228, archived 2026-02-28)
- Conditional vertical layout: `[main, terminal, status]` when terminal visible, `[main, status]` when hidden — use `Constraint::Length` for terminal rows from height_percent (from: terminal-panel_20260228, archived 2026-02-28)
- PTY resize notification must be sent alongside emulator `resize()` on every layout change to keep grid and PTY in sync (from: terminal-panel_20260228, archived 2026-02-28)

- Modifier checks require `contains()` with explicit `!contains(SHIFT)` to distinguish Ctrl+Arrow from Ctrl+Shift+Arrow — crossterm CONTROL|SHIFT is a combined bitflag (from: focus-nav-remap_20260228, archived 2026-02-28)
- All Ctrl+Arrow and Ctrl+Shift+Arrow must be intercepted BEFORE the terminal key forwarding check in `handle_normal_mode`, otherwise they get forwarded as PTY input (from: focus-nav-remap_20260228, archived 2026-02-28)
- Reserved-keys block at the top of `handle_normal_mode` is the correct place for global intercepts — runs before the terminal focus check (from: focus-nav-remap_20260228, archived 2026-02-28)

- Selection state uses `Option<(line, col)>` anchor — set on Shift+Arrow start, cleared on non-shift movement or Escape (from: editor-selection, 2026-02-28)
- Editor mouse click maps screen coordinates to buffer position using `area.x + gutter_width` offset and `scroll_offset + row` for line — same border-offset pattern as tree widget (from: editor-mouse, 2026-02-28)
- Mouse drag selection: set anchor on MouseDown, extend selection on MouseDrag event, clear on next non-shift keystroke (from: editor-mouse, 2026-02-28)
- Shift+Arrow extends selection by updating cursor while keeping anchor fixed — Ctrl+A sets anchor=(0,0) cursor=(last_line, last_col) (from: editor-selection, 2026-02-28)
- Editor theme colors for selection highlight (`editor_selection_bg`) added alongside existing cursor/match colors (from: editor-selection, 2026-02-28)

- Two-state overlay transition (Search → SearchAction) preserves search query when going back via Esc — reusable pattern for multi-step overlays (from: search-action_20260228, archived 2026-03-01)
- Clone SearchActionState before match in handler to avoid borrow conflicts — same pattern as DialogKind clone (from: search-action_20260228, archived 2026-03-01)
- `enter_edit_mode()` requires `update_preview()` first since it reads `preview_state.current_path` — order-dependent state setup (from: search-action_20260228, archived 2026-03-01)
- `detect_file_type` must handle directory case before checking binary — directories are never binary (from: search-action_20260228, archived 2026-03-01)
- Action filtering at both handler level (guards on key matches) and widget level (dynamic action list) for context-aware menus (from: search-action_20260228, archived 2026-03-01)

- Backward newline scanning for tail-lines: skip trailing `\n`, then find N newline boundaries in 64KB chunks — O(N) memory regardless of file size (from: large-file-scalability_20260302, 2026-03-02)
- Check file size (cheap metadata) before `fast_line_count()` (reads file) when guarding editor — avoid unnecessary I/O (from: large-file-scalability_20260302, 2026-03-02)
- Adding new `NodeType` variants requires updating ALL exhaustive matches: tree widget, ui.rs status bar, handler.rs (from: large-file-scalability_20260302, 2026-03-02)
- Store `event_tx: Option<mpsc::UnboundedSender<Event>>` on App to enable async ops from non-handler contexts (from: large-file-scalability_20260302, 2026-03-02)
- Keep sync fallback when `event_tx` is None for test contexts that don't have a runtime event loop (from: large-file-scalability_20260302, 2026-03-02)

- Tab-aware overlay architecture: pass full state struct to widget rather than individual fields — allows conditional rendering per tab without changing constructor signature (from: help-settings_20260302, archived 2026-03-02)
- Lazy initialization for expensive state: use `Option<State>` and initialize on first demand (e.g., first tab switch) to avoid unnecessary work (from: help-settings_20260302, archived 2026-03-02)
- Live config application: apply changes to running `AppConfig` immediately then serialize to disk; match on `(section, key)` tuples for clean dispatch; handle side effects (re-sort, re-flatten, re-resolve theme) explicitly (from: help-settings_20260302, archived 2026-03-02)
- TOML merge strategy: read existing file → parse to `toml::Table` → merge modified entries → write back; preserves comments and hand-edited values (from: help-settings_20260302, archived 2026-03-02)
- Entry line index computation for auto-scroll: iterate through all entries counting section headers and separators for variable-height lists (from: help-settings_20260302, archived 2026-03-02)
- Type bridge pattern (`SettingValueKind`): intermediate enum bridging heterogeneous config field types to a uniform UI/editing API — enables generic toggle/cycle/edit without knowing specific field type (from: help-settings_20260302, archived 2026-03-02)

- Shallow-first → on-demand-deep pattern: default to cheap depth-1 counting for directory preview, offer explicit user action (`D` key) for full recursive scan — keeps UI responsive while preserving power-user access (from: shallow-dir-preview_20260302, archived 2026-03-02)
- Wrap async directory scans with `tokio::time::timeout` using configurable `preview_timeout_ms` — abort and show partial results on timeout rather than blocking indefinitely (from: shallow-dir-preview_20260302, archived 2026-03-02)
- Cancel-on-navigate: store `Arc<AtomicBool>` cancel token on App, signal it in `update_preview()` before launching new scan — prevents stale background scans from wasting resources (from: shallow-dir-preview_20260302, archived 2026-03-02)
- Use `is_shallow_preview` flag on preview state for conditional UI hints (e.g., "[D] Deep scan") — avoids showing deep-scan prompt when results are already full (from: shallow-dir-preview_20260302, archived 2026-03-02)
- Theme changes in Settings must update both color palette (`theme_colors`) and syntect theme (`syntax_theme`); set `last_previewed_index = None` to force preview repaint on next render (from: light-preview-contrast_20260302, archived 2026-03-02)
- Use `config.syntax_theme_name(config.theme_scheme())` as the single source of truth for preview syntax theme resolution (from: light-preview-contrast_20260302, archived 2026-03-02)
- Keep preview rendering APIs explicit by passing both syntect `Theme` (syntax colors) and UI `ThemeColors` (semantic panel colors) (from: light-preview-contrast_20260302, archived 2026-03-02)
- Bulk regex refactors across function calls can accidentally touch function declarations; re-run targeted signature validation with `rg` immediately after (from: light-preview-contrast_20260302, archived 2026-03-02)
- `spawn_async_dir_summary_shallow` needs cloned `ThemeColors` moved into `spawn_blocking` for directory preview styling consistency (from: light-preview-contrast_20260302, archived 2026-03-02)

- Viewport vs Selection scrolling: mouse wheel moves `scroll_offset` (viewport) independently from `selected_index` (selection); keyboard nav uses `update_scroll()` to keep selection visible by adjusting scroll_offset (from: tree-scroll_20260303, archived 2026-03-03)
- Scrollbar rendering: only show when `total_items > visible_height`; reduce content width by 1 when present; proportional thumb size `(visible² / total).max(1)`; use `█` for thumb, `░` for track (from: tree-scroll_20260303, archived 2026-03-03)
- Scrollbar mouse interaction: click-to-jump maps click row to scroll offset via `inner_y * max_scroll / visible_height`; drag-to-scroll uses `scrollbar_dragging` flag to prevent event leak to other handlers (from: tree-scroll_20260303, archived 2026-03-03)
- Config `Option<u16>` with clamp range for scroll settings; `UInt` kind in settings panel for inline numeric editing; live-apply maps value directly to app config field (from: tree-scroll_20260303, archived 2026-03-03)

- Terminal mouse selection uses bordered-panel offset mapping (`y+1`, `x+1`) to convert screen coordinates to emulator grid positions (from: terminal-mouse-copy_20260302, archived 2026-03-03)
- OSC 52 clipboard fallback: encode text as base64, write `\x1b]52;c;<base64>\x07` to stdout for headless/web terminal clipboard access — degrades gracefully when unsupported (from: terminal-mouse-copy_20260302, archived 2026-03-03)
- Ctrl+Shift+C matching requires `modifiers.contains(CONTROL | SHIFT)` — crossterm combines modifier flags; explicit bitflag checks prevent false positives with plain Ctrl+C (from: terminal-mouse-copy_20260302, archived 2026-03-03)
- Preview view-mode mouse selection reuses the same coordinate-mapping and selection-rendering patterns as terminal and editor panels (from: terminal-mouse-copy_20260302, archived 2026-03-03)

- `load_highlighted_content` reads entire file with `fs::read` — no internal size guard; caller gates via `update_preview` (from: preview-hardening_20260304, archived 2026-03-04)
- `highlight_single_line` and `load_highlighted_content` share duplicate span-building logic — candidate for extraction (from: preview-hardening_20260304, archived 2026-03-04)
- `load_directory_summary` (sync deep scan) lacks `VisitedDirs` symlink-loop protection that `spawn_async_dir_summary` has (from: preview-hardening_20260304, archived 2026-03-04)
- `total_lines` in `load_head_tail_content` returns displayed line count, not actual file line count (from: preview-hardening_20260304, archived 2026-03-04)
- `read_head_lines` uses `map_while(Result::ok)` which silently drops I/O errors (from: preview-hardening_20260304, archived 2026-03-04)
- `line_wrap` field exists on `PreviewState` but is never read by `PreviewWidget::render` — dead code (from: preview-hardening_20260304, archived 2026-03-04)
- `Ctrl+W` handler toggles `line_wrap` state but has no visual effect — dead code (from: preview-hardening_20260304, archived 2026-03-04)
- `epoch_days_to_date` casts `u64` to `i64` without bounds checking (from: preview-hardening_20260304, archived 2026-03-04)

- `CopyOverlay` AppMode: disable mouse capture → render text overlay → user selects with browser-native Ctrl+C → any key dismisses and re-enables mouse; used for web terminals (xterm.js) where OSC 52 is unsupported (from: clipboard-ux standalone commits, 2026-03-03–04)
- Async clipboard copy: spawn `tokio::spawn` task for `copy_selected_text` / `copy_path` with result sent via `event_tx` channel — prevents UI freeze on slow clipboard IPC (from: 80243ef, 2026-03-04)
- `open_terminal_at_selected()`: resolve selected tree item's parent dir, show terminal panel if hidden, send `cd <dir> && clear` bytes to PTY — reuses existing PTY write path (from: c3f0924, 2026-03-03)

- Double-click detection: `Option<(Instant, u16, u16)>` with `.take()` for one-shot state consumption — `.take()` atomically reads and clears, preventing stale state (from: preview-dblclick_20260304, archived 2026-03-05)
- Line content length from `Line<'static>` spans: `l.spans.iter().map(|s| s.content.len()).sum::<usize>()` — accounts for multi-span syntax-highlighted content (from: preview-dblclick_20260304, archived 2026-03-05)
- Double-click detection must use screen coordinates (col, row) not content coordinates — screen coords are stable across scrolls while content coords shift (from: preview-dblclick_20260304, archived 2026-03-05)
- `set_anchor` + `set_endpoint` (without `begin_drag`) creates a non-dragging programmatic selection — useful for selections that shouldn't be cleared by mouse-up anchor==endpoint logic (from: preview-dblclick_20260304, archived 2026-03-05)

- `syntect` `load_defaults_newlines()` expects each line to end with `\n`; editor buffer stores lines without trailing newlines, causing the parser to carry incorrect scope contexts (e.g. unterminated comment scopes from `#`) into all subsequent lines — must append `\n` before `highlight_line()` and filter newline chars in render output (from: standalone fix f9658df, 2026-03-04)

- S3 runtime state (`s3_backend`, `s3_config`) stored on App struct, not in AppConfig — it's runtime state not user configuration (from: s3-browse_20260310, archived 2026-03-10)
- Virtual TreeNode construction without `fs::metadata` by directly setting struct fields (no `TreeNode::new()`) — used for S3 entries that have no local filesystem metadata (from: s3-browse_20260310, archived 2026-03-10)
- `TreeState::find_node_mut` is private; use `find_node_mut_pub` for access from app.rs (from: s3-browse_20260310, archived 2026-03-10)
- `aws s3 ls` output uses leading whitespace before `PRE` — parser must handle both trimmed and untrimmed input (from: s3-browse_20260310, archived 2026-03-10)
- S3 paths stored as `PathBuf` (`PathBuf::from(s3_uri)`) for compatibility with existing tree code that expects `PathBuf` throughout (from: s3-browse_20260310, archived 2026-03-10)
- `S3ListingComplete` event for both initial and subdirectory listings; routing in main.rs checks `is_root` flag to dispatch correctly (from: s3-browse_20260310, archived 2026-03-10)

- `wrap_text()` char-boundary string wrapping for long URIs/paths in preview panels — splits at char boundaries (not word boundaries) since URIs have no natural breaks; uses `preview_area.width` for dynamic wrap width (from: S3 preview fix 9efcdb5, 2026-03-10)
- `Y` key in `handle_preview_keys` for clipboard copy — mirrors tree-panel `y` behavior for consistent UX across panels (from: S3 preview fix 9efcdb5, 2026-03-10)

- Use `std::result::Result<T, E>` explicitly when the crate has a custom `Result<T>` type alias — avoids confusing type inference errors (from: s3-head-preview_20260311, archived 2026-03-11)
- `highlight_content_from_string()` reuses `detect_syntax_name` + `highlight_single_line` for in-memory content — same pipeline as file-based highlighting (from: s3-head-preview_20260311, archived 2026-03-11)
- Shell pipe via `sh -c 'aws s3 cp ... - | head -n N'` is cleanest way to stream first N lines without downloading full file (from: s3-head-preview_20260311, archived 2026-03-11)
- Binary detection for in-memory content needs `content.as_bytes()[..check_len].contains(&0)` — same pattern as file-based binary check (from: s3-head-preview_20260311, archived 2026-03-11)
- S3 head state (active/loading/content/uri) lives on App struct alongside s3_backend/s3_config — reset on file navigation change in update_preview() (from: s3-head-preview_20260311, archived 2026-03-11)
- Config test pattern: default → TOML parse → merge → CLI load with explicit path (from: s3-head-preview_20260311, archived 2026-03-11)

- Adding new ThemeColors fields follows a 3-file pattern: struct field in theme.rs → defaults in dark_theme()/light_theme() → Option<String> in config.rs ThemeColorsConfig → apply_custom_colors() entry in theme.rs (from: s3-tree-colors_20260311, 2026-03-11)
- Border style override per-mode: compute a mode-specific focused style variable and substitute it in the border tuple destructuring in ui.rs — pattern for any mode-contextual border tinting (from: s3-tree-colors_20260311, 2026-03-11)

Last refreshed: 2026-03-11 (S3 tree colors patterns elevated)
