# fm-tui

A fast, keyboard-driven terminal file manager built with Rust and [Ratatui](https://ratatui.rs).

![Rust](https://img.shields.io/badge/language-Rust-orange)
![License](https://img.shields.io/badge/license-MIT-blue)

## Features

- **Dual-pane layout** — file tree + live preview with syntax highlighting
- **Vim-style navigation** — `j`/`k`/`g`/`G` and arrow keys
- **Quick Open** — filename finder with direct document opening and secondary file actions
- **Inline filter** — `/` to filter the current directory tree
- **File operations** — create, rename, delete, copy, cut, paste with undo
- **Multi-select** — `Space` to select, batch operations on selection
- **Nerd Font icons** — file-type icons with ASCII fallback (`--no-icons`)
- **Mouse support** — click to select, scroll wheel, panel switching
- **Sort options** — sort by name, size, or modified time; toggle dirs-first
- **Configurable themes** — built-in dark (Catppuccin Mocha) / light (Catppuccin Latte) + custom colors
- **TOML configuration** — multi-source config with CLI overrides
- **File watcher** — optional, disabled by default; opt-in auto-refresh with debounce, native event or polling backend for mounted storage
- **Jupyter notebook preview** — renders `.ipynb` cells with syntax highlighting
- **Large file handling** — head/tail preview mode for files over configurable threshold
- **Embedded terminal** — integrated PTY shell panel with VT100 emulation, dynamic resize, and scrollback
- **Retained text documents** — UTF-8 editing, independent tabs/history/find/scroll/wrap, syntax highlighting, safe saves, undo/redo and selections
- **Command menu and keymaps** — configurable Standard/Web profiles, registry-generated help and live settings
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
| Linux (.deb package) | `fm-tui_<version>_amd64.deb` |
| Linux (AppImage) | `fm-tui-x86_64.AppImage` |
| macOS (Intel) | `fm-x86_64-apple-darwin` |
| macOS (Apple Silicon) | `fm-aarch64-apple-darwin` |
| Windows | `fm-x86_64-pc-windows-msvc.exe` |

```bash
# Linux / macOS — raw binary
chmod +x fm-*
sudo mv fm-* /usr/local/bin/fm

# Linux — Debian/Ubuntu .deb package
sudo dpkg -i fm-tui_*_amd64.deb

# Linux — AppImage (portable, no install needed)
chmod +x fm-tui-x86_64.AppImage
./fm-tui-x86_64.AppImage
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

# Browser-terminal workspace routes (explicit opt-in)
fm --keymap-profile web

# Disable automatic preview, not explicit text document opening
fm --no-preview

# Light theme
fm --theme light

# Browse an S3 bucket
fm s3://my-bucket

# Browse an S3 prefix with a specific AWS profile
fm s3://my-bucket/experiments/run-1/ --aws-profile mfa
```

## Documents and workspace commands

Enter on a file and Quick Open open supported local text directly. Clean automatic
preview documents may be replaced; editing or pinning retains a document. Tabs,
next/previous/list navigation preserve each document's cursor, find query, undo,
dirty content and view state. Binary/invalid-UTF-8/oversized/read-only files and
notebooks keep supported read-only preview behavior; S3 remains read-only.

Saving targets the retained/captured document, never an unrelated selected
preview. Close and quit offer save/discard/cancel for dirty documents; cancelled
or failed saves retain their buffers. External disk revisions stop unsafe saves;
Save As creates a new destination with explicit overwrite/conflict checks.

F8 or the status-bar **Commands** mouse button opens a searchable command menu.
The rendered button and menu title reflect current bindings; the button remains
clickable when keyboard entry is unbound. Menu dismissal uses its own configured
Menu-context bindings plus native Esc. Help (`?` in tree/preview) lists the
originating context's active bindings, unbound commands and disabled reasons.
Git indicators, private recovery and installed-server LSP are implemented and
described below. Commands remain context-gated: an unavailable action reports
its reason rather than running in the wrong context.

Both profiles provide **Alt+G, then a suffix** in ordinary tree/preview/editor/shell
contexts:

| Suffix | Command |
|---|---|
| `m` | Commands menu |
| `s` / `a` / `q` | Save / Save As / Close retained document |
| `o` | Quick Open |
| `d` / `n` / `b` | List / next / previous document |
| `1` / `2` / `3` / `4` | Explorer / retained editor / shell / selected preview focus |
| `t` / `w` | Toggle terminal / wrap the actual text view |

Standard also retains contextual Ctrl+S/Ctrl+P/Ctrl+T and directional focus
shortcuts. Web omits those registered Ctrl routes, avoiding exclusive dependence
on browser-reserved Ctrl+P/T/S/R. Profiles are explicit, not auto-detected. Browser,
OS, locale and extension delivery still varies: controlled PTY tests are **not**
real-browser or Kubeflow certification. Native editor controls remain native.

A pending prefix times out after 1200 ms by default (100–10000 ms configurable).
Esc, mismatch, timeout or context change cancels and consumes the suffix, without
replaying it as shell/editor/tree input. Paste, mouse-down, document activation,
focus and live replacement reset sequences. External cancellation/replacement of
a pending prefix quarantines one subsequent key, deliberately dropping a delayed
suffix rather than risking a destructive action. Literal textual prefixes,
conflicting/unknown bindings and nonportable encodings are rejected.

## Native panel controls (default Standard profile)

These controls are panel-local; explicit workspace overrides take precedence.
Workspace commands and their authoritative configured shortcuts are in Help.

### Navigation (Tree Panel)

| Key | Action |
|-----|--------|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| `g` / `Home` | Jump to first item |
| `G` / `End` | Jump to last item |
| `Enter` | Expand directory / open text document |
| `l` / `→` | Expand directory |
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

Quick Open normally opens directly. Its secondary action menu offers:

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
| `Alt+G w` / Commands → Wrap | Toggle the actual text view’s wrap |
| `Ctrl+Shift+C` / `Ctrl+C` (when selected) | Copy selected preview text |
| Right click | Copy selected preview text |
| Commands → Cycle selected preview mode | Full → head/tail → head → tail; full respects byte budget |
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
| `Ctrl+C` | Copy selection, or current line |
| `Ctrl+X` | Cut selection, or current line |
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
| `Ctrl+Shift+↑` | Grow terminal height (Standard profile) |
| `Ctrl+Shift+↓` | Shrink terminal height (Standard profile) |
| `Esc` / `q` / `Tab` / `Ctrl+C` | Forward to shell (including Ctrl+C with selected text) |
| `Shift+↑/↓` | Scroll terminal history |
| `Shift+PgUp/PgDn` | Fast scroll terminal history |

> Only configured workspace routes and native terminal scroll/distinct copy
> actions are intercepted. Shell Ctrl+A/E/U/K/W retain their normal semantics;
> Ctrl+W is not wrap. Unreserved Alt chords forward their ESC prefix. Copy uses
> distinctly delivered Ctrl+Shift+C/Super+C or right-click, not ordinary Ctrl+C.

### General

| Key | Action |
|-----|--------|
| `?` | Toggle help overlay |
| `q` | Quit in tree/preview |
| `Ctrl+C` | Quit in tree/preview (selected preview text copies instead) |
| `F5` | Manual refresh |
| `Ctrl+R` | Toggle tree auto-refresh (open-document change detection stays active) |

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

1. **CLI flags** (e.g., `--no-mouse`, `--theme light`, `--keymap-profile web`)
2. **Explicit `--config` / `-c` file**
3. **Environment variable** `$FM_TUI_CONFIG` (path to config file)
4. **Local config** `.fm-tui.toml` in current directory
5. **Global config** `~/.config/fm-tui/config.toml`
6. **Built-in defaults**

### Configurable bindings

```toml
[keymap]
profile = "web"
timeout_ms = 1200

[[keymap.bindings]]
command = "document.save"
context = "editor"
keys = ["F9", "Alt+G s"]

# Remove all terminal-toggle routes in shell context.
[[keymap.bindings]]
command = "pane.terminal.toggle"
context = "terminal"
keys = []

# Menu dismissal is independent of editor entry.
[[keymap.bindings]]
command = "workspace.commands"
context = "menu"
keys = ["F9"]
```

Sources merge by `(command, context)`; each incoming pair replaces that pair's
complete profile/lower-priority routes. Omitted pairs remain; `keys = []` unbinds.
A CLI profile override preserves TOML binding overrides. Include original keys to
retain them when adding a key. Startup config errors fail before TUI setup.

Help's Settings tab registers an explicit profile selector and merges partial
settings with the current effective config. Compilation and file validation
precede atomic persistence and live application. Failure reports no save/apply,
keeps previous live bindings/state and retains unsaved settings edits.
Settings saves capture exact source bytes and their revision together before
merging, then reuse the shared bounded safe-save policy. Source/generated config
are limited to 1 MiB; validation reads are capped at the captured revision size
plus one growth-detection byte. Existing private modes and ownership are preserved
or the save refuses; symlinked destinations/ancestors, hardlinks, read-only files
and unpreservable ownership/extended permissions are refused without dropping the
original file or live state. New Unix configs use a restrictive 0600 default.
Changed/deleted/replaced destinations and creation collisions fail safely.
Final revision validation followed by publication remains advisory under trusted
parents, not filesystem compare-and-swap; a last validation/publication race is
not claimed closed.

Preview disable (`--no-preview` or `[preview] enabled = false`) suppresses automatic
legacy preview reads/admission, but explicit editable opens and retained editors
remain available. Wrap changes the originating text view only; other retained
documents keep their local wrap/horizontal offsets. `full` respects the configured
byte budget; `head_tail` is an alias for `head_and_tail`. View cycling is a menu
command, not Ctrl+T. Editor byte/line limits apply to new admission without
silently evicting existing buffers.

Watcher preferences survive editor/focus transitions. The in-app `Ctrl+R` toggle
and `watcher.auto_refresh` control **tree auto-refresh only**: while they are off,
already-open documents still detect external writes (no global suppression). To
stop change detection entirely, launch with `--no-watcher` or set
`watcher.enabled = false`; configuration-disable cannot be temporarily resumed by
Ctrl+R, and enabling a watcher that was not started at launch requires restart.
Backend debounce, mouse capture and default shell
startup options require restart; changing the default shell does not restart an
existing process. Theme changes retain the existing live highlighting policy.
Terminal scrollback applies at startup and live, clamped to **0–100000 lines**
(default 1000). Shrink removes only oldest history, clamps scroll position and
clears/rebases affected selections; grid, cursor and process remain intact.
Changing history capacity never spawns a shell or enables a disabled terminal.

Editor paste is literal text (not simulated keys), with a 1 MiB bound. Text
clipboard fallbacks use internal text, optional platform tools, OSC52 and a
selectable copy overlay. OSC52 transport is unconfirmed: it does not prove the
host/browser clipboard changed. File copy/cut/paste remains separate from text
clipboard. No real-clipboard browser certification is claimed.

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
default_view_mode = "full"  # bounded full, head_and_tail (alias head_tail), head_only, tail_only
tab_width = 4
line_wrap = false
syntax_theme = "base16-ocean.dark"

[tree]
sort_by = "name"       # "name", "size", "modified"
dirs_first = true
use_icons = true       # Set to false for ASCII-only mode

[watcher]
enabled = true           # false disables change detection entirely (no backend)
auto_refresh = false     # tree auto-refresh only; open-document detection stays on
debounce_ms = 300
mode = "event"           # "event" (native notifications) or "polling" (mounted storage)
poll_interval_ms = 2000  # polling only; clamped to 250..=60000

[git]
enabled = true           # read-only branch/status indicators; --no-git overrides

[recovery]
enabled = true           # private snapshots of dirty documents
# state_dir = "~/.local/state/fm-tui/recovery"  # defaults to the platform state dir
max_records = 32         # retained snapshots per workspace, clamped 1..=1024
max_age_secs = 604800    # 7 days; clamped 3600..=~100 years
min_interval_ms = 2000   # throttle between snapshot writes

[lsp]
enabled = true           # subsystem gate; servers still need a resolvable argv

# Installed-server examples — executable must exist on PATH (or absolute path).
[lsp.servers.python]
argv = ["pylsp"]                    # pip install python-lsp-server
# argv = ["pyright-langserver", "--stdio"]  # npm i -g pyright
root_markers = ["pyproject.toml", "setup.py", ".git"]

[lsp.servers.yaml]
argv = ["yaml-language-server", "--stdio"]  # npm i -g yaml-language-server
root_markers = [".git"]

[lsp.servers.rust]
argv = ["rust-analyzer"]            # rustup component add rust-analyzer
root_markers = ["Cargo.toml"]

# Map an extension to a configured language.
[lsp.languages]
templ = "html"

# Durable execution grant — global config only. Project-local
# [[lsp.trust]] entries are ignored on purpose: a checked-out repo must
# not be able to grant its own argv execution.
[[lsp.trust]]
root = "~/src/myproject"
argv = ["pylsp"]

[terminal]
enabled = true
scrollback_lines = 1000  # clamped to 0..100000, live without shell restart

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

## Git indicators (read-only)

Inside a Git work tree the status bar shows the branch label, and the tree
marks modified/untracked files and directories containing them. The backend
is strictly read-only: the only query ever run is
`git status --porcelain=v2 -z --branch`, always with `--no-optional-locks`
and `GIT_OPTIONAL_LOCKS=0`, so status can never take `index.lock` or write
to the repository. A missing or failing `git` executable degrades silently
to no indicators. Disable with `--no-git` or `[git] enabled = false`; both
remove every decoration and stop further queries.

## Language servers (LSP)

fm can talk to **installed** language servers over stdio JSON-RPC for
diagnostics and language features (completion, hover, definition,
references, symbols). Nothing is downloaded — a server is spawned only
when its argv resolves to an executable.

```toml
[lsp]
enabled = true

[lsp.servers.python]
argv = ["pylsp"]
root_markers = ["pyproject.toml", "setup.py", ".git"]
```

See the example `config.toml` above for Python (`pylsp` /
`pyright-langserver`), YAML (`yaml-language-server --stdio`) and Rust
(`rust-analyzer`) setups, plus `[lsp.languages]` extension overrides.
Not every server implements every feature; missing capabilities degrade
to their absence, never to a crash.

**Trust model.** A server argv from the *global* config (`~/.config/fm-tui/config.toml`)
is trusted configuration. An argv found only in a **project-local** config is
untrusted: the first matching document opens a trust prompt, and approval is
bound to the exact `(workspace root, argv)` pair for this session only —
nothing is persisted. Durable grants are written by you, in the global
config, as `[[lsp.trust]] root = "…" argv = […]`; `[[lsp.trust]]` entries in
a project-local file are ignored (a checked-out repo cannot grant its own
execution). Session restore never carries execution trust. Headless runs
deny untrusted argv outright instead of prompting.

`lsp.status` shows the negotiated capability/encoding state; `lsp.restart`
restarts the current document's server. A missing executable or a server
that never completes `initialize` surfaces as a status note and otherwise
behaves like no server. The automated test matrix exercises fake peers —
including malformed and slow servers — over the real stdio transport; it
never downloads or runs a real language server.

## Private recovery

If fm exits while a document has unsaved changes (crash, `kill -9`,
terminal loss), the next launch offers a recovery prompt: restore the
snapshot into a dirty buffer (`r`), discard it (`d`), or dismiss (`Esc`/`n`)
— dismissing keeps the record. Restoring never writes the original file;
it produces an ordinary dirty buffer you can edit or save.

Snapshots are bounded and private:

- **Dirty documents only**, keyed by (workspace root, path, disk revision)
  so a record from a stale disk state can't overwrite a newer one.
- Written to a user-private state directory (`0700` directory, `0600`
  records, atomic rename); symlinked, foreign-owned or world-readable
  records are refused.
- Retention: `max_records` per workspace (default 32, clamped 1..=1024) and
  `max_age_secs` (default 7 days, minimum 1 hour); writes are throttled by
  `min_interval_ms` (default 2000).
- Commands: `recovery.restore` / `recovery.discard` / `recovery.clear` /
  `recovery.enable` / `recovery.disable` — or `[recovery] enabled = false`
  to turn snapshots off entirely.

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

# Run with clippy (the local gate also checks all targets)
cargo clippy -- -D warnings
cargo clippy --all-targets -- -D warnings

# Format check
cargo fmt --check

# Release build
cargo build --release

# Run in development
cargo run -- .
```

### Workspace acceptance harness

The terminal-workspace acceptance matrix runs against a **release** binary:

```bash
cargo build --release

# PTY scenarios — 13 automated runs (workflow, resize, TERM variants,
# nested tmux, feature flags, missing/fake LSP, external save, crash
# recovery, corrupt records, missing git, git markers). Exits nonzero on
# any failure or a missing harness/binary — never a silent skip.
python3 scripts/test-terminal-workspace.py

# Browser-terminal suite — xterm.js + ws fixture served locally, driven by
# Playwright: paste paths, browser-reserved shortcut exclusions, copy
# fallback, resize, full workflow. `npm ci` first if node_modules is absent.
npm --prefix tools/terminal-tests ci
npm --prefix tools/terminal-tests test

# All of the above plus Rust gates in one place, nonzero exit on any
# mandatory failure:
scripts/check-terminal-workspace.sh
```

Live SSH/Jupyter/Kubeflow deployment remains a manual boundary: controlled
PTY/tmux transport evidence is not a production-deployment claim.

## License

MIT License. See [LICENSE](LICENSE) for details.
