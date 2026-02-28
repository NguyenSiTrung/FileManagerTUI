# fm-tui

A fast, keyboard-driven terminal file manager built with Rust and [Ratatui](https://ratatui.rs).

![Rust](https://img.shields.io/badge/language-Rust-orange)
![License](https://img.shields.io/badge/license-MIT-blue)

## Features

- **Dual-pane layout** — file tree + live preview with syntax highlighting
- **Vim-style navigation** — `j`/`k`/`g`/`G` and arrow keys
- **Fuzzy finder** — `Ctrl+P` for project-wide file search with action menu
- **Inline filter** — `/` to filter the current directory tree
- **File operations** — create, rename, delete, copy, cut, paste with undo
- **Multi-select** — `Space` to select, batch operations on selection
- **Nerd Font icons** — file-type icons with ASCII fallback (`--no-icons`)
- **Mouse support** — click to select, scroll wheel, panel switching
- **Sort options** — sort by name, size, or modified time; toggle dirs-first
- **Configurable themes** — built-in dark (Catppuccin Mocha) / light (Catppuccin Latte) + custom colors
- **TOML configuration** — multi-source config with CLI overrides
- **File watcher** — auto-refresh on filesystem changes with debounce
- **Jupyter notebook preview** — renders `.ipynb` cells with syntax highlighting
- **Large file handling** — head/tail preview mode for files over configurable threshold
- **Embedded terminal** — integrated PTY shell panel with VT100 emulation, dynamic resize, and scrollback
- **Inline text editor** — press `e` in preview to edit files with syntax highlighting, undo/redo, find & replace, auto-indent, text selection (Shift+Arrow, Ctrl+A, mouse drag), and mouse cursor positioning

## Installation

### From Source

```bash
cargo install --git https://github.com/NguyenSiTrung/FileManagerTUI.git
```

### From GitHub Releases

Download the latest binary for your platform from the [Releases](https://github.com/NguyenSiTrung/FileManagerTUI/releases) page:

| Platform | Binary |
|----------|--------|
| Linux (x86_64, static) | `fm-x86_64-unknown-linux-musl` |
| macOS (Intel) | `fm-x86_64-apple-darwin` |
| macOS (Apple Silicon) | `fm-aarch64-apple-darwin` |
| Windows | `fm-x86_64-pc-windows-msvc.exe` |

```bash
# Linux / macOS
chmod +x fm-*
sudo mv fm-* /usr/local/bin/fm
```

### Build from Source

```bash
git clone https://github.com/NguyenSiTrung/FileManagerTUI.git
cd FileManagerTUI
cargo build --release
# Binary at target/release/file_manager_tui
```

## Usage

```bash
# Open current directory
fm

# Open a specific path
fm ~/projects

# Use a custom config file
fm -c ~/.config/fm-tui/config.toml ~/projects

# Minimal mode (no icons, no mouse, no watcher)
fm --no-icons --no-mouse --no-watcher

# Disable embedded terminal
fm --no-terminal

# Light theme
fm --theme light
```

## Keybindings

### Navigation (Tree Panel)

| Key | Action |
|-----|--------|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| `g` / `Home` | Jump to first item |
| `G` / `End` | Jump to last item |
| `Enter` / `l` / `→` | Expand directory |
| `Backspace` / `h` / `←` | Collapse directory / go to parent |
| `Tab` | Cycle panel focus (forward) |
| `Ctrl+←/→` | Focus left/right panel |
| `Ctrl+↑/↓` | Focus up/down (terminal) |
| `.` | Toggle hidden files |
| `Space` | Toggle multi-select |
| `Esc` | Clear multi-selection |
| `s` | Cycle sort (name → size → modified) |
| `S` | Toggle directories first |

### File Operations

| Key | Action |
|-----|--------|
| `a` | Create new file |
| `A` | Create new directory |
| `r` | Rename |
| `d` | Delete |
| `y` | Copy to clipboard |
| `x` | Cut to clipboard |
| `p` | Paste from clipboard |
| `Ctrl+Z` | Undo last operation |

### Search & Filter

| Key | Action |
|-----|--------|
| `Ctrl+P` | Open fuzzy finder |
| `/` | Start inline filter |
| `Esc` | Cancel / clear filter |
| `Enter` | Accept filter / Open action menu |

### Search Action Menu

After selecting a file in the fuzzy finder, an action menu appears:

| Key | Action |
|-----|--------|
| `Enter` | Navigate (Go to file in tree) |
| `p` | Preview (navigate + focus preview) |
| `e` | Edit (open inline editor) |
| `y` | Copy absolute path to system clipboard |
| `r` | Rename file |
| `d` | Delete file |
| `c` | Copy to clipboard |
| `x` | Cut to clipboard |
| `t` | Open parent dir in terminal |
| `Esc` | Back to search results |

> **Context filtering:** Edit/Preview are hidden for directories; Edit is hidden for binary files.

### Preview Panel

| Key | Action |
|-----|--------|
| `j` / `↓` | Scroll down |
| `k` / `↑` | Scroll up |
| `g` / `Home` | Jump to top |
| `G` / `End` | Jump to bottom |
| `Ctrl+D` | Half page down |
| `Ctrl+U` | Half page up |
| `Ctrl+W` | Toggle line wrap |
| `Ctrl+T` | Cycle view mode (head/tail/full for large files) |
| `+` / `-` | Adjust head/tail lines |
| `e` | Enter edit mode |

### Editor Mode (Preview)

| Key | Action |
|-----|--------|
| `Esc` | Exit edit mode (prompt if unsaved) |
| `Ctrl+S` | Save file |
| `Arrow keys` | Move cursor |
| `Home` / `End` | Start / end of line |
| `Ctrl+Home` / `Ctrl+End` | Top / bottom of file |
| `PgUp` / `PgDn` | Page up / page down |
| `Tab` / `Shift+Tab` | Indent / dedent |
| `Ctrl+Z` | Undo |
| `Ctrl+Y` | Redo |
| `Ctrl+C` | Copy line |
| `Ctrl+X` | Cut line |
| `Ctrl+V` | Paste |
| `Ctrl+F` | Find |
| `Ctrl+H` | Find & Replace |
| `Ctrl+A` (in replace) | Replace all |
| `Shift+Arrow` | Extend text selection |
| `Ctrl+A` | Select all text |
| Mouse click | Position cursor at click point |
| Mouse drag | Select text |
| Scroll wheel | Scroll editor viewport |

### Terminal Panel

| Key | Action |
|-----|--------|
| `Ctrl+T` | Toggle terminal panel |
| `Ctrl+Shift+↑` | Decrease terminal height |
| `Ctrl+Shift+↓` | Increase terminal height |
| `Esc` | Unfocus terminal (return to tree) |
| `Shift+↑/↓` | Scroll terminal history |
| `Shift+PgUp/PgDn` | Fast scroll terminal history |

> When the terminal is focused, all other keys are forwarded to the shell.

### General

| Key | Action |
|-----|--------|
| `?` | Toggle help overlay |
| `q` | Quit |
| `Ctrl+C` | Quit |
| `F5` | Manual refresh |
| `Ctrl+R` | Toggle file watcher |

### Mouse

| Action | Behavior |
|--------|----------|
| Left click (tree) | Select item |
| Left click (selected dir) | Expand/collapse |
| Left click (preview) | Switch focus to preview |
| Scroll wheel | Navigate tree / scroll preview |

## Configuration

Configuration is loaded from multiple sources with the following priority (highest wins):

1. **CLI flags** (e.g., `--no-mouse`, `--theme light`)
2. **Environment variable** `$FM_TUI_CONFIG` (path to config file)
3. **Local config** `.fm-tui.toml` in current directory
4. **Global config** `~/.config/fm-tui/config.toml`
5. **Built-in defaults**

### Example `config.toml`

```toml
[general]
show_hidden = false
confirm_delete = true
mouse = true

[preview]
enabled = true
max_full_preview_bytes = 1048576  # 1 MB
head_lines = 100
tail_lines = 50
default_view_mode = "full"  # "full", "head_tail", "head_only", "tail_only"
tab_width = 4
line_wrap = false
syntax_theme = "base16-ocean.dark"

[tree]
sort_by = "name"       # "name", "size", "modified"
dirs_first = true
use_icons = true       # Set to false for ASCII-only mode

[watcher]
enabled = true
debounce_ms = 300

[theme]
scheme = "dark"        # "dark" or "light"

# Optional custom color overrides (hex format)
[theme.custom]
tree_dir_fg = "#89b4fa"
tree_file_fg = "#cdd6f4"
tree_hidden_fg = "#585b70"
tree_selected_bg = "#45475a"
tree_selected_fg = "#cdd6f4"
border_fg = "#585b70"
border_focused_fg = "#89b4fa"
status_bar_bg = "#1e1e2e"
status_bar_fg = "#cdd6f4"
preview_fg = "#cdd6f4"
dialog_bg = "#313244"
dialog_fg = "#cdd6f4"
```

## Built-in Themes

### Dark (Catppuccin Mocha) — Default

Based on the [Catppuccin Mocha](https://catppuccin.com/) color palette with deep blues, soft purples, and warm accents.

### Light (Catppuccin Latte)

Based on [Catppuccin Latte](https://catppuccin.com/) for well-lit environments with a clean light background.

## Architecture

```
src/
├── main.rs            # Entry point, CLI parsing, event loop
├── app.rs             # Application state and logic
├── handler.rs         # Key/mouse event dispatch
├── ui.rs              # Layout and rendering
├── tui.rs             # Terminal setup/teardown
├── event.rs           # Event system (key, mouse, tick, async)
├── config.rs          # TOML configuration loading and merging
├── theme.rs           # Theme colors and palettes
├── error.rs           # Error types
├── preview_content.rs # Syntax highlighting, notebook rendering
├── editor.rs          # Editor state, undo/redo, find/replace
├── components/
│   ├── tree.rs        # File tree widget with icons
│   ├── preview.rs     # Preview pane widget
│   ├── editor.rs      # Editor widget (line numbers, cursor, find bar)
│   ├── status_bar.rs  # Status bar widget
│   ├── dialog.rs      # Modal dialog widget
│   ├── search.rs      # Fuzzy finder overlay
│   ├── search_action.rs # Search action menu overlay
│   ├── help.rs        # Help overlay widget
│   └── terminal.rs    # Terminal panel widget
├── fs/
│   ├── tree.rs        # Tree data structure, sorting, filtering
│   ├── operations.rs  # File CRUD operations
│   ├── clipboard.rs   # Copy/cut/paste state
│   └── watcher.rs     # Filesystem watcher with debounce
└── terminal/
    ├── mod.rs         # Module exports, PtyProcess struct
    ├── pty.rs         # PTY creation and async I/O
    └── emulator.rs    # VTE-based terminal emulator
```

## Development

```bash
# Run tests
cargo test

# Run with clippy
cargo clippy -- -D warnings

# Format check
cargo fmt --check

# Run in development
cargo run -- .
```

## License

MIT License. See [LICENSE](LICENSE) for details.
