# Initial Concept

A terminal-based file manager TUI (FileManagerTUI) built with Rust and Ratatui, designed for environments like KubeFlow and Jupyter notebooks where folder tree interaction is limited.

# Product Guide

## Vision
A single static binary that provides a VS Code-like file explorer experience in any terminal — fast, simple to deploy, yet powerful enough for daily ML workflows.

## Target Users
- **ML Engineers** working in KubeFlow pods with limited UI tooling
- **Data Scientists** using Jupyter notebooks needing quick file navigation
- **DevOps/SRE Teams** managing Kubernetes workloads via terminal
- **General Developers** who prefer terminal-based file managers

## Core Value Proposition
- **Speed** — Instant startup, lazy loading, async operations; no lag even with thousands of checkpoint files
- **Simplicity** — Zero config required, works out of the box, single binary deployment (`kubectl cp` or `COPY` in Dockerfile)
- **Power** — Full CRUD, clipboard ops, fuzzy search, syntax-highlighted preview, filesystem watching

## Supported Environments
- Standard Linux terminals (xterm, alacritty, gnome-terminal)
- Web-based terminals (KubeFlow, Jupyter, VS Code web)
- tmux / screen sessions
- macOS Terminal / iTerm2

## Key Features
1. **Tree Navigation** — Folder tree with lazy loading, expand/collapse, multi-select, inline filter (`/`), viewport mouse-wheel scrolling with visual scrollbar (drag-to-scroll, click-to-jump), PageUp/PageDown, configurable `scroll_lines`
2. **File Preview** — Syntax-highlighted preview panel with theme-aware defaults (auto-selects `InspiredGitHub` for light scheme, `base16-ocean.dark` for dark), streaming head/tail for large files, shallow directory summaries (depth-1) by default with on-demand deep scan (`D` key), cancel-on-navigate, configurable `preview_timeout_ms`, and semantic `ThemeColors` for consistent contrast across themes
3. **File Operations** — Create, rename, delete, copy, cut, paste with confirmation dialogs and async progress
4. **Fuzzy Search + Action Menu** — Ctrl+P fuzzy finder overlay with context-aware action menu (navigate, preview, edit, copy path via `y`, rename, delete, open in terminal)
5. **Filesystem Watcher** — Background watcher with manual refresh (F5/Ctrl+R) and optional auto-refresh mode via config
6. **ML-Aware** — Special handling for .ipynb, .pt, .h5, .csv, .parquet, .yaml files
7. **Configurable** — TOML config, CLI args, themes, keybindings; live settings panel in help overlay (`?` → Settings tab) with live syntax theme reload on scheme change
8. **Embedded Terminal** — Integrated PTY shell panel with VT100 emulation, dynamic resize, scrollback, mouse text selection, and Ctrl+Shift+C copy
9. **Inline Text Editor** — Press `e` in preview to edit files with syntax highlighting, undo/redo, find & replace, auto-indent, text selection (Shift+Arrow, Ctrl+A, mouse drag), and mouse cursor positioning
10. **Large Directory Performance** — Paginated directory loading, async expansion with Loading... placeholder, snapshot-based sorting, configurable page size
11. **Large File Handling** — Streaming head/tail preview, editor hard block for >10MB / >100K lines, backward newline scanning for O(N) tail reads
12. **Install Script** — One-command setup (`scripts/install.sh`) — installs Rust if missing, builds, and installs the `fm` binary
13. **Clipboard** — OSC 52 clipboard fallback for headless/remote/web terminal environments; graceful degradation when system clipboard unavailable
14. **Status Bar** — Context-aware key hints per focus panel, copy path to system clipboard (`y`), mouse text selection copy in preview view mode

## Non-Functional Requirements
- Binary size target: < 10MB (static musl build)
- No runtime dependencies (single static binary)
- Full keyboard navigation (mouse optional)
- Unicode/CJK/emoji filename support
