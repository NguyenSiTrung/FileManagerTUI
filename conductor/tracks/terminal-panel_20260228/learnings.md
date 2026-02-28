# Track Learnings: terminal-panel_20260228

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Widget builder pattern: `WidgetName::new(state, theme).block(block)` — theme is always the last constructor parameter
- Clone ThemeColors at render start to avoid borrow checker conflicts with `app` mutation during rendering
- Handler uses 3-level dispatch: global keys → panel-specific keys (handle_tree_keys/handle_preview_keys) → dialog keys
- Store layout `Rect` on App from render → handler uses them for mouse coordinate mapping
- Mouse events only processed in Normal mode — prevents accidental clicks during dialogs
- crossterm event polling is blocking — must run in spawned tokio task with mpsc channel
- Use `Arc<AtomicBool>` for cancel tokens — no need for `tokio_util::CancellationToken`
- Graceful degradation for optional subsystems: wrap initialization in match, set state flag to false, show status message on error

---

<!-- Learnings from implementation will be appended below -->
