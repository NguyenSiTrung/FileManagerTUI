# Tech Stack

> Last refreshed: 2026-09-30 — packaging artifacts (.deb/AppImage), Windows target, dev-dependency

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
| `portable-pty` | 0.8 | Cross-platform pseudo-terminal (PTY) creation |
| `vte` | 0.13 | VT100/xterm escape sequence parser for terminal emulation |
| `base64` | 0.22 | Base64 encoding for OSC 52 clipboard escape sequences |

**Dev dependency**: `tempfile` 3 — temporary directories for filesystem tests.

## Build & Distribution
- **Binary**: `[[bin]] name = "fm"` (package `file_manager_tui`)
- **Release profile**: `opt-level = "z"`, LTO, single codegen unit, stripped
- **Static binary**: `x86_64-unknown-linux-musl` target for container deployment
- **Binary size target**: < 10MB
- **CI/CD**: GitHub Actions `ci.yml` on push/PR to `master`/`main` — `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`
- **Release automation**: GitHub Actions `release.yml` on `v*` tags — 4-target cross-compile (`x86_64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`), plus `.deb` package (cargo-deb via `[package.metadata.deb]`) and AppImage (appimagetool), aggregated into a GitHub Release with generated notes
- **Windows support**: `#[cfg(unix)]` / `#[cfg(not(unix))]` guards where platform-specific (permission formatting); S3 browse mode shells out to `which` + `sh -c` and therefore requires a POSIX shell with the AWS CLI

## Architecture
- Single-binary monolith (no plugins, no IPC)
- Event-driven TUI loop (crossterm poll → handler dispatch → render)
- Lazy directory loading (on-demand tree expansion with pagination)
- Async file operations (tokio tasks for large copy/delete, directory expansion, preview)
- Module structure: `main.rs`, `app.rs`, `event.rs`, `handler.rs`, `ui.rs`, `tui.rs`, `error.rs`, `config.rs`, `theme.rs`, `editor.rs`, `preview_content.rs`, `components/` (tree, preview, editor, status_bar, dialog, search, search_action, help, settings, terminal), `fs/` (mod, tree, operations, watcher, clipboard), `terminal/` (mod, pty, emulator), `s3/` (mod, types, parser, backend)
