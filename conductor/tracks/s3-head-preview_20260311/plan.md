# S3 Head Preview — Implementation Plan

## Phase 1: Configuration & Backend

- [x] Task 1: Add `s3_head_lines` to config system
  - Add `s3_head_lines: Option<usize>` to `PreviewConfig` struct
  - Add `DEFAULT_S3_HEAD_LINES: usize = 100` constant
  - Add `s3_head_lines()` convenience getter
  - Add merge logic in `AppConfig::merge()`
  - Add unit tests for default, TOML parsing, and merge override

- [x] Task 2: Add `--s3-head-lines` CLI flag
  - Add `s3_head_lines: Option<usize>` to `Cli` struct with `#[arg(long)]`
  - Wire into `as_config_overrides()` → `preview.s3_head_lines`

- [x] Task 3: Add `S3Backend::stream_head()` method
  - Implement async method: spawns `aws s3 cp <uri> - | head -n <N>` via shell pipe
  - Must respect `--profile` flag
  - Returns `Result<String, String>` (content or error)
  - Handle binary content detection (null bytes → error)
  - Add unit test for command construction (mock test)

## Phase 2: App State & Event Wiring

- [x] Task 1: Add S3 head preview state to `App`
  - Add `s3_head_active: bool` field to track toggle state
  - Add `s3_head_loading: bool` field for loading indicator
  - Add `s3_head_content: Option<Vec<Line<'static>>>` for cached rendered lines
  - Add `s3_head_uri: Option<String>` to track which file the head is for
  - Reset `s3_head_active` when navigating to a different file

- [x] Task 2: Add `S3HeadComplete` event variant
  - Add to `Event` enum: `S3HeadComplete { s3_uri: String, content: Result<String, String> }`
  - Handle event in main loop (`main.rs`): call `app.handle_s3_head_complete()`

- [x] Task 3: Implement `App::spawn_s3_head()`
  - Async spawn: clone backend, parse S3 path from URI, call `stream_head()`
  - Send `S3HeadComplete` event on completion
  - Set `s3_head_loading = true` and display loading text in preview

- [x] Task 4: Implement `App::handle_s3_head_complete()`
  - On success: syntax-highlight content using existing `preview_content` helpers, set `s3_head_active = true`, update preview panel
  - On error: show error in preview, reset `s3_head_loading`, keep metadata view
  - Reset `s3_head_loading = false`

## Phase 3: Key Handler & Preview Integration

- [x] Task 1: Add `H` key handler in S3 mode
  - In `handler.rs`, S3 file context: handle `KeyCode::Char('h') | KeyCode::Char('H')`
  - If `s3_head_active`: toggle back to metadata view (set `false`, call `update_preview()`)
  - If not active: call `app.spawn_s3_head(event_tx)`
  - Ignore if current selection is a directory

- [x] Task 2: Update `update_preview()` S3 file branch
  - When `s3_head_active == true` and preview content exists, render the head content instead of metadata
  - Show header line: `"☁ S3 Head (N lines) — filename  [H to close]"`
  - Include line numbers
  - Reset `s3_head_active` on file selection change

- [x] Task 3: Update help overlay
  - Add `H` keybinding to S3 mode section in help overlay text
  - Description: "Head preview (first N lines)"

## Phase 4: Testing & Polish

- [x] Task 1: Unit tests for `stream_head` command building
  - Test with/without profile
  - Test binary content detection

- [ ] Task 2: Integration test for config + CLI override
  - Test `s3_head_lines` from config file
  - Test `--s3-head-lines` CLI flag overrides config

- [ ] Task 3: End-to-end manual verification
  - Test on real S3 bucket with text files (.rs, .py, .json, .log)
  - Test on binary files (images)
  - Test toggle H on/off
  - Test with custom `--s3-head-lines` value

- [ ] Task: Conductor - User Manual Verification 'Testing & Polish' (Protocol in workflow.md)
