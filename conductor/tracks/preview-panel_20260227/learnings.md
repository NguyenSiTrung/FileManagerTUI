# Track Learnings: preview-panel_20260227

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Use `#[allow(dead_code)]` on struct fields/variants reserved for future milestones
- TreeState owns root TreeNode + flat_items Vec + selected_index; `flatten()` rebuilds flat list from tree recursively
- App delegates tree operations to TreeState methods; handler.rs maps keys to App methods
- Module structure: fs/tree.rs (data), components/tree.rs (widget), app.rs (state), handler.rs (input), event.rs (async events), tui.rs (terminal), ui.rs (layout)
- Handler uses mode-based dispatch: `handle_normal_mode` vs `handle_dialog_mode`
- Use `Clear` widget + centered `Block` for modal overlays in ratatui
- `TreeState::reload_dir()` reloads a specific directory's children and re-flattens after file ops
- UI layout: `[Min(3), Length(1)]` vertical split for tree + status bar
- Root node must always bypass hidden filter in flatten
- crossterm event polling is blocking — must run in spawned tokio task with mpsc channel
- AppMode can no longer derive `Copy` once DialogKind contains heap types
- Must prevent delete on root node — check `depth > 0`
- Must clone DialogKind before matching to avoid borrow conflicts with `app`
- Use `tempfile::TempDir` for filesystem tests

---

<!-- Learnings from implementation will be appended below -->

## [2026-02-27] - Phase 1: Data Model & Layout Foundation
- **Implemented:** PreviewState, FocusedPanel, ViewMode in app.rs; PreviewWidget in components/preview.rs; split layout in ui.rs
- **Files changed:** src/app.rs, src/components/preview.rs, src/components/mod.rs, src/ui.rs
- **Commits:** 1c10af8, 97a6e1f, 3d92862
- **Learnings:**
  - Patterns: Use `#[allow(dead_code)]` consistently on new enums/structs/methods reserved for future phases
  - Patterns: `Layout::default().direction(Direction::Horizontal).constraints([Percentage(40), Percentage(60)])` for panel splits
  - Patterns: Use `Block.border_style()` with `Color::Cyan` for focused panel indication
  - Context: PreviewWidget follows same pattern as TreeWidget — struct with `block()` builder, implements `Widget` trait
---

## [2026-02-27] - Phase 2-4: Syntax Highlighting, Focus, Head+Tail
- **Implemented:** syntect-based highlighting, focus-aware key dispatch (Tab/j/k/g/G/Ctrl+D/U/Ctrl+W), large file head+tail with fast line counting, Ctrl+T view toggle, +/- adjustment
- **Files changed:** src/preview_content.rs, src/app.rs, src/handler.rs, src/ui.rs, Cargo.toml
- **Commits:** ac6e27e, f24a5bc, 7a58f52
- **Learnings:**
  - Patterns: Store SyntaxSet and Theme on App struct (expensive to load, reuse across previews)
  - Patterns: Use `last_previewed_index` to avoid re-loading preview on every render frame
  - Patterns: Handler uses 3-level dispatch: global keys → panel-specific keys (handle_tree_keys/handle_preview_keys) → dialog keys
  - Gotchas: syntect `find_syntax_by_extension` returns Option, chain with `find_syntax_by_name` for robust fallback
  - Gotchas: `fast_line_count` must handle files without trailing newline (check for content if newline count is 0)
  - Context: ViewMode cycling only applies when `is_large_file` is true — noop for normal files
---

## [2026-02-27] - Phase 5-6: Special Content Types + Integration & Polish
- **Implemented:** Binary file detection (extension + null-byte scan), metadata display, directory summary preview, Jupyter notebook cell rendering, integration tests, edge case tests
- **Files changed:** src/preview_content.rs, src/app.rs, src/ui.rs, Cargo.toml
- **Commits:** 1b297dd, 67d46fb, b65a0a9, f8f085b, 774e8bb
- **Learnings:**
  - Patterns: Use `vec![...]` macro instead of `Vec::new()` + `.push()` chains — clippy enforces this
  - Patterns: Binary detection: check known extensions first (fast), then null-byte scan in 8KB (fallback)
  - Patterns: Use `r##"..."##` for raw strings that contain `"#` sequences (notebook JSON with markdown headers)
  - Patterns: Use iterative stack-based directory walk with entry cap (10K) to prevent hanging on huge trees
  - Patterns: Notebook source fields can be String or Array<String> — handle both with `extract_notebook_text()`
  - Patterns: Strip ANSI escape codes from notebook error tracebacks for clean display
  - Gotchas: `detect_syntax_name` returns `&str` with lifetime tied to argument — bind format! result to a let before passing
  - Gotchas: `.ipynb` is in the extension-to-syntax map as "Python" — must check for notebook _before_ normal file loading
  - Context: `serde_json` added as dependency for notebook parsing (not using serde derive, just Value-based parsing)
---
