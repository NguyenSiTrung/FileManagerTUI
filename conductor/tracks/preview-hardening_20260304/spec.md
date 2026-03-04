# Preview Content Deep Analysis & Hardening

## Overview

A comprehensive code quality pass on the preview panel's file content handling
(`preview_content.rs`, `components/preview.rs`, and related `app.rs` methods).
This track addresses potential bugs, expands file type coverage, removes dead code,
and applies DRY refactoring — all without changing user-facing behavior significantly.

## Functional Requirements

### FR-1: Defense-in-Depth Size Guard in `load_highlighted_content`
- Add a file size check at the start of `load_highlighted_content`
- If file exceeds a reasonable hard cap (e.g., 5MB), return an error/info line
  instead of loading the entire file into memory
- Protects against future callers that skip the `update_preview()` size gate

### FR-2: Explicit UTF-8 Lossy Indication
- When `String::from_utf8` fails and `from_utf8_lossy` is used, prepend an
  info line to the preview: "⚠ File contains non-UTF-8 bytes (showing lossy)"
- Uses existing `colors.warning_fg` style

### FR-3: Fix `total_lines` Semantics in `load_head_tail_content`
- Currently returns `displayed.max(1)` (visual line count) as `total_lines`
- Change to return the actual file's total line count (from `fast_line_count`)
- Ensures `PreviewState.total_lines` always means "total lines in file"

### FR-4: Robust Error Handling in `read_head_lines`
- Replace `map_while(Result::ok)` with explicit error handling
- On I/O error mid-read, return the lines collected so far (partial success)

### FR-5: Expand `detect_syntax_name` Coverage
- Add: Dockerfile, Makefile, .env, .gitignore, Lua (.lua), PHP (.php),
  Swift (.swift), Kotlin (.kt/.kts), Scala (.scala), R (.r/.R),
  Terraform (.tf), Nix (.nix), Zig (.zig), GLSL (.glsl), XML (.xml)

### FR-6: Expand `BINARY_EXTENSIONS` Coverage
- Add: .wasm, .pyc, .pyo, .class, .o, .a, .lib, .dll, .deb, .rpm,
  .7z, .rar, .lz4, .zst, .parquet, .arrow, .avro, .safetensors,
  .png, .jpg, .jpeg, .gif, .bmp, .ico, .webp, .mp3, .mp4, .wav, .flac,
  .pdf, .doc, .docx, .xls, .xlsx, .ppt, .pptx

### FR-7: Remove Dead `Ctrl+W` Line-Wrap Handler
- Remove the `Ctrl+W` toggle from `handle_preview_keys` — it toggles
  `line_wrap` state but rendering never reads it, misleading users
- Keep the `line_wrap` field on `PreviewState` (reserved for future per convention)

### FR-8: Symlink-Loop Protection in `load_directory_summary`
- Add `VisitedDirs` tracking (same pattern as `spawn_async_dir_summary`)
  to the synchronous `load_directory_summary` deep scan function
- Prevents infinite loops on circular symlinks

### FR-9: DRY Refactor — Highlighting Code Deduplication
- Extract the shared line-number + syntax-highlighting spans pattern
  used in both `load_highlighted_content` and `highlight_single_line`
  into a single shared utility function
- Both functions currently duplicate the same spans-building logic

### FR-10: Safe Epoch Date Calculation
- Guard `epoch_days_to_date` against potential integer overflow when
  casting large `u64` values to `i64`
- Add bounds checking or use saturating arithmetic

## Non-Functional Requirements

### NFR-1: Zero User-Facing Behavior Changes
- All fixes must be backward-compatible
- No new keybindings, no changed keybindings (except removing dead Ctrl+W)
- No new dependencies

### NFR-2: Test Coverage
- Each FR must have corresponding unit tests
- Maintain existing test suite (all 60+ tests must continue to pass)
- Target: 80%+ coverage for modified functions

### NFR-3: Performance
- No regression in preview load time
- Size guard check adds negligible overhead (single `fs::metadata` call)

## Acceptance Criteria

1. `cargo test` passes with all existing + new tests
2. `cargo clippy -- -D warnings` clean
3. `cargo fmt --check` clean
4. `load_highlighted_content` returns gracefully for files > 5MB
5. UTF-8 lossy fallback shows warning line in preview
6. `total_lines` in head+tail mode reflects actual file line count
7. `detect_syntax_name` handles all listed extensions
8. `BINARY_EXTENSIONS` covers all listed types
9. `Ctrl+W` no longer toggles dead state in preview
10. Deep directory scan is protected against symlink loops
11. No duplicate highlighting code between functions

## Out of Scope

- Implementing actual line-wrap rendering (separate track)
- Adding new keybindings or UI changes
- Image preview support
- CSV/Parquet structured data preview
- Preview panel scrollbar
