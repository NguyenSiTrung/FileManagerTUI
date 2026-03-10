# Plan: AWS S3 Browse Mode

## Phase 1: S3 Backend Module
<!-- execution: sequential -->

- [x] Task 1: Create S3 types and path parsing (`src/s3/mod.rs`, `src/s3/types.rs`)
  - [x] Define `S3Path` struct (bucket, prefix/key)
  - [x] Parse `s3://bucket/prefix/key` URIs into `S3Path`
  - [x] Define `S3Entry` struct (name, is_dir, size, modified)
  - [x] Define `S3Backend` struct (profile: Option<String>)
  - [x] Unit tests for URI parsing (edge cases: root bucket, trailing slash, no prefix)

- [x] Task 2: Implement `aws s3 ls` output parser (`src/s3/parser.rs`)
  - [x] Parse `PRE <name>/` lines as directory entries
  - [x] Parse `<date> <time> <size> <name>` lines as file entries
  - [x] Handle empty output (empty prefix)
  - [x] Handle error output from stderr
  - [x] Unit tests with sample `aws s3 ls` output (dirs, files, mixed, empty, errors)

- [x] Task 3: Implement async S3 listing via CLI (`src/s3/backend.rs`)
  - [x] `S3Backend::list_prefix()` — spawn `aws [--profile] s3 ls s3://...` via `tokio::process::Command`
  - [x] Capture stdout → parse with parser; capture stderr → error handling
  - [x] `S3Backend::check_cli()` — verify `aws` exists on PATH
  - [x] `S3Backend::download_to_cache()` — `aws s3 cp` to temp cache dir
  - [x] Manage temp cache directory lifecycle (`/tmp/fm-s3-cache-<pid>/`)
  - [x] Unit tests for command construction (verify args are correct)

- [x] Task 4: Add CLI flags for S3 mode (`src/config.rs`, `src/main.rs`)
  - [x] Add `--aws-profile <name>` flag to clap args
  - [x] Detect `s3://` prefix in PATH argument
  - [x] Store S3 config in `AppConfig` (s3_mode: bool, aws_profile: Option<String>, s3_path: Option<S3Path>)
  - [x] Validate `aws` CLI at startup; exit with actionable error if missing
  - [x] Tests for CLI arg parsing

- [x] Task: Conductor - User Manual Verification 'S3 Backend Module' (Protocol in workflow.md)

## Phase 2: S3 Tree Integration
<!-- execution: sequential -->

- [ ] Task 1: Add backend mode to App state (`src/app.rs`)
  - [ ] Add `BackendMode` enum (Local, S3 { backend: S3Backend })
  - [ ] Store `backend_mode` on `App` struct
  - [ ] Initialize `BackendMode::S3` when config has s3_path
  - [ ] Add `App::is_s3_mode()` helper
  - [ ] Disable filesystem watcher when in S3 mode

- [ ] Task 2: Build S3 TreeNodes from listing results (`src/fs/tree.rs`)
  - [ ] Add `TreeNode::from_s3_entry()` constructor — creates node without `fs::metadata`
  - [ ] Use S3 URI string as the `path` field (stored as PathBuf for compatibility)
  - [ ] Set `FileMeta` from `S3Entry` (size, modified, is_hidden)
  - [ ] S3 root node: `TreeNode::new_s3_root(s3_path)` — creates expandable root
  - [ ] Tests for S3 TreeNode construction

- [ ] Task 3: S3 directory expansion (`src/app.rs`, `src/fs/tree.rs`)
  - [ ] Override `expand_selected()` path for S3 mode — call `S3Backend::list_prefix()` instead of `fs::read_dir`
  - [ ] Build children from `Vec<S3Entry>` via `TreeNode::from_s3_entry()`
  - [ ] Async expansion with loading indicator (reuse `is_loading` + `DirScanComplete` event pattern)
  - [ ] Wire `DirScanComplete` handler to accept S3 listing results
  - [ ] Tests for S3 tree expansion flow

- [ ] Task 4: Disable write operations in S3 mode (`src/handler.rs`)
  - [ ] Guard `a`/`A` (create), `r` (rename), `d` (delete), `x` (cut), `p` (paste) keys
  - [ ] Show "Not available in S3 mode" status message when attempted
  - [ ] Disable `Ctrl+Z` undo in S3 mode
  - [ ] Disable inline editor (`e` key) in S3 mode
  - [ ] Disable `T` (open terminal at path) for S3 entries

- [ ] Task: Conductor - User Manual Verification 'S3 Tree Integration' (Protocol in workflow.md)

## Phase 3: S3 Preview & Clipboard
<!-- execution: sequential -->

- [ ] Task 1: S3 metadata preview (`src/app.rs`, `src/preview_content.rs`)
  - [ ] When in S3 mode + selected item is file: render metadata preview (size, date, URI, download prompt)
  - [ ] Add `load_s3_metadata_preview()` function in `preview_content.rs`
  - [ ] Generate styled `Vec<Line<'static>>` showing S3 object info
  - [ ] For S3 directories: show prefix info + child count from listing

- [ ] Task 2: On-demand download and preview (`src/app.rs`, `src/handler.rs`)
  - [ ] Enter key on S3 file → spawn async download via `S3Backend::download_to_cache()`
  - [ ] Show loading indicator in preview panel during download
  - [ ] On download complete: pipe cached file through existing `update_preview()` pipeline
  - [ ] Cache tracking: `HashMap<String, PathBuf>` mapping S3 keys → local cache paths
  - [ ] Skip re-download if already cached for this session

- [ ] Task 3: S3 URI clipboard copy (`src/app.rs`, `src/handler.rs`)
  - [ ] `y` key on S3 file → copy `s3://bucket/key` string to clipboard
  - [ ] Use existing system clipboard + OSC 52 fallback path
  - [ ] Show "📋 Copied: s3://..." status message

- [ ] Task: Conductor - User Manual Verification 'S3 Preview & Clipboard' (Protocol in workflow.md)

## Phase 4: UX Polish & Error Handling
<!-- execution: parallel -->

- [ ] Task 1: S3 status bar badge (`src/components/status_bar.rs`)
  <!-- files: src/components/status_bar.rs -->
  - [ ] Show `☁ S3 | s3://bucket` on the left section when in S3 mode
  - [ ] Replace local path display with S3 URI for selected item

- [ ] Task 2: S3 tree colors (`src/theme.rs`, `src/components/tree.rs`)
  <!-- files: src/theme.rs, src/components/tree.rs -->
  - [ ] Add `tree_s3_fg` color to `ThemeColors` (default: amber/orange #fab387)
  - [ ] Apply S3-specific color to S3 entries in tree widget rendering
  - [ ] Add S3 icon prefix: `☁` for S3 directories, `📦` for S3 objects

- [ ] Task 3: Loading indicators (`src/components/preview.rs`)
  <!-- files: src/components/preview.rs -->
  - [ ] Show `⏳ Downloading...` in preview panel during S3 file download
  - [ ] Show `⏳ Loading...` in tree during S3 directory expansion (reuse existing pattern)

- [ ] Task 4: Help overlay updates (`src/components/help.rs`)
  <!-- files: src/components/help.rs -->
  - [ ] Grey out disabled S3 keybindings with `DarkGray` color
  - [ ] Add "(S3: disabled)" suffix to write operation entries
  - [ ] Add S3-specific hints: "Enter: Download & preview", "y: Copy S3 URI"

- [ ] Task 5: Error handling polish (`src/s3/backend.rs`)
  <!-- files: src/s3/backend.rs -->
  - [ ] Parse common AWS CLI errors (ExpiredToken, AccessDenied, NoSuchBucket) into user-friendly messages
  - [ ] Show error dialog for authentication failures
  - [ ] Add retry prompt on network/timeout errors
  - [ ] Clean up temp cache directory on app exit

- [ ] Task: Conductor - User Manual Verification 'UX Polish & Error Handling' (Protocol in workflow.md)

## Phase 5: Integration & Cleanup
<!-- execution: sequential -->
<!-- depends: phase1, phase2, phase3, phase4 -->

- [ ] Task 1: Disable incompatible features (`src/app.rs`, `src/main.rs`)
  - [ ] Fuzzy search (Ctrl+P): show "Not available in S3 mode"
  - [ ] File watcher: skip initialization entirely
  - [ ] Terminal panel: allow toggle but `T` action on S3 entries shows message
  - [ ] Inline filter (`/`): works on currently loaded tree nodes (no S3 calls)

- [ ] Task 2: Temp cache cleanup and session management
  - [ ] Create unique cache dir per session: `/tmp/fm-s3-cache-<pid>/`
  - [ ] Register cleanup on app shutdown (in `main.rs` restore flow)
  - [ ] Handle Ctrl+C cleanup via existing panic hook

- [ ] Task 3: End-to-end manual testing checklist
  - [ ] Verify: `fm s3://bucket/prefix/ --aws-profile mfa` launches correctly
  - [ ] Verify: expanding S3 directories lists contents
  - [ ] Verify: file metadata preview shows without download
  - [ ] Verify: Enter downloads and shows syntax-highlighted preview
  - [ ] Verify: disabled operations show correct messages
  - [ ] Verify: status bar, colors, loading indicators work
  - [ ] Verify: error cases (missing CLI, bad credentials, no internet)

- [ ] Task: Conductor - User Manual Verification 'Integration & Cleanup' (Protocol in workflow.md)
