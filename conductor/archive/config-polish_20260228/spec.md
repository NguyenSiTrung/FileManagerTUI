# Spec: Configuration + Polish (Milestone 7)

## Overview

Make FileManagerTUI production-ready with full configuration support, theme system,
UX polish features (help overlay, mouse support, sort options, icon toggle), comprehensive
documentation, and cross-platform release automation.

This is the final milestone — after completion, the app should be deployable and
configurable for daily use in KubeFlow, Jupyter, and standard terminal environments.

## Functional Requirements

### FR-1: TOML Configuration Loading (`config.rs`)
- Create a dedicated `config.rs` module with `AppConfig` struct (serde-deserializable)
- Config resolution order (first found wins, values merge/override):
  1. CLI flags (`--config`, `--no-preview`, `--theme`, etc.)
  2. `$FM_TUI_CONFIG` environment variable (path to config file)
  3. Project-local `.fm-tui.toml` in the current working directory
  4. Global `~/.config/fm-tui/config.toml`
  5. Built-in defaults
- Full schema as defined in PLAN.md Section 9.2:
  - `[general]`: default_path, show_hidden, confirm_delete
  - `[preview]`: max_full_preview_bytes, head_lines, tail_lines, default_view_mode, tab_width, line_wrap, syntax_theme
  - `[tree]`: sort_by, dirs_first, use_icons
  - `[watcher]`: enabled, debounce_ms
  - `[theme]`: scheme (dark/light/custom) + color sections
- All fields optional with sensible defaults (app works with zero config)

### FR-2: Full CLI Argument Parsing
- Extend existing `clap` setup to support all options from PLAN.md Section 9.3:
  - `-c, --config <FILE>`: Path to config file
  - `--no-preview`: Disable preview panel
  - `--no-watcher`: Disable filesystem watcher
  - `--no-icons`: Use ASCII instead of Nerd Font icons
  - `--head-lines <N>`: Lines from top for large file preview
  - `--tail-lines <N>`: Lines from bottom for large file preview
  - `--max-preview <BYTES>`: Max file size for full preview
  - `--theme <THEME>`: Color theme selection
- CLI args override config file values

### FR-3: Theme Support
- Built-in dark theme (Catppuccin Mocha palette as in PLAN.md)
- Built-in light theme (complementary light palette)
- Custom theme via `[theme.custom]` section in config
- Theme colors applied to: tree panel, preview panel, status bar, borders, dialogs, search overlay
- Runtime theme is resolved at startup based on `scheme` field

### FR-4: Help Overlay (`?` key)
- Centered modal overlay showing all keybindings grouped by category:
  - Navigation, File Operations, Search & Filter, Preview Mode
- Scrollable if content exceeds screen height
- Dismiss with `?` or `Esc`
- Uses same modal pattern as existing dialogs (Clear widget + Block)

### FR-5: Mouse Support
- Click to select item in tree panel
- Scroll wheel to navigate tree (up/down)
- Click to expand/collapse directories (on the directory name)
- Click on preview panel to switch focus
- Scroll wheel in preview panel to scroll content
- Drag-to-scroll in both panels
- Mouse can be disabled via config (`[general] mouse = false`) or CLI (`--no-mouse`)
- Enable crossterm mouse capture on startup, disable on exit

### FR-6: Nerd Font Icon Toggle
- Config option `[tree] use_icons = true/false`
- CLI flag `--no-icons`
- When disabled: use ASCII fallback characters (📁→[D], 📄→[F], etc. or simple text indicators)
- Icon mapping already defined in PLAN.md Section 6.1

### FR-7: Sort Options
- Config option `[tree] sort_by = "name" | "size" | "modified"`
- `dirs_first = true/false` (directories always listed before files)
- Sorting applied during `flatten()` — affects display order
- Default: name ascending, dirs first

### FR-8: README.md
- Project description and feature highlights
- Screenshots/GIFs of key features (tree navigation, preview, search, dialogs)
- Installation instructions:
  - From GitHub Releases (pre-built binaries)
  - From source (`cargo install`)
  - Container deployment (`kubectl cp`)
- Configuration guide with example `config.toml`
- Keybinding reference table
- License and contribution section

### FR-9: GitHub Actions Release
- CI workflow (`.github/workflows/ci.yml`): test + clippy + fmt on every push/PR
- Release workflow (`.github/workflows/release.yml`): triggered on `v*` tag push
- Build targets:
  - `x86_64-unknown-linux-musl` (static Linux binary)
  - `x86_64-apple-darwin` (macOS Intel)
  - `aarch64-apple-darwin` (macOS Apple Silicon)
  - `x86_64-pc-windows-msvc` (Windows)
- Create GitHub Release with all binaries attached
- Binary naming: `fm-<target>` (e.g., `fm-x86_64-unknown-linux-musl`)

## Non-Functional Requirements

- Config loading must not add noticeable startup latency (< 10ms)
- Theme colors validated at load time (invalid hex → fallback to default)
- Mouse events must not interfere with keyboard-only workflows
- README must be accurate and up-to-date with actual keybindings
- Release binaries must be under 10MB (static musl build)

## Acceptance Criteria

1. ✅ App reads config from TOML file at all 4 resolution levels (CLI, env, local, global)
2. ✅ All CLI flags from PLAN.md Section 9.3 are functional
3. ✅ `--theme dark` and `--theme light` produce visually distinct, usable UIs
4. ✅ Custom theme colors in config file are applied correctly
5. ✅ `?` key shows a comprehensive help overlay with all keybindings
6. ✅ Mouse click selects items, scroll wheel navigates, click expands/collapses dirs
7. ✅ `--no-icons` produces usable output without Nerd Fonts
8. ✅ `sort_by = "size"` and `sort_by = "modified"` produce correct ordering
9. ✅ README.md has install instructions, screenshots, keybinding table
10. ✅ Pushing a `v*` tag builds and releases binaries for Linux, macOS (x2), Windows

## Out of Scope

- Plugin system or extensible keybindings
- Custom file type associations
- Bookmarks or favorites
- Multi-tab / split-pane navigation
- Remote filesystem support (SSH, S3)
- Localization / i18n
