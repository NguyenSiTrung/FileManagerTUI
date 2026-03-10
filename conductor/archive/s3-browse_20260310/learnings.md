# Track Learnings: s3-browse_20260310

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

Key patterns from `conductor/patterns.md` relevant to this track:

- TreeState owns root TreeNode + flat_items Vec; `flatten()` rebuilds flat list from tree recursively
- App delegates tree operations to TreeState methods; handler.rs maps keys to App methods
- Handler uses 3-level dispatch: global keys → panel-specific keys → dialog keys
- Store `event_tx: Option<mpsc::UnboundedSender<Event>>` on App to enable async ops from non-handler contexts
- Keep sync fallback when `event_tx` is None for test contexts
- Every `load_children()` call MUST be followed by `sort_children_of()`
- Adding new `NodeType` variants requires updating ALL exhaustive matches: tree widget, ui.rs status bar, handler.rs
- Graceful degradation for optional subsystems: wrap initialization in match, set state flag to false, show status message on error
- Async operations via `tokio::spawn` + `mpsc::unbounded_channel` events integrate with the existing event loop
- Use `Arc<AtomicBool>` for cancel tokens
- OSC 52 clipboard fallback for headless/web terminal environments

---

<!-- Learnings from implementation will be appended below -->

## [2026-03-10 19:55] - Phase 1: S3 Backend Module
- **Implemented:** S3 types (S3Path, S3Entry, S3Config), aws s3 ls parser, async S3Backend, CLI flags, event integration
- **Files changed:** src/s3/mod.rs, src/s3/types.rs, src/s3/parser.rs, src/s3/backend.rs, src/main.rs, src/app.rs, src/event.rs
- **Commit:** a6f7b54
- **Learnings:**
  - Patterns: S3 config stored on App (s3_backend, s3_config) rather than in AppConfig since it's runtime state not file config
  - Patterns: Virtual TreeNode construction without fs::metadata by directly setting struct fields (no TreeNode::new() call)
  - Gotchas: TreeState::find_node_mut is private; must use find_node_mut_pub for access from app.rs
  - Gotchas: `aws s3 ls` output uses leading whitespace before `PRE` — parser must handle both trimmed and untrimmed input
  - Context: S3 paths stored as PathBuf (PathBuf::from(s3_uri)) for compatibility with existing tree code
  - Context: S3ListingComplete event used for both initial and subdirectory listings; routing in main.rs checks is_root flag
---
