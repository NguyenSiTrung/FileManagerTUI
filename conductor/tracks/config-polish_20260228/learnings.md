# Track Learnings: config-polish_20260228

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Use `#[allow(dead_code)]` on struct fields/variants reserved for future milestones
- Use `vec![...]` macro instead of `Vec::new()` + `.push()` chains — clippy enforces this
- TreeState owns root TreeNode + flat_items Vec + selected_index; `flatten()` rebuilds flat list from tree recursively
- App delegates tree operations to TreeState methods; handler.rs maps keys to App methods
- Handler uses 3-level dispatch: global keys → panel-specific keys → dialog keys
- Store SyntaxSet and Theme on App struct (expensive to load, reuse across previews)
- Use `last_previewed_index` to avoid re-loading preview on every render frame
- Use `Clear` widget + centered `Block` for modal overlays in ratatui
- Graceful degradation for optional subsystems: wrap initialization in match, set state flag to false, show status message on error

---

<!-- Learnings from implementation will be appended below -->
