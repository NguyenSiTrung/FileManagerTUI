# S3 Tree Color Improvement

## Overview

Improve the visual presentation of AWS S3 entries in the tree panel by replacing
the single hardcoded peach color (`#fab387`) with a theme-integrated, differentiated
color scheme that distinguishes S3 directories from S3 files, respects dark/light
themes, supports user customization via `config.toml`, and adds a contextual border
tint to the tree panel when in S3 mode.

## Functional Requirements

### FR1: Differentiated S3 Directory vs File Colors
- S3 directories use a distinct color (e.g., sky/cyan `#89dceb` dark, `#04a5e5` light)
  that semantically matches the cloud icon ☁
- S3 files use a warm color (e.g., peach `#fab387` dark, `#fe640b` light) that
  semantically matches the package icon 📦
- S3 Loading entries continue using `theme.info_fg` with DIM modifier (unchanged)

### FR2: Theme System Integration
- Add `s3_dir_fg` and `s3_file_fg` fields to `ThemeColors` struct
- Set appropriate defaults in both `dark_theme()` and `light_theme()` palettes
- Remove hardcoded `Color::Rgb(250, 179, 135)` from `tree.rs`

### FR3: User Configurability
- Add `s3_dir_fg` and `s3_file_fg` optional fields to `ThemeColorsConfig`
- Support hex color override in `[theme.custom]` section of `config.toml`
- Apply overrides in `apply_custom_colors()` following existing pattern

### FR4: S3 Tree Panel Border Tint
- When in S3 mode, tint the tree panel focused border with S3 directory color
  (sky/cyan) for instant visual context reinforcement
- Add `s3_border_fg` to ThemeColors with appropriate dark/light defaults
- Unfocused border remains unchanged

## Non-Functional Requirements
- No new crate dependencies
- No performance impact (color fields are resolved once at theme init)
- Passes `cargo clippy -- -D warnings` and `cargo fmt --check`

## Acceptance Criteria
- [ ] S3 directories render in sky/cyan, S3 files render in peach/orange
- [ ] Colors change appropriately when switching dark → light theme
- [ ] `[theme.custom]` s3_dir_fg / s3_file_fg / s3_border_fg overrides work
- [ ] Tree border tints to S3 color when focused in S3 mode
- [ ] No hardcoded `Color::Rgb` values remain in S3 tree rendering code
- [ ] All existing tests pass, new theme tests added

## Out of Scope
- Changing S3 icons (☁/📦) — emoji icons are intentionally universal
- S3 preview panel color changes
- Status bar styling changes
