# Track Learnings: s3-tree-colors_20260311

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- S3 runtime state (`s3_backend`, `s3_config`) stored on App struct, not in AppConfig — it's runtime state not user configuration (from: s3-browse_20260310)
- Widget builder pattern: `WidgetName::new(state, theme).block(block)` — theme is always the last constructor parameter (from: config-polish_20260228)
- Clone ThemeColors at render start to avoid borrow checker conflicts with `app` mutation during rendering (from: config-polish_20260228)
- All config fields use `Option<T>` so partial configs from different sources compose cleanly via `.or()` merge (from: config-polish_20260228)
- `#[serde(default)]` on both struct and fields ensures TOML parsing tolerates missing sections (from: config-polish_20260228)

---

<!-- Learnings from implementation will be appended below -->
