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
