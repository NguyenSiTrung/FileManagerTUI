# Initial Concept

> Last refreshed: 2026-09-30 — S3 head preview, S3 theme colors, undo, icons, Windows/distribution

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
- **Simplicity** — Zero config required, works out of the box, single binary deployment (`kubectl cp` or `COPY` in Dockerfile); prebuilt artifacts on GitHub Releases (static musl binary, `.deb`, AppImage, macOS Intel/ARM, Windows)
- **Power** — Full CRUD, clipboard ops, fuzzy search, syntax-highlighted preview, filesystem watching

## Supported Environments
- Standard Linux terminals (xterm, alacritty, gnome-terminal)
- Web-based terminals (KubeFlow, Jupyter, VS Code web)
- tmux / screen sessions
- macOS Terminal / iTerm2
- Windows (x86_64 release build; not covered by CI tests) — S3 browse mode requires a POSIX shell and AWS CLI, so it is effectively Linux/macOS-only

## Key Features
1. **Tree Navigation** — Folder tree with lazy loading, expand/collapse, multi-select, inline filter (`/`), viewport mouse-wheel scrolling with visual scrollbar (drag-to-scroll, click-to-jump), PageUp/PageDown, configurable `scroll_lines`, Nerd Font file-type icons with ASCII fallback (`--no-icons`), `T` to open terminal at selected path
2. **File Preview** — Syntax-highlighted preview panel with theme-aware defaults (auto-selects `InspiredGitHub` for light scheme, `base16-ocean.dark` for dark), streaming head/tail for large files, shallow directory summaries (depth-1) by default with on-demand deep scan (`D` key), cancel-on-navigate, configurable `preview_timeout_ms`, semantic `ThemeColors` for consistent contrast across themes, and double-click line selection with mouse text selection support
3. **File Operations** — Create, rename, delete, copy, cut, paste with confirmation dialogs and async progress; single-level undo (`Ctrl+Z`) for rename, copy-paste, and move-paste
4. **Fuzzy Search + Action Menu** — Ctrl+P fuzzy finder overlay with context-aware action menu (navigate, preview, edit, copy path via `y`, rename, delete, open in terminal)
5. **Filesystem Watcher** — Background watcher with manual refresh (F5/Ctrl+R) and optional auto-refresh mode via config
6. **ML-Aware** — Special handling for .ipynb, .pt, .h5, .csv, .parquet, .yaml files
7. **Configurable** — TOML config, CLI args, themes, keybindings; live settings panel in help overlay (`?` → Settings tab) with live syntax theme reload on scheme change
8. **Embedded Terminal** — Integrated PTY shell panel with VT100 emulation, dynamic resize, scrollback, mouse text selection, and Ctrl+Shift+C copy
9. **Inline Text Editor** — Press `e` in preview to edit files with syntax highlighting (newline-aware for correct scope tracking), undo/redo, find & replace, auto-indent, text selection (Shift+Arrow, Ctrl+A, mouse drag), and mouse cursor positioning
10. **Large Directory Performance** — Paginated directory loading, async expansion with Loading... placeholder, snapshot-based sorting, configurable page size
11. **Large File Handling** — Streaming head/tail preview, editor hard block for >10MB / >100K lines, backward newline scanning for O(N) tail reads
12. **Install Script** — One-command setup (`scripts/install.sh`) — installs Rust if missing, builds, and installs the `fm` binary
13. **Clipboard** — OSC 52 clipboard fallback for headless/remote/web terminal environments; in-TUI copy overlay with disabled mouse capture for browser-native text selection; async clipboard operations to prevent UI freeze; graceful degradation when system clipboard unavailable
14. **Status Bar** — Context-aware key hints per focus panel, copy path to system clipboard (`y`), mouse text selection copy in preview view mode
15. **S3 Browse Mode** — Read-only AWS S3 bucket browsing via `fm s3://bucket/prefix/` with `--aws-profile` support; cloud tree icons (`☁`/`📦`) with theme-aware S3 colors (`s3_dir_fg`/`s3_file_fg`/`s3_border_fg`, per-mode border tint); metadata preview (size, date, URI) with long-URI wrapping; streaming head preview of remote files (`aws s3 cp … - | head -n N`, configurable via `s3_head_lines`, default 100); on-demand file download to temp cache; S3 URI clipboard copy (`y` / `Y` in preview panel); disabled write operations with user-friendly messages; status bar badge (`☁ S3 | s3://bucket`); async directory expansion; graceful AWS CLI error handling (ExpiredToken, AccessDenied, NoSuchBucket)
16. **Retained Documents & Tabs** — Multiple open text documents with per-document cursor/find/undo/wrap state, pinned vs preview tabs, dirty/external-change/readonly markers, and safe saves (collision-checked, revision-validated, permissions preserved)
17. **Command Menu & Keymap Profiles** — Searchable command menu (F8), Standard/Web keymap profiles that avoid browser-reserved Ctrl+P/T/S/R, Alt+G prefix routes, registry-generated help and a live Settings tab
18. **Read-Only Git Indicators** — Branch label on the status bar and modified/untracked tree markers via `git status --porcelain=v2 -z --branch` with `--no-optional-locks`; a missing `git` degrades to no indicators; `--no-git` / `[git] enabled = false` disables entirely
19. **Private Recovery** — Bounded snapshots of dirty documents (0700 dir / 0600 records, workspace+path+disk-revision keyed, retention + age limits) with a relaunch prompt for restore/discard; restoring never writes the original file
20. **Installed-Server LSP** — Optional stdio JSON-RPC language features and diagnostics for user-installed servers (e.g. `pylsp`, `yaml-language-server`, `rust-analyzer`); project-local argv requires an interactive, session-scoped trust grant; durable grants live only in the global config

## Non-Functional Requirements
- Binary size target: < 10MB (static musl build)
- No runtime dependencies (single static binary)
- Full keyboard navigation (mouse optional)
- Unicode/CJK/emoji filename support
