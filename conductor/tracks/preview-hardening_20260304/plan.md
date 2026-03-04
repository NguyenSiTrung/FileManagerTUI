# Implementation Plan: Preview Content Deep Analysis & Hardening

## Phase 1: Bug Fixes

- [ ] Task 1: Add defense-in-depth size guard to `load_highlighted_content` (FR-1)
  - [ ] Add `fs::metadata` size check at function start (before `fs::read`)
  - [ ] If file > 5MB, return info line: "File too large for full preview (X MB)"
  - [ ] Add test: file > 5MB returns info line gracefully
  - [ ] Add test: file < 5MB loads normally (existing behavior preserved)

- [ ] Task 2: Add UTF-8 lossy fallback warning (FR-2)
  - [ ] When `String::from_utf8` fails, prepend warning line with `colors.warning_fg`
  - [ ] Warning text: "⚠ File contains non-UTF-8 bytes (showing lossy conversion)"
  - [ ] Add test: file with null bytes + text shows warning + content
  - [ ] Add test: valid UTF-8 file shows no warning

- [ ] Task 3: Fix `total_lines` semantics in `load_head_tail_content` (FR-3)
  - [ ] Change return value from `displayed.max(1)` to actual `total_lines` from `fast_line_count`
  - [ ] Verify `total_lines` is already computed at line 291 — use it in the return
  - [ ] Update existing tests to assert on actual file line count
  - [ ] Add test: 100-line file with head=10/tail=5 returns total_lines=100

- [ ] Task 4: Fix silent error dropping in `read_head_lines` (FR-4)
  - [ ] Replace `map_while(Result::ok)` with explicit error handling
  - [ ] On I/O error mid-read, return the lines collected so far (partial success)
  - [ ] Add test: simulate I/O scenarios with valid files
  - [ ] Ensure existing `read_head_lines` tests still pass

- [ ] Task 5: Safe epoch date calculation (FR-10)
  - [ ] Add bounds check on `days` parameter before `i64` cast in `epoch_days_to_date`
  - [ ] Use `i64::try_from(days)` with fallback to a default date on overflow
  - [ ] Add test: very large day value doesn't panic
  - [ ] Add test: normal dates still compute correctly

## Phase 2: Symlink Protection & Dead Code Cleanup

- [ ] Task 1: Add symlink-loop protection to `load_directory_summary` (FR-8)
  - [ ] Import and use `VisitedDirs` from `crate::fs::tree`
  - [ ] Initialize `VisitedDirs` before the walk loop
  - [ ] Skip directories already visited (same pattern as `spawn_async_dir_summary`)
  - [ ] Add test: directory with symlink loop doesn't hang

- [ ] Task 2: Remove dead `Ctrl+W` line-wrap handler (FR-7)
  - [ ] Remove the `KeyCode::Char('w')` + `CONTROL` match arm from `handle_preview_keys`
  - [ ] Keep `line_wrap` field on `PreviewState` (convention: reserved for future)
  - [ ] Verify no references to the removed handler remain
  - [ ] Run full test suite to confirm no regressions

## Phase 3: File Type Coverage Expansion

- [ ] Task 1: Expand `detect_syntax_name` coverage (FR-5)
  - [ ] Add extensions: lua, php, swift, kt, kts, scala, r, tf, nix, zig, glsl, xml
  - [ ] Add filename-based detection: Dockerfile → "Dockerfile", Makefile → "Makefile"
  - [ ] Add: .env → "Bash", .gitignore → "Plain Text"
  - [ ] Add tests for each new extension mapping
  - [ ] Add tests for filename-based detection (Dockerfile, Makefile)

- [ ] Task 2: Expand `BINARY_EXTENSIONS` list (FR-6)
  - [ ] Add ML formats: .safetensors, .parquet, .arrow, .avro
  - [ ] Add compiled: .wasm, .pyc, .pyo, .class, .o, .a, .lib, .dll
  - [ ] Add packages: .deb, .rpm, .7z, .rar, .lz4, .zst
  - [ ] Add media: .png, .jpg, .jpeg, .gif, .bmp, .ico, .webp, .mp3, .mp4, .wav, .flac
  - [ ] Add documents: .pdf, .doc, .docx, .xls, .xlsx, .ppt, .pptx
  - [ ] Add tests for representative new extensions

## Phase 4: Code Quality Refactoring

- [ ] Task 1: DRY refactor — unify highlighting span building (FR-9)
  - [ ] Extract shared pattern from `load_highlighted_content` loop body and `highlight_single_line`
  - [ ] Create single helper: `build_highlighted_spans(line_str, line_num, width, highlighter, ss, colors) -> Vec<Span>`
  - [ ] Update `load_highlighted_content` to use the new helper
  - [ ] Update `highlight_single_line` to use the new helper (or remove and inline)
  - [ ] Run full test suite — all 60+ existing tests must pass
  - [ ] Run `cargo clippy -- -D warnings` — clean
  - [ ] Run `cargo fmt --check` — clean
