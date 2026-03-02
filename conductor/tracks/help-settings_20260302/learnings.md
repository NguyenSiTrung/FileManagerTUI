# Track Learnings: help-settings_20260302

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Handler uses 3-level dispatch: global keys → panel-specific keys → dialog keys (from: preview-panel_20260227)
- Two-state overlay transition (Search → SearchAction) preserves search query when going back via Esc — reusable pattern for multi-step overlays (from: search-action_20260228)
- Clone SearchActionState before match in handler to avoid borrow conflicts — same pattern as DialogKind clone (from: search-action_20260228)
- All config fields use `Option<T>` so partial configs from different sources compose cleanly via `.or()` merge (from: config-polish_20260228)
- `#[serde(default)]` on both struct and fields ensures TOML parsing tolerates missing sections (from: config-polish_20260228)
- Widget builder pattern: `WidgetName::new(state, theme).block(block)` — theme is always the last constructor parameter (from: config-polish_20260228)
- Clone ThemeColors at render start to avoid borrow checker conflicts with `app` mutation during rendering (from: config-polish_20260228)
- `enter_edit_mode()` requires `update_preview()` first since it reads `preview_state.current_path` — order-dependent state setup (from: search-action_20260228)

---

<!-- Learnings from implementation will be appended below -->
