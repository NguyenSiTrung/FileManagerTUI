# Plan: Core Loop + Tree Rendering

## Phase 1: Project Setup & Error Foundation
<!-- execution: sequential -->

- [ ] Task 1: Initialize Cargo project with dependencies
  - [ ] Create `Cargo.toml` with: ratatui (0.29, crossterm), crossterm (0.28), tokio (1, full), thiserror, clap (4, derive)
  - [ ] Create directory structure: `src/`, `src/fs/`, `src/components/`
  - [ ] Create module files with `mod` declarations: `src/fs/mod.rs`, `src/components/mod.rs`
  - [ ] Verify `cargo check` passes

- [ ] Task 2: Define custom error types
  - [ ] Create `src/error.rs` with `AppError` enum using `thiserror` (Io, Terminal, InvalidPath variants)
  - [ ] Define `pub type Result<T> = std::result::Result<T, AppError>` alias
  - [ ] Write unit tests for error conversions
  - [ ] Verify `cargo test` passes

## Phase 2: TreeNode Data Structure & Lazy Loading
<!-- execution: sequential -->
<!-- depends: phase1 -->

- [ ] Task 1: Implement TreeNode struct and types
  <!-- files: src/fs/tree.rs -->
  - [ ] Create `src/fs/tree.rs` with `TreeNode`, `NodeType`, `FileMeta` structs
  - [ ] Implement `TreeNode::new()` constructor from a path
  - [ ] Write unit tests for TreeNode creation (file, directory, symlink)

- [ ] Task 2: Implement lazy directory loading
  <!-- files: src/fs/tree.rs -->
  <!-- depends: task1 -->
  - [ ] Implement `TreeNode::load_children()` — reads directory, creates child nodes
  - [ ] Sort children: directories first, then alphabetical case-insensitive
  - [ ] Handle permission-denied and broken symlinks gracefully (skip with warning)
  - [ ] Write tests: load directory, sort order, empty directory, hidden files

- [ ] Task 3: Implement tree flattening and TreeState
  <!-- files: src/fs/tree.rs -->
  <!-- depends: task2 -->
  - [ ] Create `FlatItem` struct with depth, name, path, node_type, is_expanded, is_last_sibling
  - [ ] Implement `TreeState` with root, flat_items, selected_index, scroll_offset, show_hidden
  - [ ] Implement `TreeState::flatten()` — recursive walk producing `Vec<FlatItem>`, respecting show_hidden
  - [ ] Implement `TreeState::toggle_hidden()` — re-flattens with updated visibility
  - [ ] Write tests: flatten expanded tree, hidden file filtering, is_last_sibling correctness

## Phase 3: Terminal Init & Async Event Loop
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [ ] Task 1: Implement terminal setup and teardown
  <!-- files: src/tui.rs -->
  - [ ] Create `src/tui.rs` with `Tui` struct wrapping `Terminal<CrosstermBackend<Stdout>>`
  - [ ] Implement `Tui::new()` — enter alternate screen, enable raw mode
  - [ ] Implement `Tui::restore()` — leave alternate screen, disable raw mode
  - [ ] Install panic hook that restores terminal before printing panic info

- [ ] Task 2: Implement async event system
  <!-- files: src/event.rs -->
  - [ ] Create `src/event.rs` with `Event` enum (Key, Tick, Resize)
  - [ ] Implement `EventHandler` struct — spawns tokio task polling crossterm events
  - [ ] Use `tokio::sync::mpsc` channel to forward events
  - [ ] Configurable tick rate (default 16ms)

- [ ] Task 3: Wire up main.rs entry point
  <!-- files: src/main.rs -->
  <!-- depends: task1, task2 -->
  - [ ] Create `src/main.rs` with `#[tokio::main]`, clap CLI arg for path
  - [ ] Initialize Tui, EventHandler, App
  - [ ] Implement main loop: receive event → handle → draw → check should_quit
  - [ ] Verify: `cargo run -- .` launches and `q` exits cleanly

## Phase 4: App State & Tree Widget Rendering
<!-- execution: parallel -->
<!-- depends: phase2, phase3 -->

- [ ] Task 1: Implement App struct and state management
  <!-- files: src/app.rs -->
  - [ ] Create `src/app.rs` with `App` struct holding `TreeState`, `should_quit`, `AppMode`
  - [ ] Define `AppMode::Normal` enum (only variant for this milestone)
  - [ ] Implement `App::new(path)` — constructs TreeState from root path, expands root
  - [ ] Implement navigation methods: `select_next()`, `select_previous()`, `select_first()`, `select_last()`
  - [ ] Implement tree methods: `expand_selected()`, `collapse_selected()`, `toggle_hidden()`
  - [ ] Write tests for each navigation and tree method

- [ ] Task 2: Implement tree StatefulWidget
  <!-- files: src/components/tree.rs -->
  - [ ] Create `src/components/tree.rs` with `TreeWidget` implementing ratatui `Widget` (or render function)
  - [ ] Render box-drawing characters: `├──`, `└──`, `│` based on depth and is_last_sibling
  - [ ] Render directory indicators: `▶` collapsed, `▼` expanded
  - [ ] Highlight selected item (reverse video style)
  - [ ] Handle viewport scrolling: ensure selected item is always visible

- [ ] Task 3: Implement UI layout
  <!-- files: src/ui.rs -->
  - [ ] Create `src/ui.rs` with `render()` function
  - [ ] Single-panel layout: tree fills entire terminal area
  - [ ] Draw tree widget with current app state

## Phase 5: Keyboard Navigation & Integration
<!-- execution: sequential -->
<!-- depends: phase4 -->

- [ ] Task 1: Implement key handler
  - [ ] Create `src/handler.rs` with `handle_key_event()` function
  - [ ] Map keys: `j`/`↓` → select_next, `k`/`↑` → select_previous
  - [ ] Map keys: `Enter`/`l`/`→` → expand, `Backspace`/`h`/`←` → collapse/jump-to-parent
  - [ ] Map keys: `g`/`Home` → select_first, `G`/`End` → select_last
  - [ ] Map keys: `.` → toggle_hidden, `q`/`Ctrl+C` → quit
  - [ ] Write tests for each key mapping

- [ ] Task 2: Integration testing and polish
  - [ ] Verify full flow: launch → navigate → expand → collapse → quit
  - [ ] Test with large directory (1000+ files) — ensure no lag on expand
  - [ ] Test with deeply nested directories (10+ levels)
  - [ ] Test hidden file toggle works correctly
  - [ ] Run `cargo clippy -- -D warnings` and `cargo fmt --check`
  - [ ] Ensure all public APIs have doc comments
