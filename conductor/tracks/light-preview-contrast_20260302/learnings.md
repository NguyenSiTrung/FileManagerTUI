# Track Learnings: light-preview-contrast_20260302

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Widget builder pattern: `WidgetName::new(state, theme).block(block)` — theme is always the last constructor parameter (from: config-polish_20260228)
- Clone ThemeColors at render start to avoid borrow checker conflicts with `app` mutation during rendering (from: config-polish_20260228)
- Store SyntaxSet and Theme on App struct (expensive to load, reuse across previews) (from: preview-panel_20260227)
- Use `last_previewed_index` to avoid re-loading preview on every render frame (from: preview-panel_20260227)
- Live config application: apply changes to running `AppConfig` immediately then serialize to disk (from: help-settings_20260302)

---

<!-- Learnings from implementation will be appended below -->
