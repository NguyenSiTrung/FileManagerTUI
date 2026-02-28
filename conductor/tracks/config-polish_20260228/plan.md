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

- [ ] Task 3: Integrate `AppConfig` into `App` and runtime
  <!-- files: src/app.rs, src/main.rs -->
  <!-- depends: task2 -->
  - [ ] Replace hardcoded values in `App` with `AppConfig` fields (preview limits, show_hidden, watcher settings)
  - [ ] Pass config to `App::new()`, propagate to subsystems (preview, watcher, tree)
  - [ ] Update `main.rs` to call `AppConfig::load()` before `App::new()`
  - [ ] Verify existing functionality unchanged with default config (regression test: `cargo test`)

- [ ] Task: Conductor - Phase 1 Verification (manual verification per workflow.md)

## Phase 2: Theme System
<!-- execution: sequential -->
<!-- depends: phase1 -->

- [ ] Task 1: Define theme data model and built-in palettes
  <!-- files: src/config.rs, src/theme.rs -->
  - [ ] Create `ThemeColors` struct with all color fields from PLAN.md Section 9.2
  - [ ] Implement `dark_theme()` — Catppuccin Mocha palette
  - [ ] Implement `light_theme()` — complementary light palette
  - [ ] Add `resolve_theme(config: &ThemeConfig) -> ThemeColors` — select dark/light/custom
  - [ ] Hex color string → `ratatui::style::Color` parsing with validation and fallback
  - [ ] Write tests for theme resolution, color parsing, invalid hex fallback

- [ ] Task 2: Apply theme colors throughout UI
  <!-- files: src/components/tree.rs, src/components/preview.rs, src/components/status_bar.rs, src/components/dialog.rs, src/components/search.rs, src/ui.rs -->
  <!-- depends: task1 -->
  - [ ] Pass `ThemeColors` to all widget renderers (TreeWidget, PreviewWidget, StatusBar, Dialog, Search)
  - [ ] Replace all hardcoded `Color::*` values with theme color references
  - [ ] Update `ui.rs` border/layout colors to use theme
  - [ ] Verify both dark and light themes render correctly (manual check)

- [ ] Task: Conductor - Phase 2 Verification (manual verification per workflow.md)

## Phase 3: UX Polish
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [ ] Task 1: Help overlay (`?` key)
  <!-- files: src/components/help.rs, src/components/mod.rs, src/handler.rs, src/app.rs -->
  - [ ] Create `components/help.rs` — `HelpOverlay` widget
  - [ ] Populate keybinding data grouped by category (Navigation, File Ops, Search, Preview)
  - [ ] Render as centered modal with scroll support (reuse Clear + Block pattern)
  - [ ] Add `AppMode::Help` state, wire `?` key toggle in handler
  - [ ] Scrollable with j/k or arrow keys, dismiss with `?` or `Esc`
  - [ ] Write test for help mode transitions

- [ ] Task 2: Mouse support
  <!-- files: src/tui.rs, src/event.rs, src/handler.rs, src/ui.rs -->
  - [ ] Enable crossterm mouse capture in `tui.rs` (init/restore)
  - [ ] Add `MouseEvent` handling in `event.rs` event loop
  - [ ] Tree panel: click-to-select (map y-coordinate to flat_items index)
  - [ ] Tree panel: click on directory to expand/collapse
  - [ ] Tree panel: scroll wheel up/down
  - [ ] Preview panel: click to switch focus
  - [ ] Preview panel: scroll wheel to scroll content
  - [ ] Drag-to-scroll in both panels
  - [ ] Config option `mouse = false` and CLI `--no-mouse` to disable
  - [ ] Write tests for mouse coordinate → item index mapping

- [ ] Task 3: Nerd Font icon toggle
  <!-- files: src/components/tree.rs, src/config.rs -->
  <!-- depends: task2 -->
  - [ ] Add `use_icons` field to `TreeConfig` (default: true)
  - [ ] Create icon lookup function with ASCII fallback mode
  - [ ] Update `TreeWidget` to use icon lookup instead of hardcoded icons
  - [ ] CLI flag `--no-icons` wired to config
  - [ ] Write tests for icon vs ASCII mode rendering

- [ ] Task 4: Sort options
  <!-- files: src/fs/tree.rs -->
  - [ ] Add `sort_by` and `dirs_first` to `TreeConfig`
  - [ ] Implement `sort_children()` with name/size/modified comparators
  - [ ] Apply sorting in `TreeNode::load_children()` and `reload_dir()`
  - [ ] Write tests for each sort mode with dirs_first on/off

- [ ] Task: Conductor - Phase 3 Verification (manual verification per workflow.md)

## Phase 4: Documentation & Release
<!-- execution: parallel -->
<!-- depends: -->

- [ ] Task 1: README.md
  <!-- files: README.md -->
  - [ ] Project description, feature highlights, screenshot placeholders
  - [ ] Installation: GitHub Releases, `cargo install`, container deployment
  - [ ] Configuration guide with example `config.toml`
  - [ ] Full keybinding reference table
  - [ ] License section

- [ ] Task 2: GitHub Actions CI workflow
  <!-- files: .github/workflows/ci.yml -->
  - [ ] Create `.github/workflows/ci.yml`
  - [ ] Jobs: cargo test, clippy, fmt check on push/PR
  - [ ] Rust toolchain setup with caching

- [ ] Task 3: GitHub Actions Release workflow
  <!-- files: .github/workflows/release.yml -->
  - [ ] Create `.github/workflows/release.yml`
  - [ ] Trigger on `v*` tag push
  - [ ] Build matrix: linux-musl (x86_64), macOS (x86_64 + aarch64), Windows (x86_64)
  - [ ] Create GitHub Release with all binaries attached
  - [ ] Binary naming convention: `fm-<target>`

- [ ] Task: Conductor - Phase 4 Verification (manual verification per workflow.md)
