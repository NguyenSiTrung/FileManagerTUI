# Learnings: Large File & Directory Scalability

## [2026-03-02] - Phase 1: Streaming Preview
- **Implemented:** `read_head_lines()` and `read_tail_lines()` for streaming I/O, rewrote `load_head_tail_content()` to use them
- **Files changed:** `src/preview_content.rs`
- **Learnings:**
  - Patterns: Backward newline scanning with trailing-newline skip — when file ends with `\n`, skip it before counting line boundaries to avoid off-by-one
  - Gotchas: `BufReader::lines().take(n)` is sufficient for head, but tail requires manual backward chunk scanning with `seek()`

## [2026-03-02] - Phase 2: Editor Hard Block
- **Implemented:** `DEFAULT_MAX_EDITOR_BYTES`/`DEFAULT_MAX_EDITOR_LINES` constants, `enter_edit_mode()` guards
- **Files changed:** `src/config.rs`, `src/app.rs`, `src/main.rs`
- **Learnings:**
  - Patterns: Check file size first (cheap metadata call) before `fast_line_count()` (requires reading file) — avoid unnecessary I/O
  - Gotchas: New fields in `GeneralConfig` must also be added to `main.rs::as_config_overrides()` explicit struct construction

## [2026-03-02] - Phase 3: Async Directory Expansion
- **Implemented:** `NodeType::Loading`, `is_loading` flag, `expand_selected_async()` with threshold dispatch
- **Files changed:** `src/fs/tree.rs`, `src/app.rs`, `src/handler.rs`, `src/components/tree.rs`, `src/ui.rs`
- **Learnings:**
  - Patterns: Keep sync `expand_selected()` for tests, use `expand_selected_async()` for handlers — allows non-async test contexts
  - Gotchas: Adding new NodeType variants requires updating all exhaustive matches (tree widget, ui.rs status bar, handler.rs)
  - Gotchas: Mouse handler parameter was `_event_tx` (unused) — rename to `event_tx` when it becomes used

## [2026-03-02] - Phase 4: Async Directory Preview
- **Implemented:** Async directory summary in `update_preview()` with placeholder, stored `event_tx` on App
- **Files changed:** `src/app.rs`, `src/main.rs`
- **Learnings:**
  - Patterns: Store `event_tx: Option<mpsc::UnboundedSender<Event>>` on App to enable async from non-handler contexts (like update_preview called from ui.rs)
  - Patterns: Fallback to sync when event_tx is None (tests) — preserves testability without runtime dependency
---
