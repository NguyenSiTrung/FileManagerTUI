# Spec: Core Loop + Tree Rendering

## Overview

Implement the foundational layer of FileManagerTUI: a terminal application that displays a navigable folder tree using Ratatui and Crossterm. This is the MVP starting point — a user launches the binary with a path argument and can fully navigate the filesystem tree using keyboard controls.

The event loop is async (tokio) from the start to match the final architecture. The tree renders with Unicode box-drawing characters (`├──`, `└──`, `│`). Error handling uses a custom `AppError` enum via `thiserror`.

## Functional Requirements

### FR-1: Project Scaffolding
- Initialize Rust project with `Cargo.toml` containing all Milestone 1 dependencies: `ratatui` (0.29, crossterm feature), `crossterm` (0.28), `tokio` (1, full features), `thiserror`, `clap` (4, derive)
- Establish directory structure: `src/main.rs`, `src/app.rs`, `src/event.rs`, `src/handler.rs`, `src/ui.rs`, `src/fs/mod.rs`, `src/fs/tree.rs`, `src/components/mod.rs`, `src/components/tree.rs`

### FR-2: Terminal Initialization & Restoration
- `main.rs` initializes crossterm: enable raw mode, enter alternate screen, enable mouse capture (optional)
- On exit (normal or panic), restore terminal state: disable raw mode, leave alternate screen
- Accept a path argument via `clap` (defaults to current directory)

### FR-3: Async Event Loop
- Tokio-based async runtime in `main.rs`
- `event.rs`: Poll crossterm events asynchronously, forward to handler
- Tick-based rendering at configurable interval (default ~60fps / 16ms)
- Clean shutdown on `q` / `Ctrl+C`

### FR-4: TreeNode with Lazy Directory Loading
- `fs/tree.rs`: `TreeNode` struct with `name`, `path`, `node_type` (File/Directory/Symlink), `children: Option<Vec<TreeNode>>`, `is_expanded`, `depth`, `metadata` (size, modified, permissions, is_hidden)
- Lazy loading: `children = None` until directory is expanded; on expand, read directory and populate children
- Sorting: directories first, then alphabetical (case-insensitive)
- `TreeState`: holds root node, flattened items list, selected index, scroll offset, show_hidden flag

### FR-5: Tree Widget Rendering
- `components/tree.rs`: Implement Ratatui `StatefulWidget` for the tree
- Unicode box-drawing: `├──` for siblings, `└──` for last sibling, `│` for continuation lines
- Visual indicators: `▶` / `▼` for collapsed/expanded directories, distinct styling for files vs directories
- Selected item highlighting (reverse video style)
- Scroll support: viewport follows selected item

### FR-6: Keyboard Navigation
- `j` / `↓`: Move selection down
- `k` / `↑`: Move selection up
- `Enter` / `l` / `→`: Expand directory or no-op on file
- `Backspace` / `h` / `←`: Collapse directory or jump to parent
- `g` / `Home`: Jump to first item
- `G` / `End`: Jump to last item
- `.` (dot): Toggle show/hide hidden files
- `q` / `Ctrl+C`: Quit

### FR-7: App State
- `app.rs`: `App` struct holding `TreeState`, `should_quit` flag, current `AppMode` (only `Normal` for this milestone)
- Methods: `toggle_hidden()`, `expand()`, `collapse()`, `select_next()`, `select_previous()`, `select_first()`, `select_last()`

## Non-Functional Requirements

- **NFR-1**: Terminal compatibility — must work in xterm, alacritty, tmux, Jupyter terminal, KubeFlow web terminal (crossterm backend ensures this)
- **NFR-2**: Performance — lazy loading must handle directories with 1000+ entries without lag
- **NFR-3**: Error handling — custom `AppError` enum via `thiserror`; no `.unwrap()` in non-test code; terminal always restored on error
- **NFR-4**: Code quality — `cargo clippy -- -D warnings` clean, `cargo fmt` formatted, public APIs documented

## Acceptance Criteria

1. `cargo run -- /some/path` launches the TUI showing the folder tree rooted at the given path
2. Navigating with `j`/`k` moves selection up/down with visual highlight
3. `Enter` on a directory expands it (loads children lazily); `Backspace` collapses it
4. `g`/`G` jump to first/last item
5. `.` toggles hidden file visibility
6. `q` or `Ctrl+C` cleanly exits and restores terminal
7. Unicode box-drawing characters render correctly for tree structure
8. Directories with 1000+ files load without noticeable delay
9. All tests pass, clippy clean, formatted

## Out of Scope

- File preview panel (Milestone 3)
- File operations — create, rename, delete (Milestone 2)
- Fuzzy search (Milestone 5)
- Filesystem watcher (Milestone 6)
- Configuration file loading (Milestone 7)
- Mouse interaction (Milestone 7)
- Nerd Font icons (Milestone 7)
- Multi-select (Milestone 4)
