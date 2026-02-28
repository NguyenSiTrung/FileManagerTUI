# Plan: Embedded Terminal Panel

## Phase 1: PTY Infrastructure & Terminal Module

- [x] Task 1: Add dependencies and create terminal module structure
  - [x] Add `portable-pty` crate to Cargo.toml (cross-platform PTY)
  - [x] Add `vte` crate to Cargo.toml (ANSI escape sequence parser from alacritty)
  - [x] Create `src/terminal/mod.rs` with module declarations
  - [x] Create `src/terminal/pty.rs` (PTY process management)
  - [x] Register `mod terminal` in `main.rs`

- [x] Task 2: Implement PtyProcess — shell spawning and async I/O
  - [x] Implement `PtyProcess::spawn(shell, cwd, size)` using `portable-pty`
  - [x] Implement async PTY output reader (tokio task → mpsc channel)
  - [x] Implement `write(&[u8])` to send input to PTY stdin
  - [x] Implement `resize(rows, cols)` to send SIGWINCH
  - [x] Implement `is_alive()` and `shutdown()` for process lifecycle
  - [x] Write unit tests for spawn, write, and shutdown

- [x] Task 3: Add TerminalOutput event to the event system
  - [x] Add `Event::TerminalOutput(Vec<u8>)` variant to `event.rs`
  - [x] Wire PTY reader task to send `TerminalOutput` events via the existing `event_tx` channel
  - [x] Handle `Event::TerminalOutput` in main loop (forward to terminal emulator)

- [ ] Task 4: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

## Phase 2: Terminal Emulation (Screen Buffer)

- [x] Task 1: Create terminal emulator with VTE parser
  - [x] Create `src/terminal/emulator.rs`
  - [x] Define `Cell` struct (character, fg color, bg color, attributes)
  - [x] Define `TerminalEmulator` struct (grid: Vec<Vec<Cell>>, cursor, size, scrollback)
  - [x] Implement `vte::Perform` trait for `TerminalEmulator` — handle:
    - [x] `print()` — regular characters
    - [x] `execute()` — control characters (CR, LF, BS, TAB, BEL)
    - [x] `csi_dispatch()` — cursor movement, erase, SGR (colors/styles)
    - [x] `osc_dispatch()` — terminal title (store but ignore for now)
    - [x] `esc_dispatch()` — escape sequences
  - [x] Implement scrollback buffer (ring buffer, configurable size)
  - [x] Write tests for basic escape sequence handling (cursor move, colors, erase)

- [x] Task 2: Implement screen buffer to ratatui conversion
  - [x] Implement `TerminalEmulator::render_lines() -> Vec<Line<'static>>`
  - [x] Map Cell fg/bg colors to ratatui `Style` (ANSI 16, 256, and RGB)
  - [x] Map Cell attributes (bold, italic, underline, reverse) to ratatui `Modifier`
  - [x] Write tests for color/style mapping

- [ ] Task 3: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)

## Phase 3: App State & UI Layout Integration

- [x] Task 1: Add terminal state to App
  - [x] Create `TerminalState` struct in `src/terminal/mod.rs`:
    - `emulator: TerminalEmulator`
    - `pty: Option<PtyProcess>`
    - `visible: bool`
    - `height_percent: u16` (default 30)
    - `scroll_offset: usize` (scrollback position)
  - [x] Add `terminal_state: TerminalState` to `App` struct
  - [x] Add `Terminal` variant to `FocusedPanel` enum
  - [x] Add `terminal_area: Rect` to App for mouse mapping
  - [x] Implement `App::toggle_terminal()` — spawn PTY on first open, toggle visibility
  - [x] Implement `App::resize_terminal_up/down()` — adjust `height_percent` (min 10%, max 80%)

- [x] Task 2: Modify UI layout for bottom terminal panel
  - [x] In `ui.rs`, add conditional vertical split: `[main_area, terminal_area]` when visible
  - [x] When terminal hidden, use existing layout unchanged
  - [x] When terminal visible: `[Min(3), Length(term_height), Length(1)]` for tree+preview / terminal / status bar
  - [x] Store `terminal_area` on App for mouse click mapping

- [x] Task 3: Create TerminalWidget component
  - [x] Create `src/components/terminal.rs`
  - [x] Implement `TerminalWidget` following existing widget pattern (new + block builder)
  - [x] Render emulator's `render_lines()` output with scrollback support
  - [x] Show cursor position when terminal is focused
  - [x] Register in `src/components/mod.rs`
  - [x] Write tests for widget rendering

- [ ] Task 4: Conductor - User Manual Verification 'Phase 3' (Protocol in workflow.md)

## Phase 4: Input Routing & Focus Management

- [x] Task 1: Update focus cycling logic
  - [x] Modify `App::toggle_focus()` to cycle: Tree → Preview → Terminal (when visible) → Tree
  - [x] When terminal is hidden, cycle as before: Tree → Preview → Tree
  - [x] Update border styling in `ui.rs` for 3-panel focus (tree, preview, terminal)

- [x] Task 2: Implement terminal input routing in handler
  - [x] Add `handle_terminal_keys()` function in `handler.rs`
  - [x] When `FocusedPanel::Terminal`: forward all keystrokes to PTY as raw bytes
  - [x] Intercept reserved global keys BEFORE forwarding:
    - `Ctrl+T` → toggle terminal visibility
    - `Ctrl+↑` → resize terminal smaller
    - `Ctrl+↓` → resize terminal larger
    - `Esc` → return focus to Tree panel
  - [x] Map crossterm `KeyEvent` to PTY byte sequences (handle special keys: arrows, Home, End, etc.)
  - [ ] Write tests for key routing logic

- [x] Task 3: Handle terminal mouse events
  - [x] Mouse click in terminal area → set `FocusedPanel::Terminal`
  - [x] Mouse scroll in terminal area → scroll terminal scrollback
  - [x] Update `handle_mouse_event()` in handler.rs

- [ ] Task 4: Conductor - User Manual Verification 'Phase 4' (Protocol in workflow.md)

## Phase 5: Integration, Polish & Cleanup

- [x] Task 1: Working directory sync on open
  - [x] On terminal toggle-open (first spawn): set PTY cwd to `app.current_dir()`
  - [x] On subsequent toggle-open (re-show): do not change directory
  - [x] On close + reopen (new spawn after exit): use current directory

- [x] Task 2: Process lifecycle management
  - [x] Detect shell exit (PTY read returns EOF) → show "[Process exited]" in panel
  - [x] On next toggle: respawn shell in current directory
  - [x] On `App::should_quit`: send SIGHUP, close PTY, wait briefly for cleanup
  - [x] Handle PTY errors gracefully (show error message, don't crash)

- [x] Task 3: Scrollback navigation
  - [x] `Shift+↑/↓` scrolls through scrollback buffer when terminal is focused
  - [x] `Shift+PageUp/PageDown` for fast scrollback navigation
  - [x] Any new output auto-scrolls to bottom (reset scroll offset)

- [x] Task 4: Theme integration and visual polish
  - [x] Use `ThemeColors` for terminal panel border, title, and cursor
  - [x] Terminal panel title: " Terminal " (or " Terminal [exited] " when process ended)
  - [x] Focused border matches existing cyan/theme color scheme

- [ ] Task 5: Update help overlay
  - [ ] Add terminal keybindings section to help overlay (`?` key)
  - [ ] Document: Ctrl+T (toggle), Ctrl+↑/↓ (resize), Esc (unfocus), Tab (cycle)

- [ ] Task 6: CLI flag and config support
  - [ ] Add `--no-terminal` CLI flag to disable terminal feature
  - [ ] Add `[terminal]` section to TOML config: `enabled`, `default_shell`, `scrollback_lines`
  - [ ] Merge config chain: defaults → file → CLI

- [ ] Task 7: Conductor - User Manual Verification 'Phase 5' (Protocol in workflow.md)
