# Plan: Core Loop + Tree Rendering

## Phase 1: Project Setup & Error Foundation
<!-- execution: sequential -->

- [x] Task 1: Initialize Cargo project with dependencies
  - [x] Create `Cargo.toml` with: ratatui (0.29, crossterm), crossterm (0.28), tokio (1, full), thiserror, clap (4, derive)
  - [x] Create directory structure: `src/`, `src/fs/`, `src/components/`
  - [x] Create module files with `mod` declarations: `src/fs/mod.rs`, `src/components/mod.rs`
  - [x] Verify `cargo check` passes

- [x] Task 2: Define custom error types
  - [x] Create `src/error.rs` with `AppError` enum using `thiserror` (Io, Terminal, InvalidPath variants)
  - [x] Define `pub type Result<T> = std::result::Result<T, AppError>` alias
  - [x] Write unit tests for error conversions
  - [x] Verify `cargo test` passes

## Phase 2: TreeNode Data Structure & Lazy Loading
<!-- execution: sequential -->
<!-- depends: phase1 -->

- [x] Task 1: Implement TreeNode struct and types
  <!-- files: src/fs/tree.rs -->
  - [x] Create `src/fs/tree.rs` with `TreeNode`, `NodeType`, `FileMeta` structs
  - [x] Implement `TreeNode::new()` constructor from a path
  - [x] Write unit tests for TreeNode creation (file, directory, symlink)

- [x] Task 2: Implement lazy directory loading
  <!-- files: src/fs/tree.rs -->
  <!-- depends: task1 -->
  - [x] Implement `TreeNode::load_children()` — reads directory, creates child nodes
  - [x] Sort children: directories first, then alphabetical case-insensitive
  - [x] Handle permission-denied and broken symlinks gracefully (skip with warning)
  - [x] Write tests: load directory, sort order, empty directory, hidden files

- [x] Task 3: Implement tree flattening and TreeState
  <!-- files: src/fs/tree.rs -->
  <!-- depends: task2 -->
  - [x] Create `FlatItem` struct with depth, name, path, node_type, is_expanded, is_last_sibling
  - [x] Implement `TreeState` with root, flat_items, selected_index, scroll_offset, show_hidden
  - [x] Implement `TreeState::flatten()` — recursive walk producing `Vec<FlatItem>`, respecting show_hidden
  - [x] Implement `TreeState::toggle_hidden()` — re-flattens with updated visibility
  - [x] Write tests: flatten expanded tree, hidden file filtering, is_last_sibling correctness

## Phase 3: Terminal Init & Async Event Loop
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [x] Task 1: Implement terminal setup and teardown
  <!-- files: src/tui.rs -->
  - [x] Create `src/tui.rs` with `Tui` struct wrapping `Terminal<CrosstermBackend<Stdout>>`
  - [x] Implement `Tui::new()` — enter alternate screen, enable raw mode
  - [x] Implement `Tui::restore()` — leave alternate screen, disable raw mode
  - [x] Install panic hook that restores terminal before printing panic info

- [x] Task 2: Implement async event system
  <!-- files: src/event.rs -->
  - [x] Create `src/event.rs` with `Event` enum (Key, Tick, Resize)
  - [x] Implement `EventHandler` struct — spawns tokio task polling crossterm events
  - [x] Use `tokio::sync::mpsc` channel to forward events
  - [x] Configurable tick rate (default 16ms)

- [x] Task 3: Wire up main.rs entry point
  <!-- files: src/main.rs -->
  <!-- depends: task1, task2 -->
  - [x] Create `src/main.rs` with `#[tokio::main]`, clap CLI arg for path
  - [x] Initialize Tui, EventHandler, App
  - [x] Implement main loop: receive event → handle → draw → check should_quit
  - [x] Verify: `cargo run -- .` launches and `q` exits cleanly

## Phase 4: App State & Tree Widget Rendering
<!-- execution: parallel -->
<!-- depends: phase2, phase3 -->

- [x] Task 1: Implement App struct and state management
  <!-- files: src/app.rs -->
  - [x] Create `src/app.rs` with `App` struct holding `TreeState`, `should_quit`, `AppMode`
  - [x] Define `AppMode::Normal` enum (only variant for this milestone)
  - [x] Implement `App::new(path)` — constructs TreeState from root path, expands root
  - [x] Implement navigation methods: `select_next()`, `select_previous()`, `select_first()`, `select_last()`
  - [x] Implement tree methods: `expand_selected()`, `collapse_selected()`, `toggle_hidden()`
  - [x] Write tests for each navigation and tree method

- [x] Task 2: Implement tree StatefulWidget
  <!-- files: src/components/tree.rs -->
  - [x] Create `src/components/tree.rs` with `TreeWidget` implementing ratatui `Widget` (or render function)
  - [x] Render box-drawing characters: `├──`, `└──`, `│` based on depth and is_last_sibling
  - [x] Render directory indicators: `▶` collapsed, `▼` expanded
  - [x] Highlight selected item (reverse video style)
  - [x] Handle viewport scrolling: ensure selected item is always visible

- [x] Task 3: Implement UI layout
  <!-- files: src/ui.rs -->
  - [x] Create `src/ui.rs` with `render()` function
  - [x] Single-panel layout: tree fills entire terminal area
  - [x] Draw tree widget with current app state

## Phase 5: Keyboard Navigation & Integration
<!-- execution: sequential -->
<!-- depends: phase4 -->

- [x] Task 1: Implement key handler
  - [x] Create `src/handler.rs` with `handle_key_event()` function
  - [x] Map keys: `j`/`↓` → select_next, `k`/`↑` → select_previous
  - [x] Map keys: `Enter`/`l`/`→` → expand, `Backspace`/`h`/`←` → collapse/jump-to-parent
  - [x] Map keys: `g`/`Home` → select_first, `G`/`End` → select_last
  - [x] Map keys: `.` → toggle_hidden, `q`/`Ctrl+C` → quit
  - [x] Write tests for each key mapping

- [x] Task 2: Integration testing and polish
  - [x] Verify full flow: launch → navigate → expand → collapse → quit
  - [x] Test with large directory (1000+ files) — ensure no lag on expand
  - [x] Test with deeply nested directories (10+ levels)
  - [x] Test hidden file toggle works correctly
  - [x] Run `cargo clippy -- -D warnings` and `cargo fmt --check`
  - [x] Ensure all public APIs have doc comments
