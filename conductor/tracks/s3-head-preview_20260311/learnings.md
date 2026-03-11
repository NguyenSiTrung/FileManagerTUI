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
