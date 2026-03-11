# Track Learnings: s3-head-preview_20260311

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- S3 backend shells out to `aws` CLI — no SDK dependency (from: patterns.md)
- S3 events follow async pattern: spawn tokio task → send Event via mpsc channel → handle in main loop (from: s3-browse_20260310)
- Binary detection: check known extensions first (fast), then null-byte scan in 8KB (from: patterns.md)
- Store SyntaxSet and Theme on App struct — reuse across previews (from: patterns.md)
- `last_previewed_index` prevents re-loading preview on every render frame (from: patterns.md)
- Handler uses 3-level dispatch: global → panel-specific → dialog keys (from: patterns.md)
- Config uses Option fields with merge chain: defaults → file → CLI overrides (from: patterns.md)

---

<!-- Learnings from implementation will be appended below -->

## [2026-03-11 12:00] - Phase 1-3: Full implementation
Thread: T-019cdb31-391c-7478-9f47-49c2cfcbfe62
- **Implemented:** Config (s3_head_lines), Backend (stream_head), App state, Event wiring, Key handler (H toggle), Preview integration, Help overlay
- **Files changed:** config.rs, main.rs, s3/backend.rs, event.rs, app.rs, preview_content.rs, handler.rs, components/help.rs
- **Commit:** de74ce7, 0422b62
- **Learnings:**
  - Patterns: Use `std::result::Result<T, E>` explicitly when the crate has a custom `Result<T>` type alias (app.rs uses `crate::error::Result`)
  - Patterns: `highlight_content_from_string()` reuses `detect_syntax_name` + `highlight_single_line` for in-memory content — same pipeline as file-based highlighting
  - Patterns: Shell pipe via `sh -c 'aws s3 cp ... - | head -n N'` is the cleanest way to stream first N lines without downloading full file
  - Gotchas: Binary detection for in-memory content needs `content.as_bytes()[..check_len].contains(&0)` — same pattern as file-based binary check
  - Context: S3 head state (active/loading/content/uri) lives on App struct alongside s3_backend/s3_config — reset on file navigation change in update_preview()
---

## [2026-03-11 12:10] - Phase 4: Config integration tests
Thread: T-019cdb31-391c-7478-9f47-49c2cfcbfe62
- **Implemented:** 4 config tests: default, TOML parsing, merge override, CLI override
- **Files changed:** config.rs
- **Commit:** 0422b62
- **Learnings:**
  - Patterns: Config test pattern: default → TOML parse → merge → CLI load with explicit path
---
