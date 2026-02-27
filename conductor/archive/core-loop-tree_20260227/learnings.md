# Track Learnings: core-loop-tree_20260227

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

*No prior patterns — this is the first track.*

---

## [2026-02-27 08:47] - Phase 1-5: Full Track Implementation
Thread: https://ampcode.com/threads/T-019c9e3c-89b4-7559-bed0-0eec7c415ccd
- **Implemented:** Complete core loop + tree rendering MVP — project scaffolding, error types, TreeNode with lazy loading, terminal init/restore, async event loop, App state, tree widget with box-drawing, keyboard navigation
- **Files changed:** Cargo.toml, src/main.rs, src/app.rs, src/error.rs, src/event.rs, src/handler.rs, src/tui.rs, src/ui.rs, src/fs/mod.rs, src/fs/tree.rs, src/components/mod.rs, src/components/tree.rs
- **Commits:** 3181fb4, 7aeee4d, 753a461, 16922a6, 3a9acef, cbbeb14, 125a8a6
- **Learnings:**
  - Patterns: TreeState owns root + flat_items + selected_index; flatten() rebuilds flat list from tree. App delegates tree ops to TreeState. Widget renders from flat_items with scroll offset.
  - Patterns: Root node always shown regardless of hidden status (is_root flag in flatten_node) — tempfile creates hidden-prefixed dirs.
  - Patterns: Box-drawing prefix built by walking ancestor chain backwards to determine continuation lines (│ vs space).
  - Gotchas: tempfile TempDir names can start with `.tmp` prefix, making them "hidden" — root must bypass hidden filter.
  - Gotchas: crossterm event polling is blocking, must be in a spawned tokio task with mpsc channel forwarding.
  - Context: Module structure: fs/tree.rs (data), components/tree.rs (widget), app.rs (state), handler.rs (input), event.rs (async events), tui.rs (terminal), ui.rs (layout).
---
