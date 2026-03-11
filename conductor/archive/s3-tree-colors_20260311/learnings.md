# Track Learnings: s3-tree-colors_20260311

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- S3 runtime state (`s3_backend`, `s3_config`) stored on App struct, not in AppConfig — it's runtime state not user configuration (from: s3-browse_20260310)
- Widget builder pattern: `WidgetName::new(state, theme).block(block)` — theme is always the last constructor parameter (from: config-polish_20260228)
- Clone ThemeColors at render start to avoid borrow checker conflicts with `app` mutation during rendering (from: config-polish_20260228)
- All config fields use `Option<T>` so partial configs from different sources compose cleanly via `.or()` merge (from: config-polish_20260228)
- `#[serde(default)]` on both struct and fields ensures TOML parsing tolerates missing sections (from: config-polish_20260228)

---

## [2026-03-11 17:54] - All Phases: S3 Tree Color Implementation
- **Implemented:** Added `s3_dir_fg`, `s3_file_fg`, `s3_border_fg` to ThemeColors struct with Catppuccin dark (sky/peach/sky) and light (sky/orange/sky) defaults. Added matching config fields to ThemeColorsConfig with apply_custom_colors entries. Replaced hardcoded `Color::Rgb(250,179,135)` in tree.rs with differentiated `s3_dir_fg`/`s3_file_fg`. Added S3 border tint in ui.rs when tree focused in S3 mode.
- **Files changed:** `src/theme.rs`, `src/config.rs`, `src/components/tree.rs`, `src/ui.rs`
- **Commit:** 143a5a8
- **Learnings:**
  - Patterns: Adding new theme color fields follows a 3-file pattern: struct field in theme.rs → defaults in dark/light_theme() → Option<String> in config.rs ThemeColorsConfig → apply_custom_colors() entry in theme.rs
  - Patterns: Border style override per-mode: compute a mode-specific style and substitute it in the border tuple destructuring in ui.rs
  - Context: S3 directories use ☁ icon and sky/cyan color, S3 files use 📦 icon and peach/orange color — creates a strong visual hierarchy
---
