# Track Learnings: tree-scroll_20260303

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- TreeState owns root TreeNode + flat_items Vec + selected_index; `flatten()` rebuilds flat list from tree recursively
- Store layout `Rect` on App from render → handler uses them for mouse coordinate mapping
- Mouse events only processed in Normal mode — prevents accidental clicks during dialogs
- Must account for border offset (y+1) when mapping mouse click row to flat_items index in bordered widgets
- Widget builder pattern: `WidgetName::new(state, theme).block(block)` — theme is always the last constructor parameter
- All config fields use `Option<T>` so partial configs from different sources compose cleanly via `.or()` merge
- `#[serde(default)]` on both struct and fields ensures TOML parsing tolerates missing sections
- Type bridge pattern (`SettingValueKind`): intermediate enum bridging heterogeneous config field types to a uniform UI/editing API

---

<!-- Learnings from implementation will be appended below -->
