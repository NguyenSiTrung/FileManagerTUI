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

## [2026-02-28 09:45] - Phase 1 Task 1: Create config.rs module with AppConfig struct
- **Implemented:** Full config module with TOML loading, multi-source merge, 14 getter methods
- **Files changed:** src/config.rs (new), src/main.rs (mod declaration), Cargo.toml (serde, toml, dirs deps)
- **Commit:** dad644b
- **Learnings:**
  - Patterns: All config fields use `Option<T>` so partial configs from different sources compose cleanly via `.or()` merge
  - Patterns: `#[serde(default)]` on both struct and fields ensures TOML parsing tolerates missing sections
  - Gotchas: Raw strings containing `"#` sequences (hex colors) need `r##"..."##` double-hash delimiters — single `r#"..."#` breaks
  - Gotchas: Adding `#[allow(dead_code)]` on the impl block (not individual methods) when all methods will be used in a later task
  - Context: Config candidate path resolution: env var → CWD `.fm-tui.toml` → `dirs::config_dir()/fm-tui/config.toml`
---

## [2026-02-28 10:05] - Phase 2 Task 1: Define theme data model and built-in palettes
- **Implemented:** ThemeColors struct, dark_theme() (Catppuccin Mocha), light_theme() (Catppuccin Latte), resolve_theme(), parse_hex_color(), apply_custom_colors()
- **Files changed:** src/theme.rs (new), src/app.rs (ThemeColors field + import), src/main.rs (mod declaration)
- **Commit:** 417c1a5
- **Learnings:**
  - Patterns: All customizable colors use `Option<String>` in the config struct (ThemeColorsConfig), resolved to concrete `Color::Rgb` values at startup
  - Patterns: Base palette provides all defaults, custom overrides only apply if the hex string parses successfully — invalid hex silently falls back
  - Patterns: Semantic colors (error, warning, success, info, accent, dim) are NOT user-configurable — they are fixed per palette for consistency
  - Context: ThemeColors uses `Color::Reset` for bg fields to let the terminal's native background show through

## [2026-02-28 10:05] - Phase 2 Task 2: Apply theme colors throughout UI
- **Implemented:** Passed ThemeColors through all 5 widget constructors, replaced every hardcoded Color::* value
- **Files changed:** ui.rs, tree.rs, preview.rs, status_bar.rs, dialog.rs, search.rs
- **Commit:** 65a9969
- **Learnings:**
  - Gotchas: Borrow checker conflict when `&app.theme_colors` is held while `app.clear_expired_status()` is called — solved by cloning ThemeColors at render start (it's just Copy-like Color enums, cheap)
  - Gotchas: Moving `Color` import to `#[cfg(test)]` block when production code no longer uses it but test assertions still check exact Color values
  - Patterns: Widget builder pattern: `WidgetName::new(state, theme).block(block)` — theme is always the last constructor parameter
  - Patterns: Tests use `test_theme() -> ThemeColors { theme::dark_theme() }` helper to avoid repeating theme construction
---
