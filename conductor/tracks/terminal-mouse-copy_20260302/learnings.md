# Track Learnings: terminal-mouse-copy_20260302

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Mouse handling should remain mode-gated: normal mode for panels, dedicated path for edit mode (`handler.rs`).
- Mouse coordinate mapping must account for bordered panel offsets (`y + 1`, `x + 1`) before converting to local row/col.
- Terminal-focused key handling intercepts reserved shortcuts before PTY forwarding; keep this ordering for new shortcuts.
- Modifier matching should use bitflag `contains()` checks with explicit exclusions when needed.
- System clipboard integration already exists (`copy_to_system_clipboard`) and must degrade gracefully in headless sessions.

---

<!-- Learnings from implementation will be appended below -->
