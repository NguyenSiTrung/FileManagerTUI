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
1. **Tree Navigation** — Folder tree with lazy loading, expand/collapse, multi-select
2. **File Preview** — Syntax-highlighted preview panel with head+tail for large files
3. **File Operations** — Create, rename, delete, copy, cut, paste with confirmation dialogs
4. **Fuzzy Search** — Ctrl+P fuzzy finder overlay for quick file location
5. **Filesystem Watcher** — Auto-refresh tree on external changes
6. **ML-Aware** — Special handling for .ipynb, .pt, .h5, .csv, .parquet, .yaml files
7. **Configurable** — TOML config, CLI args, themes, keybindings

## Non-Functional Requirements
- Binary size target: < 10MB (static musl build)
- No runtime dependencies (single static binary)
- Full keyboard navigation (mouse optional)
- Unicode/CJK/emoji filename support
