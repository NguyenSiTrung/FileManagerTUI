# Plan: Preview Panel + Syntax Highlighting

## Phase 1: Data Model & Layout Foundation

- [x] Task 1: Add `PreviewState` struct and `FocusedPanel` enum to `app.rs`
  - Add `FocusedPanel` enum (`Tree`, `Preview`)
  - Add `PreviewState` struct (current path, content lines, scroll offset, view mode, line wrap, total lines)
  - Add `ViewMode` enum (`HeadAndTail`, `HeadOnly`, `TailOnly`)
  - Add `preview_state` and `focused_panel` fields to `App`
  - Add methods: `toggle_focus()`, `preview_scroll_down/up()`, `preview_jump_top/bottom()`, `preview_half_page_down/up()`
  - Tests for new state management

- [x] Task 2: Create `components/preview.rs` skeleton and register module
  - Create `PreviewWidget` as a ratatui `Widget` with a `Block`
  - Show placeholder "No preview" text when content is empty
  - Add `pub mod preview` to `components/mod.rs`

- [x] Task 3: Split layout in `ui.rs` — tree (40%) + preview (60%)
  - Change vertical-only layout to horizontal split within the main area
  - Tree panel on left (40%), preview panel on right (60%)
  - Status bar remains full-width at bottom
  - Highlight focused panel's border with distinct style
  - Wire `PreviewWidget` rendering

- [x] Task: Conductor - Phase Verification 'Data Model & Layout Foundation' (Protocol in workflow.md)

## Phase 2: Text File Preview with Syntax Highlighting

- [x] Task 1: Implement syntax detection utility
  - Create `preview_content.rs` module (or logic within `components/preview.rs`)
  - Extension-to-language mapping per PLAN.md §7.2
  - Shebang detection fallback for extensionless files
  - Tests for extension and shebang mapping

- [x] Task 2: Implement `syntect`-based content loading
  - Load `syntect` `SyntaxSet` and `ThemeSet` (lazy-static or stored in App/PreviewState)
  - Parse file content and produce styled line spans (ratatui `Line<'_>` / `Span`)
  - Theme loaded from config field with `base16-ocean.dark` fallback
  - Handle UTF-8 and non-UTF-8 gracefully (fallback to plain text)

- [x] Task 3: Render styled preview content with line numbers
  - Update `PreviewWidget` to render line-numbered, syntax-highlighted content
  - Respect scroll offset for visible region
  - Show file name and size in preview panel title

- [x] Task 4: Wire selection change to preview update
  - In `ui.rs` or `app.rs`: detect when selected item changes, reload preview content
  - Preview auto-updates on tree selection change regardless of focus
  - Reset scroll position on content change

- [x] Task: Conductor - Phase Verification 'Text File Preview with Syntax Highlighting' (Protocol in workflow.md)

## Phase 3: Focus Management & Scroll Controls

- [x] Task 1: Wire `Tab` key for focus toggle in `handler.rs`
  - `Tab` in normal mode toggles `focused_panel` between `Tree` and `Preview`
  - Add `handle_preview_mode()` dispatch in `handle_key_event`
  - When preview focused: `j/k` scroll, `g/G` jump, `Ctrl+D/U` half-page
  - `q` and `Ctrl+C` still quit regardless of focus

- [x] Task 2: Add `Ctrl+W` line wrap toggle
  - Toggle `line_wrap` field in `PreviewState`
  - `PreviewWidget` respects wrap setting during rendering

- [x] Task: Conductor - Phase Verification 'Focus Management & Scroll Controls' (Protocol in workflow.md)

## Phase 4: Large File Head+Tail Mode

- [x] Task 1: Implement fast line counting via byte scan
  - Read file in 64KB chunks, count `\n` bytes
  - Return total line count without loading entire file
  - Cache line count per file path in `PreviewState`
  - Tests with files of various sizes

- [x] Task 2: Implement head+tail content loading
  - Read first `head_lines` and last `tail_lines` from file
  - Insert "N lines omitted" separator
  - Trigger when file size exceeds `max_full_preview_bytes` (default 1MB)
  - Configurable `head_lines` (50) and `tail_lines` (20)

- [x] Task 3: Add view mode toggle and line count adjustment
  - `Ctrl+T` cycles through `HeadAndTail`, `HeadOnly`, `TailOnly` (only when preview focused and file is large)
  - `+`/`-` adjust head/tail line counts by 10
  - Re-render preview on toggle/adjust

- [x] Task: Conductor - Phase Verification 'Large File Head+Tail Mode' (Protocol in workflow.md)

## Phase 5: Special Content Types

- [x] Task 1: Binary file detection and metadata display
  - Known binary extension list + null-byte scan in first 8KB
  - Display: filename, human-readable size, modified time, permissions
  - Show "[Binary file — cannot preview]" message
  - Tests for detection logic

- [x] Task 2: Directory summary preview
  - When selected item is a directory, show: name, file count, subdir count, total size
  - Compute on demand (non-recursive for very deep trees, cap depth or file count)
  - Tests for directory summary

- [x] Task 3: Jupyter notebook `.ipynb` cell rendering
  - Parse `.ipynb` JSON: extract cells array
  - Render cell headers with index and type (markdown/code/raw)
  - Display cell source with syntax highlighting for code cells
  - Show text outputs (stdout/stderr) with `[Out]` prefix
  - Skip rich outputs (images, HTML)
  - Show "Notebook: N cells" in preview title
  - Tests for notebook parsing

- [x] Task: Conductor - Phase Verification 'Special Content Types' (Protocol in workflow.md)

## Phase 6: Integration Testing & Polish

- [x] Task 1: Integration tests for preview functionality
  - Test split layout renders without panic
  - Test preview updates on selection change
  - Test focus toggle and scroll keys
  - Test large file head+tail mode
  - Test binary detection
  - Test directory summary
  - Test notebook rendering

- [x] Task 2: Edge cases and polish
  - Empty file preview
  - Permission-denied file preview (show error in panel)
  - Symlink file preview (follow target)
  - Very long lines (truncation when no wrap)
  - Zero-byte files
  - Run `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`

- [x] Task: Conductor - Phase Verification 'Integration Testing & Polish' (Protocol in workflow.md)
