# Learnings: Embedded Terminal Panel

## Inherited Patterns
- Widget builder: `WidgetName::new(state, theme).block(block)`
- Handler: 3-level dispatch (global → panel → dialog)
- crossterm event polling is blocking; run in spawned tokio task
- Store layout `Rect` on App for mouse coordinate mapping

## New Learnings

### PTY Integration
- `portable-pty` handles cross-platform PTY creation cleanly; the `MasterPty` trait allows resize and writer/reader cloning
- PTY reader must use `spawn_blocking` (not `spawn`) because `Read` is blocking I/O
- Bridge pattern: PTY reader → `mpsc::unbounded_channel` → tokio task → `Event::TerminalOutput` in main event loop
- Shell process persists when terminal panel is hidden — no need to restart

### VTE Parser Architecture
- `vte::Parser::advance()` requires `&mut self` on both parser and performer — solved by separating `Performer` struct from `TerminalEmulator` to avoid borrow checker issues
- The `Performer` borrows fields from `TerminalEmulator` via mutable references, allowing the parser to drive the emulator without self-referential borrows
- VTE params come as `vte::Params` (nested slices) — flatten with `params.iter().flat_map(|sub| sub.iter().copied())`

### Input Routing Architecture
- Terminal input must be routed BEFORE general global keys (e.g., `q` should type 'q' in terminal, not quit the app)
- Reserved keys (Ctrl+T, Ctrl+↑/↓, Esc, Tab) are intercepted before PTY forwarding
- `key_event_to_bytes()` converts crossterm KeyEvents to VT100 byte sequences for the PTY

### Keybinding Conflicts
- Ctrl+T was previously used for "cycle view mode" in preview panel — reassigned to terminal toggle
- When adding global keybindings, must check for conflicts with all panel-specific bindings
- Terminal panel input routing happens at the `handle_normal_mode` level, before panel dispatch

### Layout Integration
- Terminal panel uses conditional 3-row vertical layout: `[main, terminal, status]`
- Dynamic resize via `height_percent` (clamped 10-80%) with Ctrl+↑/↓
- Emulator grid auto-resizes to match the terminal panel's inner area on each render
- PTY resize notification (`SIGWINCH`) sent alongside emulator resize

### Configuration
- Added `[terminal]` TOML section with `enabled`, `default_shell`, `scrollback_lines`
- `--no-terminal` CLI flag for disabling without config file changes
- Shell defaults to `$SHELL` env var, then `/bin/sh` fallback
