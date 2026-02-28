# FileManagerTUI — Implementation Plan

> A terminal-based file manager TUI built with Rust and Ratatui, designed for
> environments like KubeFlow and Jupyter notebooks where folder tree interaction
> is limited.

---

## Table of Contents

- [1. Problem Statement](#1-problem-statement)
- [2. Architecture Overview](#2-architecture-overview)
- [3. Project Structure](#3-project-structure)
- [4. Dependencies](#4-dependencies)
- [5. Core Data Models](#5-core-data-models)
- [6. Component Details](#6-component-details)
- [7. File Preview Strategy](#7-file-preview-strategy)
- [8. Key Bindings](#8-key-bindings)
- [9. Configuration](#9-configuration)
- [10. KubeFlow / Jupyter Considerations](#10-kubeflow--jupyter-considerations)
- [11. Build & Distribution](#11-build--distribution)
- [12. Milestones](#12-milestones)

---

## 1. Problem Statement

In KubeFlow and Jupyter notebook environments, navigating and manipulating the
folder tree is painful:

- No visual tree view like VS Code's sidebar.
- Basic `ls` / `cd` commands are the only option.
- No quick preview of file contents.
- No inline create / rename / delete / copy / move operations.
- Jupyter's built-in file browser is slow and limited.

**Goal**: A single static binary that provides a VS Code-like file explorer
experience in any terminal, including web-based terminals in KubeFlow pods.

---

## 2. Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                        main.rs                                  │
│              CLI args (clap) + Terminal init/restore             │
└──────────────────────────┬──────────────────────────────────────┘
                           │
┌──────────────────────────▼──────────────────────────────────────┐
│                        app.rs                                   │
│         App state machine: Normal | Search | Dialog | Command   │
│         Holds: TreeState, PreviewState, ClipboardState          │
└─────┬──────────────┬────────────────┬───────────────────────────┘
      │              │                │
      ▼              ▼                ▼
┌──────────┐  ┌────────────┐  ┌──────────────┐
│ event.rs │  │ handler.rs │  │   ui.rs      │
│ Event    │  │ Key/Mouse  │  │ Layout       │
│ loop     │→ │ dispatch   │→ │ composer     │
│ (poll)   │  │ per mode   │  │ (splits)     │
└──────────┘  └────────────┘  └──────┬───────┘
                                     │
              ┌──────────────────────┼──────────────────────┐
              │                      │                      │
              ▼                      ▼                      ▼
      ┌──────────────┐      ┌──────────────┐      ┌──────────────┐
      │ tree.rs      │      │ preview.rs   │      │ status_bar.rs│
      │ Folder tree  │      │ File preview │      │ Path + info  │
      │ panel widget │      │ panel widget │      │ + hints      │
      └──────────────┘      └──────────────┘      └──────────────┘
              │
              ▼
      ┌──────────────┐      ┌──────────────┐
      │ dialog.rs    │      │ search.rs    │
      │ Modal popups │      │ Fuzzy finder │
      │ (CRUD)       │      │ overlay      │
      └──────────────┘      └──────────────┘

      ┌───────────────────────────────────────────────────────────┐
      │                    fs/ (Filesystem Layer)                 │
      │  tree.rs       — TreeNode struct, lazy dir loading        │
      │  operations.rs — create / rename / delete / copy / move   │
      │  watcher.rs    — notify-based file system watcher         │
      │  clipboard.rs  — internal copy/cut/paste buffer           │
      └───────────────────────────────────────────────────────────┘
```

---

## 3. Project Structure

```
file-manager-tui/
├── Cargo.toml
├── config/                         # [Planned: M7] Default config shipped with binary
│   └── default.toml
├── src/
│   ├── main.rs                 # Entry point: CLI parsing, terminal setup/teardown
│   ├── app.rs                  # App state machine, central state holder
│   ├── event.rs                # Event loop: crossterm poll + fs watcher channel
│   ├── handler.rs              # Maps key/mouse events → actions per app mode
│   ├── ui.rs                   # Master layout: splits terminal into panels
│   ├── tui.rs                  # Terminal init/restore helpers (crossterm setup)
│   ├── error.rs                # AppError type via thiserror
│   ├── preview_content.rs      # Preview content loading: syntax highlighting,
│   │                           #   large file head+tail, binary/dir/notebook detection
│   ├── config.rs               # [Planned: M7] Config loading: CLI args → file → defaults
│   ├── components/
│   │   ├── mod.rs              # Re-exports
│   │   ├── tree.rs             # Folder tree StatefulWidget
│   │   ├── preview.rs          # File preview StatefulWidget
│   │   ├── status_bar.rs       # Bottom bar widget
│   │   ├── dialog.rs           # Modal dialog widget (input, confirm, progress)
│   │   └── search.rs           # Fuzzy search overlay widget
│   └── fs/
│       ├── mod.rs              # Re-exports
│       ├── tree.rs             # TreeNode data structure + operations
│       ├── operations.rs       # Filesystem CRUD with error handling
│       ├── watcher.rs          # File system watcher (notify crate)
│       └── clipboard.rs        # Copy/cut buffer management
└── # Tests are inline modules (#[cfg(test)] mod tests) within each source file
```

---

## 4. Dependencies

| Crate            | Version  | Purpose                                             |
| ---------------- | -------- | --------------------------------------------------- |
| `ratatui`        | `0.29`   | TUI rendering framework (crossterm feature)          |
| `crossterm`      | `0.28`   | Terminal backend (works in Jupyter/KubeFlow terms)   |
| `tokio`          | `1`      | Async runtime for fs watcher + event loop            |
| `notify`         | `7`      | Cross-platform filesystem event watcher              |
| `notify-debouncer-mini` | `0.5` | Debounced filesystem events                     |
| `clap`           | `4`      | CLI argument parsing with derive macros              |
| `syntect`        | `5`      | Syntax highlighting for file preview                 |
| `fuzzy-matcher`  | `0.3`    | Fuzzy string matching for file search                |
| `thiserror`      | `1`      | Ergonomic error type derivation                      |
| `serde_json`     | `1`      | JSON parsing (Jupyter notebook .ipynb files)          |
| `serde`          | `1`      | Config file deserialization (derive feature)          |
| `toml`           | `0.8`    | TOML config file parsing                             |
| `dirs`           | `5`      | Platform-specific config directory resolution        |

### Cargo.toml

```toml
[package]
name = "file_manager_tui"
version = "0.1.0"
edition = "2021"
description = "A terminal-based file manager TUI built with Rust and Ratatui"

[dependencies]
ratatui = { version = "0.29", features = ["crossterm"] }
crossterm = "0.28"
tokio = { version = "1", features = ["full"] }
notify = "7"
notify-debouncer-mini = "0.5"
clap = { version = "4", features = ["derive"] }
syntect = "5"
fuzzy-matcher = "0.3"
thiserror = "1"
serde_json = "1"
serde = { version = "1", features = ["derive"] }
toml = "0.8"
dirs = "5"

[dev-dependencies]
tempfile = "3"

[profile.release]
opt-level = "z"     # Optimize for binary size
lto = true
codegen-units = 1
strip = true
```

---

## 5. Core Data Models

### 5.1. App State (`app.rs`)

```rust
pub enum AppMode {
    Normal,                     // Default tree navigation
    Search,                     // Fuzzy finder overlay active
    Dialog(DialogKind),         // Modal dialog open
    Command,                    // Command palette (future)
}

pub enum DialogKind {
    CreateFile,
    CreateDirectory,
    Rename { original: PathBuf },
    DeleteConfirm { targets: Vec<PathBuf> },
    Error { message: String },
}

pub struct App {
    pub mode: AppMode,
    pub tree_state: TreeState,
    pub preview_state: PreviewState,
    pub clipboard: ClipboardState,
    pub config: AppConfig,
    pub should_quit: bool,
    pub status_message: Option<(String, Instant)>,  // Auto-dismiss after N seconds
}
```

### 5.2. Tree Node (`fs/tree.rs`)

```rust
pub struct TreeNode {
    pub name: String,
    pub path: PathBuf,
    pub node_type: NodeType,
    pub children: Option<Vec<TreeNode>>,   // None = not loaded yet (lazy)
    pub is_expanded: bool,
    pub depth: u16,
    pub metadata: FileMeta,
}

pub enum NodeType {
    File,
    Directory,
    Symlink { target: PathBuf },
}

pub struct FileMeta {
    pub size: u64,
    pub modified: SystemTime,
    pub permissions: u32,           // Unix mode bits
    pub is_hidden: bool,            // Starts with '.'
}

pub struct TreeState {
    pub root: TreeNode,
    pub flat_items: Vec<FlatItem>,   // Flattened for rendering
    pub selected_index: usize,
    pub scroll_offset: usize,
    pub multi_selected: HashSet<usize>,
    pub show_hidden: bool,
}

/// Flattened representation of a tree node for rendering in a list.
pub struct FlatItem {
    pub depth: u16,
    pub name: String,
    pub path: PathBuf,
    pub node_type: NodeType,
    pub is_expanded: bool,
    pub is_last_sibling: bool,      // For drawing └── vs ├──
}
```

### 5.3. Clipboard (`fs/clipboard.rs`)

```rust
pub enum ClipboardAction {
    Copy,
    Cut,
}

pub struct ClipboardState {
    pub action: Option<ClipboardAction>,
    pub items: Vec<PathBuf>,
}
```

### 5.4. Preview State (`components/preview.rs`)

```rust
pub enum PreviewContent {
    Text {
        lines: Vec<String>,
        language: String,
        is_truncated: bool,
        total_lines: usize,
    },
    LargeFile {
        head_lines: Vec<String>,
        tail_lines: Vec<String>,
        language: String,
        total_lines: usize,
        file_size: u64,
    },
    NotebookCells {
        cells: Vec<NotebookCell>,
    },
    DirectorySummary {
        num_files: usize,
        num_dirs: usize,
        total_size: u64,
    },
    Binary {
        file_type: String,
        size: u64,
        modified: SystemTime,
    },
    Empty,
}

pub struct NotebookCell {
    pub cell_type: String,       // "code", "markdown", "raw"
    pub source: String,
    pub index: usize,
}

pub struct PreviewState {
    pub content: PreviewContent,
    pub scroll_offset: usize,
    pub view_mode: PreviewViewMode,
}

pub enum PreviewViewMode {
    HeadAndTail,
    HeadOnly,
    TailOnly,
}
```

---

## 6. Component Details

### 6.1. Tree Panel (`components/tree.rs`)

**Rendering rules:**

```
 ┌─ /home/jovyan/project ──────────────────┐
 │  📁 data/                                │
 │  │  ├── 📁 raw/                          │
 │  │  │  ├── 📄 train.csv                  │
 │  │  │  └── 📄 test.csv                   │
 │  │  └── 📁 processed/                    │
 │  ├──  config.yaml                       │
 │  ├──  train.py               ← selected │
 │  ├──  model.py                          │
 │  ├── 📓 experiment.ipynb                 │
 │  ├──  requirements.txt                  │
 │  └── 📄 README.md                        │
 └──────────────────────────────────────────┘
```

**Icon mapping (Nerd Font):**

| Extension / Type    | Icon |
| ------------------- | ---- |
| Directory (closed)  | `` |
| Directory (open)    | `` |
| `.py`               | `` |
| `.ipynb`            | `📓` |
| `.rs`               | `` |
| `.yaml` / `.yml`    | `` |
| `.json`             | `` |
| `.toml`             | `` |
| `.md`               | `` |
| `.txt` / `.log`     | `` |
| `.csv`              | `` |
| `.sh` / `.bash`     | `` |
| `.sql`              | `` |
| `.html`             | `` |
| `.css`              | `` |
| `.js` / `.ts`       | `` |
| `.docker` / `Dockerfile` | `` |
| `.git*`             | `` |
| `.pkl` / `.pt` / `.h5` | `🧠` |
| Other / unknown     | `` |

**Behavior:**

- `TreeState.flat_items` is rebuilt whenever the tree structure changes
  (expand/collapse/refresh).
- Sorting: directories first, then files; alphabetical within each group.
- Hidden files (dotfiles) toggled with `.` key.
- Multi-select: press `Space` to toggle, highlighted with a distinct background.
- Lazy loading: children loaded on first expand, cached afterward.

### 6.2. Preview Panel (`components/preview.rs`)

See [Section 7](#7-file-preview-strategy) for full details.

### 6.3. Status Bar (`components/status_bar.rs`)

```
 ┌──────────────────────────────────────────────────────────────────┐
 │ /home/jovyan/project/train.py │ 4.2 KB │ Python │ 644 │ ?:help │
 └──────────────────────────────────────────────────────────────────┘
```

Three sections:
- **Left**: Full path of selected item.
- **Center**: File size | detected language | permissions.
- **Right**: Contextual hints (`?:help  a:new  r:rename  d:delete`).

### 6.4. Dialog System (`components/dialog.rs`)

Centered modal overlay using Ratatui's `Clear` widget + `Block`:

```
 ┌─ Create New File ──────────────────────┐
 │                                        │
 │   Name: my_notebook.ipynb█             │
 │                                        │
 │   [Enter] Confirm    [Esc] Cancel      │
 └────────────────────────────────────────┘
```

```
 ┌─ Confirm Delete ───────────────────────┐
 │                                        │
 │   Delete 3 selected items?             │
 │                                        │
 │     • raw/train.csv                    │
 │     • raw/test.csv                     │
 │     • old_model.pt                     │
 │                                        │
 │   [y] Yes, delete    [n/Esc] Cancel    │
 └────────────────────────────────────────┘
```

**State flow:**

```
Normal ──(press 'a')──→ Dialog(CreateFile)
  ↑                           │
  └───(Enter or Esc)──────────┘
```

### 6.5. Fuzzy Search (`components/search.rs`)

Full-screen overlay similar to VS Code's `Ctrl+P`:

```
 ┌─ Find File ────────────────────────────┐
 │  > train█                              │
 │ ─────────────────────────────────────  │
 │   train.py                    src/     │
 │   train.csv                   data/    │
 │   training_log.txt            logs/    │
 │   pretrained_model.pt         models/  │
 └────────────────────────────────────────┘
```

- Uses `fuzzy-matcher::skim::SkimMatcherV2` for scoring.
- Walks the tree to collect all file paths (cached, refreshed on fs events).
- Matched characters highlighted in a distinct color.
- `Enter` navigates to the selected file in the tree.

---

## 7. File Preview Strategy

### 7.1. Detection Flow

```
Is the file a directory?
  → Yes: Show DirectorySummary
  → No: Read file metadata
        ↓
  Is it a known binary extension? (.pkl, .pt, .h5, .png, .jpg, .zip, .tar, .gz)
    → Yes: Show Binary metadata
    → No: Try reading first 8KB
          ↓
    Contains null bytes?
      → Yes: Show Binary metadata
      → No: It's a text file
            ↓
    Is size ≤ max_full_preview_bytes? (default: 1MB)
      → Yes: Load full content → Text preview with syntax highlighting
      → No:  Load head N lines + tail M lines → LargeFile preview
            ↓
    Is extension .ipynb?
      → Yes: Parse JSON → NotebookCells preview
      → No:  Regular text preview
```

### 7.2. Syntax Detection

`syntect` provides automatic detection by extension. For the fast path:

```rust
fn detect_language(path: &Path) -> &str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("py")                   => "Python",
        Some("rs")                   => "Rust",
        Some("yaml" | "yml")         => "YAML",
        Some("json")                 => "JSON",
        Some("toml")                 => "TOML",
        Some("sh" | "bash" | "zsh")  => "Bash",
        Some("sql")                  => "SQL",
        Some("md" | "markdown")      => "Markdown",
        Some("html" | "htm")         => "HTML",
        Some("css")                  => "CSS",
        Some("js" | "jsx")           => "JavaScript",
        Some("ts" | "tsx")           => "TypeScript",
        Some("c" | "h")             => "C",
        Some("cpp" | "hpp" | "cc")   => "C++",
        Some("java")                => "Java",
        Some("go")                   => "Go",
        Some("rb")                   => "Ruby",
        Some("txt" | "log" | "csv"
           | "th" | "vi" | "text"
           | "cfg" | "conf" | "ini") => "Plain Text",
        Some("ipynb")                => "Python",   // After JSON parse
        None                         => detect_from_shebang(path),
        _                            => "Plain Text",
    }
}

fn detect_from_shebang(path: &Path) -> &str {
    // Read first line, check for #!/usr/bin/env python, #!/bin/bash, etc.
}
```

### 7.3. Large File Preview Rendering

For files exceeding `max_full_preview_bytes`:

```
 ┌─ Preview: training_log.txt (142 MB) ────────────────────┐
 │   1 │ Epoch 1/100, loss: 2.4523, acc: 0.1200            │
 │   2 │ Epoch 2/100, loss: 2.1034, acc: 0.1813            │
 │   3 │ Epoch 3/100, loss: 1.8721, acc: 0.2504            │
 │ ... │ ...                                                │
 │  50 │ Epoch 50/100, loss: 0.3312, acc: 0.8921           │
 │     │                                                    │
 │     │  ──── 1,204,482 lines omitted ────                 │
 │     │                                                    │
 │  N-19 │ Epoch 99/100, loss: 0.0021, acc: 0.9981         │
 │  N-18 │ Epoch 100/100, loss: 0.0019, acc: 0.9993        │
 │  ...  │ ...                                              │
 │  N    │ Training complete. Model saved to checkpoint.pt  │
 │                                                          │
 │  [Ctrl+T] Toggle view   [+/-] Adjust lines              │
 └──────────────────────────────────────────────────────────┘
```

**Line counting for large files:**

- Do NOT read the entire file to count lines.
- Use a fast byte-scanning approach: count `\n` bytes by reading in 64KB
  chunks. This can scan ~1GB/s.
- Cache the line count per file path (invalidate on fs watcher event).

### 7.4. Jupyter Notebook Preview

`.ipynb` files are JSON. Parse and render cells:

```
 ┌─ Preview: experiment.ipynb (Notebook: 12 cells) ────────┐
 │                                                          │
 │  ── [Cell 1] markdown ──────────────────────────────     │
 │  # Experiment: ResNet Fine-tuning                        │
 │  Training on custom dataset with transfer learning       │
 │                                                          │
 │  ── [Cell 2] code ──────────────────────────────────     │
 │  import torch                                            │
 │  import torch.nn as nn                                   │
 │  from torchvision import models                          │
 │                                                          │
 │  ── [Cell 3] code ──────────────────────────────────     │
 │  model = models.resnet50(pretrained=True)                │
 │  model.fc = nn.Linear(2048, 10)                          │
 │                                                          │
 └──────────────────────────────────────────────────────────┘
```

---

## 8. Key Bindings

### 8.1. Normal Mode (Tree Navigation)

| Key                        | Action                                   |
| -------------------------- | ---------------------------------------- |
| `j` / `↓`                 | Move selection down                      |
| `k` / `↑`                 | Move selection up                        |
| `Enter` / `l` / `→`       | Expand directory / open file in `$EDITOR`|
| `h` / `←` / `Backspace`   | Collapse directory / go to parent        |
| `Space`                    | Toggle multi-select on current item      |
| `g` / `Home`              | Jump to first item                       |
| `G` / `End`               | Jump to last item                        |
| `Ctrl+D` / `PageDown`     | Scroll down half page                    |
| `Ctrl+U` / `PageUp`       | Scroll up half page                      |
| `.`                        | Toggle hidden files visibility           |
| `Tab`                      | Switch focus: tree ↔ preview             |
| `q` / `Ctrl+C`            | Quit application                         |
| `?`                        | Show help overlay                        |

### 8.2. File Operations

| Key          | Action                                        |
| ------------ | --------------------------------------------- |
| `a`          | Create new file (opens input dialog)          |
| `A`          | Create new directory (opens input dialog)     |
| `r`          | Rename selected item (opens input dialog)     |
| `d`          | Delete selected / multi-selected (confirm)    |
| `y`          | Copy selected items to clipboard buffer       |
| `x`          | Cut selected items to clipboard buffer        |
| `p`          | Paste clipboard buffer into current directory |
| `Ctrl+Z`     | Undo last operation (single level)            |

### 8.3. Search & Filter

| Key          | Action                                        |
| ------------ | --------------------------------------------- |
| `Ctrl+P`     | Open fuzzy file finder                        |
| `/`          | Filter tree view (type to filter)             |
| `Esc`        | Close search / filter / dialog                |

### 8.4. Preview Mode (when preview panel is focused)

| Key          | Action                                        |
| ------------ | --------------------------------------------- |
| `j` / `↓`   | Scroll preview down                           |
| `k` / `↑`   | Scroll preview up                             |
| `g`          | Jump to top of preview                        |
| `G`          | Jump to bottom of preview                     |
| `Ctrl+T`     | Toggle view mode (head+tail / head / tail)    |
| `+`          | Increase head/tail line count by 10           |
| `-`          | Decrease head/tail line count by 10           |
| `Ctrl+W`     | Toggle line wrap                              |
| `Tab`        | Switch focus back to tree                     |

---

## 9. Configuration

### 9.1. Config File Location

Resolution order (first found wins):
1. CLI flag: `--config <path>`
2. `$FM_TUI_CONFIG` environment variable
3. `~/.config/fm-tui/config.toml`
4. Built-in defaults

### 9.2. Config Schema

```toml
# ~/.config/fm-tui/config.toml

[general]
# Starting directory (overridden by CLI --path argument)
default_path = "."
# Show hidden files by default
show_hidden = false
# Confirm before delete
confirm_delete = true

[preview]
# Maximum file size (bytes) for full preview. Above this, use head+tail mode.
max_full_preview_bytes = 1_048_576    # 1 MB
# Number of lines to show from the top of large files
head_lines = 50
# Number of lines to show from the bottom of large files
tail_lines = 20
# Default view mode for large files: "head_and_tail", "head_only", "tail_only"
default_view_mode = "head_and_tail"
# Tab rendering width
tab_width = 4
# Enable line wrapping
line_wrap = false
# Syntax highlighting theme (syntect theme name)
syntax_theme = "base16-ocean.dark"

[tree]
# Sort order: "name", "size", "modified"
sort_by = "name"
# Directories always listed first
dirs_first = true
# Use nerd font icons (set to false for basic ASCII)
use_icons = true

[watcher]
# Enable filesystem watcher for auto-refresh
enabled = true
# Debounce interval in milliseconds
debounce_ms = 300

[theme]
# Color scheme: "dark", "light", "custom"
scheme = "dark"

[theme.dark]
tree_bg = "#1e1e2e"
tree_fg = "#cdd6f4"
tree_selected_bg = "#45475a"
tree_selected_fg = "#cdd6f4"
tree_dir_fg = "#89b4fa"
tree_file_fg = "#cdd6f4"
tree_hidden_fg = "#6c7086"
preview_bg = "#1e1e2e"
preview_fg = "#cdd6f4"
preview_line_nr_fg = "#6c7086"
status_bg = "#313244"
status_fg = "#cdd6f4"
border_fg = "#585b70"
dialog_bg = "#313244"
dialog_border_fg = "#89b4fa"
```

### 9.3. CLI Arguments

```
USAGE:
    fm [OPTIONS] [PATH]

ARGS:
    <PATH>    Starting directory [default: .]

OPTIONS:
    -c, --config <FILE>       Path to config file
        --no-preview          Disable preview panel
        --no-watcher          Disable filesystem watcher
        --no-icons            Use ASCII instead of Nerd Font icons
        --head-lines <N>      Lines from top for large file preview [default: 50]
        --tail-lines <N>      Lines from bottom for large file preview [default: 20]
        --max-preview <BYTES> Max file size for full preview [default: 1048576]
        --theme <THEME>       Color theme: dark, light [default: dark]
    -h, --help                Print help
    -V, --version             Print version
```

---

## 10. KubeFlow / Jupyter Considerations

### 10.1. Terminal Compatibility

- **Backend**: Use `crossterm` exclusively — it works in:
  - Standard Linux terminals (xterm, gnome-terminal, alacritty)
  - tmux / screen sessions
  - Jupyter's built-in terminal
  - KubeFlow's web-based terminal
  - VS Code integrated terminal
- **No mouse dependency**: Full keyboard navigation. Mouse support is opt-in
  but not required (Jupyter terminals have unreliable mouse events).
- **Unicode handling**: Use `unicode-width` for correct rendering of CJK
  characters and emoji in filenames.

### 10.2. Performance for ML Workloads

- **Lazy directory loading**: Critical for directories with thousands of
  checkpoint files (`model_epoch_001.pt` through `model_epoch_500.pt`).
- **Async file operations**: Delete/copy of large model files won't freeze the
  UI.
- **Preview size limit**: Prevents loading multi-GB CSV files into memory.
- **Line counting via byte scan**: Fast even for 100MB+ log files.

### 10.3. ML-Specific File Awareness

- `.ipynb` — Jupyter notebook preview with cell rendering
- `.pt` / `.pth` — PyTorch model files (show metadata)
- `.h5` / `.hdf5` — HDF5 / Keras model files (show metadata)
- `.pkl` / `.pickle` — Python pickle files (show metadata, warn about security)
- `.onnx` — ONNX model files (show metadata)
- `.csv` / `.tsv` — Show row/column count in preview header
- `.parquet` — Show schema info if possible
- `.yaml` / `.yml` — Common for KubeFlow pipeline configs

### 10.4. Container Deployment

```bash
# Build a fully static binary (no glibc dependency)
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl

# Binary size target: < 10MB
ls -lh target/x86_64-unknown-linux-musl/release/fm

# Copy into a running KubeFlow pod
kubectl cp target/x86_64-unknown-linux-musl/release/fm \
  namespace/pod-name:/usr/local/bin/fm

# Or add to Dockerfile
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/fm /usr/local/bin/fm
```

---

## 11. Build & Distribution

### 11.1. Development

```bash
# Run in development
cargo run -- /path/to/explore

# Run with logging
RUST_LOG=debug cargo run -- /path/to/explore

# Run tests
cargo test

# Check formatting and lints
cargo fmt --check
cargo clippy -- -D warnings
```

### 11.2. Release Build

```bash
# Optimized release build
cargo build --release

# Fully static build for containers
cargo build --release --target x86_64-unknown-linux-musl
```

### 11.3. CI/CD (GitHub Actions)

```yaml
# .github/workflows/ci.yml
name: CI
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test
      - run: cargo clippy -- -D warnings
      - run: cargo fmt --check

  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: x86_64-unknown-linux-musl
      - run: cargo build --release --target x86_64-unknown-linux-musl
      - uses: actions/upload-artifact@v4
        with:
          name: fm-linux-amd64
          path: target/x86_64-unknown-linux-musl/release/fm
```

---

## 12. Milestones

### Milestone 1: Core Loop + Tree Rendering ✅
**Goal**: Navigate a folder tree in the terminal.

- [x] Project setup: `Cargo.toml`, dependencies, directory structure
- [x] `main.rs`: Terminal init/restore with crossterm
- [x] `event.rs`: Basic event loop (poll crossterm key events)
- [x] `fs/tree.rs`: `TreeNode` struct with lazy directory loading
- [x] `components/tree.rs`: Render tree as `StatefulWidget` with indentation
- [x] `handler.rs`: Navigation keys (j/k, Enter/Backspace, expand/collapse)
- [x] `ui.rs`: Single-panel layout (tree only)
- [x] `app.rs`: Basic app state (selected index, scroll offset)

**Deliverable**: Can launch `fm /path`, see folder tree, navigate with keyboard.

---

### Milestone 2: File Operations + Dialogs ✅
**Goal**: Create, rename, delete files and directories.

- [x] `components/dialog.rs`: Modal input dialog widget
- [x] `components/dialog.rs`: Confirmation dialog widget
- [x] `fs/operations.rs`: `create_file`, `create_dir`, `rename`, `delete`
- [x] `handler.rs`: Wire up `a`, `A`, `r`, `d` keys to dialogs
- [x] `app.rs`: `AppMode::Dialog` state transitions
- [x] `components/status_bar.rs`: Show success/error messages
- [x] Error handling: Display fs errors in dialog

**Deliverable**: Can create, rename, delete files/dirs with confirmation.

---

### Milestone 3: Preview Panel + Syntax Highlighting ✅
**Goal**: See file contents alongside the tree.

- [x] `ui.rs`: Split layout — tree (left 40%) + preview (right 60%)
- [x] `components/preview.rs`: Full text preview with `syntect` highlighting
- [x] Preview: Large file head+tail mode with configurable line counts
- [x] Preview: Binary file metadata display
- [x] Preview: Directory summary (file count, total size)
- [x] Preview: `.ipynb` notebook cell rendering
- [x] `handler.rs`: `Tab` to switch focus, scroll keys in preview
- [x] Syntax detection by extension + shebang fallback

**Deliverable**: Selecting a file shows syntax-highlighted preview; large files
show head+tail.

---

### Milestone 4: Copy / Cut / Paste + Multi-Select ✅
**Goal**: Move and copy files within the tree.

- [x] `fs/clipboard.rs`: Clipboard buffer (copy/cut state + paths)
- [x] `fs/operations.rs`: `copy_recursive`, `move_item`
- [x] `handler.rs`: `Space` for multi-select, `y`/`x`/`p` for clipboard ops
- [x] `components/tree.rs`: Visual indicator for multi-selected items
- [x] `components/status_bar.rs`: Show clipboard state ("3 items copied")
- [x] Async file operations for large files (don't freeze UI)
- [x] Basic undo: single-level undo for last operation

**Deliverable**: Can copy/cut files and paste them into another directory.

---

### Milestone 5: Fuzzy Finder + Search ✅
**Goal**: Quickly find files by name.

- [x] `components/search.rs`: Fuzzy finder overlay widget (centered modal)
- [x] File path indexing: walk tree, cache paths, invalidate cache on mutations
- [x] `fuzzy-matcher` integration: score + rank results
- [x] Highlight matched characters in results
- [x] `Enter` to navigate tree to selected result
- [x] `/` key for inline tree filtering (filter-as-you-type)
- [x] `handler.rs`: `Ctrl+P` to open, `Esc` to close

**Deliverable**: Press `Ctrl+P`, type partial filename, navigate to match.

---

### Milestone 6: File Watcher + Auto-Refresh ✅
**Goal**: Tree updates automatically when files change externally.

- [x] `fs/watcher.rs`: `notify` crate watcher with debouncing
- [x] Event channel: watcher → main event loop
- [x] `app.rs`: Handle `FsEvent` — refresh affected subtree
- [x] Preserve selection and scroll position on refresh
- [x] `config.rs`: Watcher enable/disable + debounce config

**Deliverable**: Creating a file via `touch` in another terminal appears
automatically in the tree.

---

### Milestone 7: Configuration + Polish ✅
**Goal**: Production-ready with customization.

- [x] `config.rs`: Load from TOML file + CLI args + defaults
- [x] CLI argument parsing with `clap` (full: `--config`, `--no-preview`, `--no-watcher`, `--no-icons`, `--no-mouse`, `--head-lines`, `--tail-lines`, `--max-preview`, `--theme`)
- [x] Theme support: dark / light / custom colors (`theme.rs` + `ThemeColors` + config integration)
- [x] Help overlay (`?` key): show all keybindings (`components/help.rs`)
- [x] Mouse support (optional): click to select, scroll wheel, focus switching
- [x] Nerd Font icon toggle (fallback to ASCII via `--no-icons`)
- [x] Sort options: by name, size, modified date (`SortBy` enum + `dirs_first`)
- [x] Error recovery: graceful handling of permission denied, broken symlinks
- [x] README.md with install instructions and screenshots
- [x] Release binary via GitHub Actions (CI + release workflows)

**Deliverable**: Configurable, polished, ready for daily use in KubeFlow.

---

## Appendix: Reference Projects

| Project | URL | Notes |
|---------|-----|-------|
| `ratatui-explorer` | https://github.com/tatounee/ratatui-explorer | File explorer widget for ratatui |
| `yazi` | https://github.com/sxyazi/yazi | Async terminal file manager in Rust |
| `broot` | https://github.com/Canop/broot | Tree-focused file manager |
| `lf` | https://github.com/gokcehan/lf | Terminal file manager (Go) |
| `ranger` | https://github.com/ranger/ranger | Classic Python TUI file manager |
| `ratatui` | https://github.com/ratatui-org/ratatui | TUI framework documentation |
