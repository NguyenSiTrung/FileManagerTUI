# Plan: Non-Blocking Directory Preview with Shallow Counting

## Phase 1: Shallow Directory Summary (Core)

- [x] Task 1: Create `load_directory_summary_shallow` in `preview_content.rs`
  - Add new function that only counts immediate children (depth 1, single `read_dir`)
  - Return: dir name, file count, subdir count, first 20 child names as listing
  - Show "(direct)" label to indicate shallow counts
  - Add unit tests for shallow summary (empty dir, small dir, large dir)

- [x] Task 2: Update sync fallback in `app.rs` to use shallow summary
  - Replace `load_directory_summary` call with `load_directory_summary_shallow` in `update_preview` sync path
  - Ensure existing tests still pass with new summary format

- [x] Task 3: Create shallow async scan variant in `app.rs`
  - Add `spawn_async_dir_summary_shallow` that does depth-1 only scan
  - Send a single `DirSummaryUpdate` event on completion (no progressive updates needed for shallow)
  - Replace `spawn_async_dir_summary` call in `update_preview` with shallow variant
  - Add "[Press D for deep scan]" hint line to preview output

- [ ] Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

## Phase 2: Scan Cancellation

- [x] Task 1: Add cancel token to async directory scans
  - Add `Arc<AtomicBool>` field `dir_scan_cancel` to `App` struct
  - Pass cancel token to both shallow and deep scan tasks
  - Check cancel token every iteration in scan loops; abort if set
  - Add unit test for cancel token behavior

- [x] Task 2: Cancel-on-navigate in `update_preview`
  - At the start of `update_preview`, if `active_dir_scan` is set and path changed, set cancel token
  - Reset cancel token before spawning new scan
  - Test: verify old scan is cancelled when navigating to new item

- [ ] Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)

## Phase 3: Preview Timeout

- [x] Task 1: Add `preview_timeout_ms` config option
  - Add field to `PreviewConfig` in `config.rs` (default: 2000ms)
  - Add accessor method `preview_timeout_ms()` on `AppConfig`
  - Add to settings panel entries in `app.rs`
  - Test config loading with and without the new field

- [x] Task 2: Implement timeout mechanism for async scans
  - Use `tokio::time::timeout` wrapping the scan task
  - On timeout: send `DirSummaryUpdate` with `done: true` and partial results
  - Show "⚠ Scan timed out (directory too large)" message in preview
  - For deep scans, display partial counts collected before timeout
  - Test timeout behavior with mock slow scan

- [ ] Conductor - User Manual Verification 'Phase 3' (Protocol in workflow.md)

## Phase 4: Deep Scan on Demand

- [x] Task 1: Add `D` key handler for deep scan trigger
  - In `handler.rs`, add `KeyCode::Char('D')` (or `'d'`) handler in preview-focused normal mode
  - Only trigger when preview shows a directory with shallow results
  - Call existing `spawn_async_dir_summary` (full recursive) with cancel token + timeout
  - Add state field `is_shallow_preview: bool` to `PreviewState` to track scan type

- [ ] Task 2: Deep scan progressive UI updates
  - Replace "[Press D for deep scan]" hint with "⏳ Deep scanning..." during scan
  - Show progressive file/dir/size counts as `DirSummaryUpdate` events arrive
  - On completion, show full recursive counts + total size
  - On cancel/timeout, show partial results + warning message
  - Test: deep scan updates preview progressively

- [ ] Conductor - User Manual Verification 'Phase 4' (Protocol in workflow.md)

## Phase 5: Polish & Integration

- [ ] Task 1: Loading indicator in preview panel
  - Show "⏳ Scanning..." with directory name during shallow scan
  - Show "⏳ Deep scan in progress..." with running counts during deep scan
  - Ensure indicator clears on scan complete, cancel, or timeout

- [ ] Task 2: Integration testing and edge cases
  - Test navigation rapid-fire (quick up/down through many directories)
  - Test deep scan cancel by navigating away
  - Test timeout with configurable value
  - Verify no resource leaks (file handles, spawned tasks)
  - Run `cargo clippy` and `cargo fmt`

- [ ] Conductor - User Manual Verification 'Phase 5' (Protocol in workflow.md)
