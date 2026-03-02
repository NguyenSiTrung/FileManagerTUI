# Spec: Light Theme Preview Panel Contrast

## Overview

Fix poor contrast and readability of the Preview panel content when using
the light theme. All preview content currently uses hardcoded dark-theme
colors (`Color::White`, `Color::DarkGray`, `Color::Cyan`, `Color::Yellow`,
`Color::Green`) that are invisible or barely readable on light backgrounds.

## Functional Requirements

### FR-1: Theme-Aware Syntax Highlighting Default
- When the app theme scheme is `"light"`, the default syntect theme should
  be `"InspiredGitHub"` (a light-appropriate theme) instead of
  `"base16-ocean.dark"`.
- When the app theme scheme is `"dark"` (or unset), keep `"base16-ocean.dark"`.
- The existing `syntax_theme` config field continues to allow user override
  regardless of app theme.

### FR-2: Theme-Aware Preview Content Colors
- Pass `&ThemeColors` to all preview content functions that generate styled
  lines:
  - `load_highlighted_content()`
  - `load_head_tail_content()`
  - `highlight_single_line()`
  - `load_binary_metadata()`
  - `load_directory_summary()`
  - `load_directory_summary_shallow()`
  - `load_notebook_content()`
- Replace hardcoded colors with theme-aware equivalents:
  - `Color::White` (values) → `theme.preview_fg`
  - `Color::DarkGray` (line numbers) → `theme.preview_line_nr_fg`
  - `Color::DarkGray` (dim text) → `theme.dim_fg`
  - `Color::Cyan` (labels) → `theme.info_fg`
  - `Color::Yellow` (hints/separators) → `theme.warning_fg`
  - `Color::Green` (output prefix) → `theme.success_fg`
  - `Color::Red` (errors) → `theme.error_fg`

### FR-3: Theme Reload on Scheme Change
- When the user changes the theme scheme via the Settings panel, the
  syntect theme must also be reloaded to match the new scheme (unless
  overridden by `syntax_theme` config).

## Non-Functional Requirements
- No new `ThemeColors` fields required — use existing semantic colors.
- All existing tests must continue to pass (update signatures as needed).
- No visual regression in dark theme.

## Acceptance Criteria
1. Light theme: syntax-highlighted code in preview is readable with
   proper contrast (light syntect theme).
2. Light theme: binary metadata labels, values, and hints are clearly
   readable.
3. Light theme: directory summaries have proper contrast for all elements.
4. Light theme: notebook cell headers, code, outputs, and errors are
   legible.
5. Dark theme: no visual changes (existing colors preserved).
6. Changing theme scheme in Settings reloads the appropriate syntect theme.
7. User-configured `syntax_theme` still overrides the auto-selected default.
8. All tests pass, clippy clean, formatted.

## Out of Scope
- Adding new configurable color fields to ThemeColors.
- Editor panel contrast (already has its own theme-aware colors).
- Custom theme (`"custom"` scheme) changes beyond what already exists.
