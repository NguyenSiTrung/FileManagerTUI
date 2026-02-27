# Spec: Preview Panel + Syntax Highlighting

## Overview

Add a file preview panel to the right side of the TUI, displaying syntax-highlighted file contents alongside the folder tree. The preview automatically updates as the user navigates the tree. Supports text files (with syntax highlighting), large files (head+tail mode), binary files (metadata display), directories (summary), and Jupyter notebooks (cell rendering).

## Functional Requirements

### FR-1: Split Layout
- `ui.rs` renders a horizontal split: tree panel (left, 40%) and preview panel (right, 60%).
- Both panels have bordered blocks with titles.
- The status bar remains at the bottom spanning full width.

### FR-2: Text File Preview with Syntax Highlighting
- Display file contents with line numbers in the preview panel.
- Use `syntect` for syntax highlighting.
- Theme is loaded from config (`syntax_theme` field in `config.toml`) with fallback to `base16-ocean.dark`.
- Syntax language is detected by file extension (see PLAN.md §7.2 mapping) with shebang fallback for extensionless files.

### FR-3: Large File Head+Tail Mode
- Files exceeding `max_full_preview_bytes` (default 1MB, configurable) use head+tail mode.
- Show configurable `head_lines` (default 50) from the top and `tail_lines` (default 20) from the bottom.
- Display an "N lines omitted" separator between head and tail sections.
- Line counting uses fast byte-scanning (64KB chunks), not full file read.
- Cache line counts per file path.
- Support toggling view mode: head+tail / head-only / tail-only via `Ctrl+T`.
- `+`/`-` keys adjust head/tail line counts by 10.

### FR-4: Binary File Metadata Display
- Detect binary files using a known extension list (`.pt`, `.pth`, `.h5`, `.hdf5`, `.pkl`, `.pickle`, `.onnx`, `.zip`, `.tar`, `.gz`, `.bz2`, `.xz`, `.so`, `.dylib`, `.exe`, `.bin`, `.img`, `.iso`) plus null-byte detection in the first 8KB as fallback for unknown extensions.
- Display: file name, size (human-readable), modification time, permissions.
- Show "[Binary file — cannot preview]" message.

### FR-5: Directory Summary
- When a directory is selected, show: directory name, total file count, total subdirectory count, total size (recursive).
- Keep it lightweight — compute on demand, not recursively for huge trees.

### FR-6: Jupyter Notebook Cell Rendering
- Parse `.ipynb` files as JSON.
- Render each cell with a header showing cell index and type (`markdown`, `code`, `raw`).
- Display cell source code (with syntax highlighting for code cells using the notebook's kernel language).
- Display text outputs (stdout/stderr) below cell source, prefixed with `[Out]`.
- Skip rich outputs (images, HTML, display_data).
- Show notebook metadata in the preview title (e.g., "Notebook: 12 cells").

### FR-7: Focus & Scrolling
- `Tab` key switches focus between tree and preview panels.
- Focused panel has a highlighted border.
- When preview is focused: `j/k` scroll line-by-line, `g/G` jump to top/bottom, `Ctrl+D/Ctrl+U` for half-page scroll, `Ctrl+W` toggles line wrap.
- Preview **auto-updates** when tree selection changes regardless of focus. Scroll position resets on selection change.

### FR-8: Preview State
- `PreviewState` struct holds: current file path, content lines (styled), scroll offset, view mode, line wrap toggle, total line count.
- Preview content is loaded when selected item changes.
- No preview for items that fail to read (show error message in panel).

## Non-Functional Requirements

- Preview loading must not block the UI — use async for large file reads if needed.
- Memory: only hold the visible portion + head/tail lines in memory for large files.
- Respect existing code conventions: `StatefulWidget` pattern, mode-based handler dispatch, `TreeState` ownership model.

## Acceptance Criteria

1. Launching `fm /path` shows a split layout with tree (left) and preview (right).
2. Selecting a `.rs`, `.py`, `.toml` file shows syntax-highlighted content with line numbers.
3. Selecting a file > 1MB shows head+tail preview with omitted line count.
4. Selecting a `.pt` or `.zip` file shows binary metadata.
5. Selecting a directory shows file/dir count and total size.
6. Selecting a `.ipynb` file shows cell-by-cell rendering with source and text outputs.
7. `Tab` switches focus; scroll keys work in preview when focused.
8. `Ctrl+T` toggles view mode; `+`/`-` adjusts line counts.
9. Syntax theme is read from config with fallback.
10. All existing tests continue to pass; new tests cover preview logic.

## Out of Scope

- Image preview / sixel rendering
- File editing within the preview panel
- Async streaming preview (loading indicator)
- Config file hot-reload
- Rich notebook output rendering (images, HTML, LaTeX)
