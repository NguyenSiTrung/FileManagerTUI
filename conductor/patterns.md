# Codebase Patterns

Reusable patterns discovered during development. **Read this before starting new work.**

This file is the project's institutional knowledge - learnings extracted from completed tracks that help future development.

---

## Code Conventions

- Use `#[allow(dead_code)]` on struct fields/variants reserved for future milestones (from: core-loop-tree_20260227, 2026-02-27)

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

## Gotchas

- Root node must always bypass hidden filter in flatten — tempfile and some paths start with `.` prefix (from: core-loop-tree_20260227, 2026-02-27)
- crossterm event polling is blocking — must run in spawned tokio task with mpsc channel (from: core-loop-tree_20260227, 2026-02-27)
- AppMode can no longer derive `Copy` once DialogKind contains heap types (PathBuf, Vec, String) (from: file-ops-dialogs_20260227, 2026-02-27)
- Must prevent delete on root node — check `depth > 0` (from: file-ops-dialogs_20260227, 2026-02-27)
- Must clone DialogKind before matching to avoid borrow conflicts with `app` (from: file-ops-dialogs_20260227, 2026-02-27)
- `detect_syntax_name` returns `&str` with lifetime tied to argument — bind format! result to a let before passing (from: preview-panel_20260227, 2026-02-27)
- `.ipynb` is in the extension-to-syntax map as "Python" — must check for notebook _before_ normal file loading in update_preview (from: preview-panel_20260227, 2026-02-27)

## Testing

- Use `tempfile::TempDir` for filesystem tests; create helper `setup_test_dir()` or `setup_app()` for reuse (from: core-loop-tree_20260227, 2026-02-27)

## Context

- Tree widget builds box-drawing prefix by walking ancestor chain backwards to determine `│` vs space continuation lines (from: core-loop-tree_20260227, 2026-02-27)

---

Last refreshed: 2026-02-27 (preview-panel_20260227 complete)
