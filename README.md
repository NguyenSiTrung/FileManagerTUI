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
- **AWS S3 browse mode** — read-only browsing of S3 buckets directly from the TUI using the AWS CLI; navigate prefixes, preview metadata, and copy S3 URIs to clipboard

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
```

**Option A — Use the install script (recommended):**

```bash
./scripts/install.sh
```

This handles everything: installs Rust if missing, builds the binary, and installs it as `fm`.

**Option B — Install via Cargo:**

```bash
cargo install --path .
```

**Option C — Manual build and install:**

```bash
cargo build --release
# Copy binary to your PATH
cp target/release/fm ~/.cargo/bin/fm
# Or system-wide:
sudo cp target/release/fm /usr/local/bin/fm
```

> Make sure `~/.cargo/bin` is in your PATH. If not, add to your `~/.bashrc` or `~/.zshrc`:
> ```bash
> source "$HOME/.cargo/env"
> ```

## Prerequisites

- **Rust toolchain** for building from source
- **AWS CLI** (optional) — required only for S3 browse mode. Install via [AWS CLI docs](https://docs.aws.amazon.com/cli/latest/userguide/install-cliv2.html) or `pip install awscli`

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

# Browse an S3 bucket
fm s3://my-bucket

# Browse an S3 prefix with a specific AWS profile
fm s3://my-bucket/experiments/run-1/ --aws-profile mfa
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
| `Ctrl+Shift+C` / `Ctrl+C` (when selected) | Copy selected preview text |
| `Ctrl+Insert` | Copy selected preview text |
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
| Left click (preview) | Focus preview and start text selection |
| Left drag (preview) | Extend text selection |
| Right click (preview) | Copy selected preview text |
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

## S3 Browse Mode

Browse AWS S3 buckets directly from the TUI by passing an `s3://` URI as the path argument. This mode uses the AWS CLI under the hood — no AWS SDK dependency.

### How it works

- **Auto-detection** — If the path starts with `s3://`, `fm` automatically enters S3 browse mode
- **Read-only** — Write operations (create, rename, delete, paste) are disabled with a friendly message
- **On-demand listing** — S3 prefixes are listed asynchronously as you expand directories
- **S3-specific UX** — Cloud icons (☁), peach/amber tree colors, and an `☁ S3` status bar badge
- **Copy S3 URIs** — Press `y` to copy the full `s3://` URI of the selected item to clipboard
- **AWS profile support** — Use `--aws-profile <name>` for MFA or role-based authentication
- **Session-scoped cache** — Downloaded previews are cached under `/tmp/fm-s3-cache-<pid>/` and cleaned up on exit

### Requirements

- AWS CLI (`aws`) must be installed and on `$PATH`
- Valid AWS credentials (via `aws configure`, environment variables, or IAM role)

### Examples

```bash
# Browse a bucket root
fm s3://my-bucket

# Browse a specific prefix
fm s3://my-bucket/experiments/run-1/

# Use a named profile
fm s3://ml-data-bucket --aws-profile mfa
```

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
├── s3/
│   ├── mod.rs         # Module exports
│   ├── backend.rs     # Async S3 backend (shells out to `aws` CLI)
│   ├── parser.rs      # Parser for `aws s3 ls` output
│   └── types.rs       # S3Path, S3Entry, S3Config types
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
