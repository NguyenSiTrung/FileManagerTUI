# Plan: Configuration + Polish (Milestone 7)

## Phase 1: Configuration Foundation
<!-- execution: sequential -->

- [x] Task 1: Create `config.rs` module with `AppConfig` struct *(dad644b)*
  <!-- files: src/config.rs -->
  - [x] Define `AppConfig`, `GeneralConfig`, `PreviewConfig`, `TreeConfig`, `WatcherConfig`, `ThemeConfig` structs with serde Deserialize
  - [x] All fields `Option<T>` with `Default` impl providing sensible defaults
  - [x] Add `AppConfig::load()` — resolution chain: CLI → env → local `.fm-tui.toml` → global `~/.config/fm-tui/config.toml` → defaults
  - [x] Merge logic: later sources fill in `None` fields from earlier sources
  - [x] Write unit tests for default values, TOML parsing, merge behavior, missing file handling

- [x] Task 2: Extend CLI argument parsing with full options *(89f0f1e)*
  <!-- files: src/main.rs -->
  <!-- depends: task1 -->
  - [x] Add all clap args: `--config`, `--no-preview`, `--no-watcher`, `--no-icons`, `--head-lines`, `--tail-lines`, `--max-preview`, `--theme`, `--no-mouse`
  - [x] Create `CliArgs::as_config_overrides()` to produce partial `AppConfig` from CLI flags
  - [x] Wire CLI overrides as highest-priority source in `AppConfig::load()`
  - [x] Write tests for CLI flag → config override mapping

- [x] Task 3: Integrate `AppConfig` into `App` and runtime *(fabcecb)*
  <!-- files: src/app.rs, src/main.rs -->
  <!-- depends: task2 -->
  - [x] Replace hardcoded values in `App` with `AppConfig` fields (preview limits, show_hidden, watcher settings)
  - [x] Pass config to `App::new()`, propagate to subsystems (preview, watcher, tree)
  - [x] Update `main.rs` to call `AppConfig::load()` before `App::new()`
  - [x] Verify existing functionality unchanged with default config (regression test: `cargo test`)

- [x] Task: Conductor - Phase 1 Verification (279 tests passing, clippy clean, fmt clean)

## Phase 2: Theme System
<!-- execution: sequential -->
<!-- depends: phase1 -->

- [x] Task 1: Define theme data model and built-in palettes *(417c1a5)*
  <!-- files: src/config.rs, src/theme.rs -->
  - [x] Create `ThemeColors` struct with all color fields from PLAN.md Section 9.2
  - [x] Implement `dark_theme()` — Catppuccin Mocha palette
  - [x] Implement `light_theme()` — complementary light palette
  - [x] Add `resolve_theme(config: &ThemeConfig) -> ThemeColors` — select dark/light/custom
  - [x] Hex color string → `ratatui::style::Color` parsing with validation and fallback
  - [x] Write tests for theme resolution, color parsing, invalid hex fallback

- [x] Task 2: Apply theme colors throughout UI *(65a9969)*
  <!-- files: src/components/tree.rs, src/components/preview.rs, src/components/status_bar.rs, src/components/dialog.rs, src/components/search.rs, src/ui.rs -->
  <!-- depends: task1 -->
  - [x] Pass `ThemeColors` to all widget renderers (TreeWidget, PreviewWidget, StatusBar, Dialog, Search)
  - [x] Replace all hardcoded `Color::*` values with theme color references
  - [x] Update `ui.rs` border/layout colors to use theme
  - [x] Verify both dark and light themes render correctly (manual check)

- [x] Task: Conductor - Phase 2 Verification (289 tests passing, clippy clean, fmt clean)

## Phase 3: UX Polish
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [x] Task 1: Help overlay (`?` key) <!-- commit: a764e89 -->
  <!-- files: src/components/help.rs, src/components/mod.rs, src/handler.rs, src/app.rs -->
  - [x] Create `components/help.rs` — `HelpOverlay` widget
  - [x] Populate keybinding data grouped by category (Navigation, File Ops, Search, Preview)
  - [x] Render as centered modal with scroll support (reuse Clear + Block pattern)
  - [x] Add `AppMode::Help` state, wire `?` key toggle in handler
  - [x] Scrollable with j/k or arrow keys, dismiss with `?` or `Esc`
  - [x] Write test for help mode transitions

- [x] Task 2: Mouse support <!-- commit: 8925743 -->
  <!-- files: src/tui.rs, src/event.rs, src/handler.rs, src/ui.rs -->
  - [x] Enable crossterm mouse capture in `tui.rs` (init/restore)
  - [x] Add `MouseEvent` handling in `event.rs` event loop
  - [x] Tree panel: click-to-select (map y-coordinate to flat_items index)
  - [x] Tree panel: click on directory to expand/collapse
  - [x] Tree panel: scroll wheel up/down
  - [x] Preview panel: click to switch focus
  - [x] Preview panel: scroll wheel to scroll content
  - [x] Config option `mouse = false` and CLI `--no-mouse` to disable
  - [x] Write tests for mouse coordinate → item index mapping

- [x] Task 3: Nerd Font icon toggle <!-- commit: 8ae38f0 -->
  <!-- files: src/components/tree.rs, src/config.rs -->
  <!-- depends: task2 -->
  - [x] Add `use_icons` field to `TreeConfig` (default: true)
  - [x] Create icon lookup function with ASCII fallback mode
  - [x] Update `TreeWidget` to use icon lookup instead of hardcoded icons
  - [x] CLI flag `--no-icons` wired to config

- [x] Task 4: Sort options <!-- commit: 10b6c37 -->
  <!-- files: src/fs/tree.rs -->
  - [x] Add `sort_by` and `dirs_first` to `TreeConfig`
  - [x] Implement `sort_children()` with name/size/modified comparators
  - [x] Apply sorting in `TreeState::sort_children_of()` and `reload_dir()`
  - [x] Write tests for each sort mode with dirs_first on/off
  - [x] Added `s` to cycle sort, `S` to toggle dirs_first keybindings

- [x] Task: Conductor - Phase 3 Verification (305 tests passing, clippy clean, fmt clean)

## Phase 4: Documentation & Release
<!-- execution: parallel -->
<!-- depends: -->

- [x] Task 1: README.md <!-- commit: caa280e -->
  <!-- files: README.md -->
  - [x] Project description, feature highlights
  - [x] Installation: GitHub Releases, `cargo install`, build from source
  - [x] Configuration guide with example `config.toml`
  - [x] Full keybinding reference table (Navigation, File Ops, Search, Preview, General, Mouse)
  - [x] License section + MIT LICENSE file

- [x] Task 2: GitHub Actions CI workflow <!-- commit: 7c789af -->
  <!-- files: .github/workflows/ci.yml -->
  - [x] Create `.github/workflows/ci.yml`
  - [x] Jobs: cargo test, clippy, fmt check on push/PR
  - [x] Rust toolchain setup with caching

- [x] Task 3: GitHub Actions Release workflow <!-- commit: 3504c41 -->
  <!-- files: .github/workflows/release.yml -->
  - [x] Create `.github/workflows/release.yml`
  - [x] Trigger on `v*` tag push
  - [x] Build matrix: linux-musl (x86_64), macOS (x86_64 + aarch64), Windows (x86_64)
  - [x] Create GitHub Release with all binaries attached
  - [x] Binary naming convention: `fm-<target>`

- [x] Task: Conductor - Phase 4 Verification (305 tests passing, clippy clean, fmt clean)
