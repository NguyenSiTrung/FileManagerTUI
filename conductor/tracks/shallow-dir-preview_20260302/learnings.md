# Track Learnings: shallow-dir-preview_20260302

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Async paste via `tokio::spawn` + `mpsc::unbounded_channel` events (Progress, OperationComplete) integrates with the existing event loop
- Use `Arc<AtomicBool>` for cancel tokens — no need for `tokio_util::CancellationToken`
- Store `event_tx: Option<mpsc::UnboundedSender<Event>>` on App to enable async ops from non-handler contexts
- Keep sync fallback when `event_tx` is None for test contexts that don't have a runtime event loop
- Use iterative stack-based directory walk with entry cap (10K) to prevent hanging on huge trees
- All config fields use `Option<T>` so partial configs from different sources compose cleanly via `.or()` merge
- `#[serde(default)]` on both struct and fields ensures TOML parsing tolerates missing sections

---

<!-- Learnings from implementation will be appended below -->
