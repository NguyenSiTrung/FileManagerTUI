# Tech Stack

> Last refreshed: 2026-10-05 — unicode-segmentation/unicode-width/libc deps, background/app_jobs.rs module path

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
| `unicode-segmentation` | 1.12 | Grapheme-cluster boundaries for text slicing and truncation (`text.rs`, workspace chrome) |
| `unicode-width` | 0.2 | Display-column widths for CJK/emoji-safe rendering and truncation |
| `libc` | 0.2 | Linux/macOS-only target dependency: no-replace atomic rename (`renameat2`/`renamex_np`) and xattr/owner/timestamp preservation for safe saves, nonblocking `poll`, signals, termios |

**Dev dependency**: `tempfile` 3 — temporary directories for filesystem tests.

## Build & Distribution
- **Binary**: `[[bin]] name = "fm"` (package `file_manager_tui`)
- **Release profile**: `opt-level = "z"`, LTO, single codegen unit, stripped
- **Static binary**: `x86_64-unknown-linux-musl` target for container deployment
- **Binary size target**: < 10MB
- **CI/CD**: GitHub Actions `ci.yml` — runs on release tags (`v*`) and manual dispatch only (no per-commit runs): `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`, plus a release-compatibility build matrix (Linux `x86_64-unknown-linux-musl`, macOS, Windows) that builds `--release` and reports binary size
- **Release automation**: GitHub Actions `release.yml` on `v*` tags — 4-target cross-compile (`x86_64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`), plus `.deb` package (cargo-deb via `[package.metadata.deb]`) and AppImage (appimagetool), aggregated into a GitHub Release with generated notes
- **Windows support**: `#[cfg(unix)]` / `#[cfg(not(unix))]` guards where platform-specific (permission formatting); S3 browse mode shells out to `which` + `sh -c` and therefore requires a POSIX shell with the AWS CLI

## Test Harness
- **Rust gates**: `cargo test`, `cargo clippy -- -D warnings`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `cargo build --release`; coverage evidence via `cargo llvm-cov` when installed
- **PTY acceptance matrix**: `scripts/test-terminal-workspace.py` — stdlib-only runner (pty.fork + TIOCSWINSZ + select) driving the release binary through 13 scenarios (workflow, resize, TERM variants, nested tmux, feature flags, missing/fake LSP, external save, crash recovery, corrupt records, missing git, git markers); ANSI-stripped needle matching for ratatui's diff-rendered output; deadline-bounded reads with assert-on-cleanup
- **Browser-terminal suite**: `tools/terminal-tests/` — node-pty + ws bridge + vendored `@xterm/xterm`, driven by Playwright (`npm --prefix tools/terminal-tests test`); covers paste paths, browser-reserved shortcut exclusions, copy fallback, resize and the full workflow; fake LSP peer `scripts/fake-lsp-server.py` exercises the stdio transport without downloading real servers
- **Quality runner**: `scripts/check-terminal-workspace.sh` — executes all gates, binary-size check and optional coverage/musl evidence; nonzero exit on mandatory failures; a missing harness is a blocked check, not a silent skip

## Architecture
- Single-binary monolith (no plugins, no IPC)
- Event-driven TUI loop (crossterm poll → handler dispatch → render)
- Lazy directory loading (on-demand tree expansion with pagination)
- Async file operations (tokio tasks for large copy/delete, directory expansion, preview)
- Module structure: `main.rs`, `app.rs`, `event.rs`, `handler.rs`, `ui.rs`, `tui.rs`, `error.rs`, `config.rs`, `theme.rs`, `editor.rs`, `keymap.rs`, `commands.rs`, `diagnostics.rs`, `git.rs`, `recovery.rs`, `session.rs`, `search.rs`, `text.rs`, `preview_content.rs`, `highlighting.rs`, `background.rs` + `background/app_jobs.rs`, `workspace/` (mod, documents, focus, layout), `components/` (tree, preview, editor, status_bar, dialog, search, content_search, search_action, help, settings, terminal, command_menu, language_features, diagnostics, document_tabs, workspace_chrome), `fs/` (mod, tree, operations, save, watcher, clipboard), `terminal/` (mod, pty, emulator), `s3/` (mod, types, parser, backend), `lsp/` (mod, config, client, transport, positions, features)
