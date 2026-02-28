# Tech Stack

## Language
- **Rust** (edition 2021)

## Core Dependencies

| Crate | Version | Purpose |
|-------|---------|---------| 
| `ratatui` | 0.29 | TUI rendering framework (crossterm feature) |
| `crossterm` | 0.28 | Terminal backend — works in Jupyter/KubeFlow web terminals |
| `tokio` | 1 | Async runtime for fs watcher + event loop |
| `notify` | 7 | Cross-platform filesystem event watcher |
| `notify-debouncer-mini` | 0.5 | Debounced filesystem events |
| `clap` | 4 | CLI argument parsing (derive macros) |
| `syntect` | 5 | Syntax highlighting for file preview |
| `fuzzy-matcher` | 0.3 | Fuzzy string matching for file search |
| `thiserror` | 1 | Ergonomic error type derivation |
| `serde_json` | 1 | JSON parsing (Jupyter notebook .ipynb files) |
| `serde` | 1 | Config file deserialization (derive feature) |
| `toml` | 0.8 | TOML config file parsing |
| `dirs` | 5 | Platform-specific config directory resolution |

## Build & Distribution
- **Release profile**: `opt-level = "z"`, LTO, single codegen unit, stripped
- **Static binary**: `x86_64-unknown-linux-musl` target for container deployment
- **Binary size target**: < 10MB
- **CI/CD**: GitHub Actions (test, clippy, fmt, musl build, artifact upload)
- **Release automation**: GitHub Actions release workflow (4-target cross-compile)

## Architecture
- Single-binary monolith (no plugins, no IPC)
- Event-driven TUI loop (crossterm poll → handler dispatch → render)
- Lazy directory loading (on-demand tree expansion)
- Async file operations (tokio tasks for large copy/delete)
- Module structure: `main.rs`, `app.rs`, `event.rs`, `handler.rs`, `ui.rs`, `tui.rs`, `error.rs`, `config.rs`, `theme.rs`, `preview_content.rs`, `components/` (tree, preview, status_bar, dialog, search, help), `fs/` (tree, operations, watcher, clipboard)
