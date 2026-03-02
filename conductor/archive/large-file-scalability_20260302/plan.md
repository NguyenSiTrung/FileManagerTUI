# Plan: Large File & Directory Scalability

## Phase 1: Streaming Preview (Core Fix)
*Files: `src/preview_content.rs`, `src/config.rs`*

- [x] Task 1: Implement `read_head_lines()` — read first N lines via `BufReader::lines().take(n)`, return `Vec<String>`
  - [x] Write unit test: verify only N lines returned for file with 10K lines
  - [x] Write unit test: file smaller than N lines returns all lines

- [x] Task 2: Implement `read_tail_lines()` — seek to end of file, scan backward in chunks for last M newlines, return `Vec<String>`
  - [x] Write unit test: verify only last M lines returned for file with 10K lines
  - [x] Write unit test: file smaller than M lines returns all lines
  - [x] Write unit test: handles files without trailing newline

- [x] Task 3: Rewrite `load_head_tail_content()` to use `read_head_lines()` + `read_tail_lines()` instead of `reader.lines().collect()`
  - [x] ViewMode::HeadAndTail — call both, join with separator
  - [x] ViewMode::HeadOnly — call only `read_head_lines()`
  - [x] ViewMode::TailOnly — call only `read_tail_lines()`
  - [x] Write integration test: 10MB temp file, verify memory-bounded behavior
  - [x] Verify existing preview tests pass

- [x] Task 4: Conductor - User Manual Verification 'Streaming Preview' (Protocol in workflow.md)

## Phase 2: Editor Hard Block
*Files: `src/app.rs`, `src/config.rs`*

- [x] Task 1: Add constants `DEFAULT_MAX_EDITOR_BYTES` (10MB) and `DEFAULT_MAX_EDITOR_LINES` (100_000) to `config.rs`
  - [x] Add configurable fields to `AppConfig` with validation

- [x] Task 2: Add size + line count guard in `enter_edit_mode()` — check file size first (cheap), then `fast_line_count()` only if size passes, refuse with clear message if either exceeds threshold
  - [x] Write test: file >10MB is refused
  - [x] Write test: file with >100K lines is refused
  - [x] Write test: file under both limits opens normally

- [x] Task 3: Conductor - User Manual Verification 'Editor Hard Block' (Protocol in workflow.md)

## Phase 3: Async Directory Expansion
*Files: `src/app.rs`, `src/fs/tree.rs`, `src/event.rs`, `src/main.rs`*

- [x] Task 1: Add `NodeType::Loading` variant and "Loading..." virtual node support in `flatten_node()`
  - [x] Flatten emits a "Loading..." FlatItem when node is expanded but children are `None` and `is_loading` flag is set
  - [x] Write test: loading node produces single "Loading..." flat item

- [x] Task 2: Add `is_loading` flag to `TreeNode`, modify `expand_selected()` with threshold check
  - [x] If entry count (from cached `total_child_count` or quick `read_dir` peek) ≤ page_size: sync expand (current path)
  - [x] If > page_size or unknown: set `is_loading = true`, flatten to show "Loading...", call `spawn_async_dir_scan()`
  - [x] Write test: small dir uses sync expand
  - [x] Write test: large dir sets loading state

- [x] Task 3: Update `handle_dir_scan_complete()` to apply snapshot, load first page, clear `is_loading`, re-flatten
  - [x] Write test: scan complete replaces loading node with children

- [x] Task 4: Conductor - User Manual Verification 'Async Directory Expansion' (Protocol in workflow.md)

## Phase 4: Async Directory Preview
*Files: `src/app.rs`, `src/preview_content.rs`*

- [x] Task 1: Replace sync `load_directory_summary()` call in `update_preview()` with `spawn_async_dir_summary()`
  - [x] Show immediate placeholder: "Directory: name\nScanning..."
  - [x] `handle_dir_summary_update()` updates preview_state with incremental counts
  - [x] On `done: true`, finalize the preview content

- [x] Task 2: Add stale-result guard — if `preview_state.current_path` changed by the time async result arrives, discard it
  - [x] (Already implemented in existing `handle_dir_summary_update`)

- [x] Task 3: Conductor - User Manual Verification 'Async Directory Preview' (Protocol in workflow.md)

## Phase 5: Integration Testing & Cleanup
*Files: across all modified files*

- [x] Task 1: Run full test suite, fix any regressions (454 tests pass)
- [x] Task 2: Run `cargo clippy -- -D warnings` and `cargo fmt --check`, fix issues (clean)
- [x] Task 3: sync `load_directory_summary` kept as fallback for tests without event_tx
- [x] Task 4: Conductor - User Manual Verification 'Integration Testing & Cleanup' (Protocol in workflow.md)
