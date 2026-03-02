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

## [2026-03-02 21:59] - Phase 1: Theme-aware syntax theme defaults + live reload
- **Implemented:** `AppConfig::syntax_theme_name()` now chooses `InspiredGitHub` for light scheme when no preview override exists; settings live-apply now reloads `app.syntax_theme` when either `preview.syntax_theme` or `theme.scheme` changes.
- **Files changed:** `src/config.rs`, `src/app.rs`, `src/handler.rs`, `src/components/settings.rs`
- **Commit:** pending
- **Learnings:**
  - Patterns: Use `config.syntax_theme_name(config.theme_scheme())` as the single source of truth for preview syntax theme resolution.
  - Gotchas: Theme color palette and syntect theme are separate state; changing `theme.scheme` must update both `app.theme_colors` and `app.syntax_theme`.
  - Context: Invalidating `app.last_previewed_index` is enough to force preview repaint on the next render cycle after theme changes.
---

## [2026-03-02 21:59] - Phase 2/3: Theme-aware preview content colors + verification
- **Implemented:** Threaded `&ThemeColors` through preview content renderers and replaced hardcoded preview colors with semantic theme fields (`preview_fg`, `preview_line_nr_fg`, `dim_fg`, `info_fg`, `warning_fg`, `success_fg`, `error_fg`); updated all call sites and tests.
- **Files changed:** `src/preview_content.rs`, `src/app.rs`
- **Commit:** pending
- **Learnings:**
  - Patterns: Keep preview rendering APIs explicit by passing both syntect `Theme` (syntax colors) and UI `ThemeColors` (semantic panel colors).
  - Gotchas: Bulk regex refactors across function calls can accidentally touch function declarations; re-run targeted signature validation with `rg` immediately after.
  - Context: `spawn_async_dir_summary_shallow` needs a cloned `ThemeColors` moved into `spawn_blocking` for directory preview styling consistency.
---
