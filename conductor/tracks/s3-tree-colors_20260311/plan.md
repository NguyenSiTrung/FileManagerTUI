# Implementation Plan: S3 Tree Color Improvement

## Phase 1: Theme System — S3 Color Fields

- [x] Task 1: Add S3 color fields to ThemeColors struct
  - Add `s3_dir_fg`, `s3_file_fg`, `s3_border_fg` fields to `ThemeColors` in `theme.rs`
  - Set dark palette defaults: sky `#89dceb`, peach `#fab387`, sky `#89dceb`
  - Set light palette defaults: sky `#04a5e5`, orange `#fe640b`, sky `#04a5e5`

- [x] Task 2: Add S3 config fields to ThemeColorsConfig
  - Add `s3_dir_fg`, `s3_file_fg`, `s3_border_fg` optional fields to `ThemeColorsConfig` in `config.rs`
  - Add `apply_custom_colors()` entries for the 3 new fields in `theme.rs`

- [x] Task 3: Add theme tests for S3 colors
  - Test dark theme has correct S3 color defaults
  - Test light theme has correct S3 color defaults
  - Test custom theme overrides apply to S3 fields

- [x] Task: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

## Phase 2: Tree Widget — S3 Color Integration

- [x] Task 1: Replace hardcoded S3 color in tree.rs
  - Remove `let s3_color = ratatui::style::Color::Rgb(250, 179, 135);`
  - Use `self.theme.s3_dir_fg` for Directory nodes in S3 mode
  - Use `self.theme.s3_file_fg` for File nodes in S3 mode
  - Keep Loading state using `self.theme.info_fg` (unchanged)

- [x] Task 2: Add S3 border tint in ui.rs
  - When `app.is_s3_mode()`, override tree focused border to use `theme.s3_border_fg`
  - Unfocused border remains `theme.border_fg`

- [x] Task: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)

## Phase 3: Validation & Cleanup

- [x] Task 1: Run quality gates
  - `cargo test` — all 570 pass
  - `cargo clippy -- -D warnings` — no warnings
  - `cargo fmt --check` — formatted

- [x] Task 2: Manual verification
  - Build and run in S3 mode, verify directory vs file colors differ
  - Test with dark and light theme schemes
  - Verify border tint appears on focused tree panel in S3 mode

- [x] Task: Conductor - User Manual Verification 'Phase 3' (Protocol in workflow.md)
