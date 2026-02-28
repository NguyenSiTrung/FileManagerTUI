# Plan: Embedded Terminal Panel

## Phase 1: PTY Infrastructure & Terminal Module

- [ ] Task 1: Add dependencies and create terminal module structure
  - [ ] Add `portable-pty` crate to Cargo.toml (cross-platform PTY)
  - [ ] Add `vte` crate to Cargo.toml (ANSI escape sequence parser from alacritty)
  - [ ] Create `src/terminal/mod.rs` with module declarations
  - [ ] Create `src/terminal/pty.rs` (PTY process management)
  - [ ] Register `mod terminal` in `main.rs`

- [ ] Task 2: Implement PtyProcess — shell spawning and async I/O
  - [ ] Implement `PtyProcess::spawn(shell, cwd, size)` using `portable-pty`
  - [ ] Implement async PTY output reader (tokio task → mpsc channel)
  - [ ] Implement `write(&[u8])` to send input to PTY stdin
  - [ ] Implement `resize(rows, cols)` to send SIGWINCH
  - [ ] Implement `is_alive()` and `shutdown()` for process lifecycle
  - [ ] Write unit tests for spawn, write, and shutdown

- [ ] Task 3: Add TerminalOutput event to the event system
  - [ ] Add `Event::TerminalOutput(Vec<u8>)` variant to `event.rs`
  - [ ] Wire PTY reader task to send `TerminalOutput` events via the existing `event_tx` channel
  - [ ] Handle `Event::TerminalOutput` in main loop (forward to terminal emulator)

- [ ] Task 4: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

## Phase 2: Terminal Emulation (Screen Buffer)

- [ ] Task 1: Create terminal emulator with VTE parser
  - [ ] Create `src/terminal/emulator.rs`
  - [ ] Define `Cell` struct (character, fg color, bg color, attributes)
  - [ ] Define `TerminalEmulator` struct (grid: Vec<Vec<Cell>>, cursor, size, scrollback)
  - [ ] Implement `vte::Perform` trait for `TerminalEmulator` — handle:
    - [ ] `print()` — regular characters
    - [ ] `execute()` — control characters (CR, LF, BS, TAB, BEL)
    - [ ] `csi_dispatch()` — cursor movement, erase, SGR (colors/styles)
    - [ ] `osc_dispatch()` — terminal title (store but ignore for now)
    - [ ] `esc_dispatch()` — escape sequences
  - [ ] Implement scrollback buffer (ring buffer, configurable size)
  - [ ] Write tests for basic escape sequence handling (cursor move, colors, erase)

- [ ] Task 2: Implement screen buffer to ratatui conversion
  - [ ] Implement `TerminalEmulator::render_lines() -> Vec<Line<'static>>`
  - [ ] Map Cell fg/bg colors to ratatui `Style` (ANSI 16, 256, and RGB)
  - [ ] Map Cell attributes (bold, italic, underline, reverse) to ratatui `Modifier`
  - [ ] Write tests for color/style mapping

- [ ] Task 3: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)

## Phase 3: App State & UI Layout Integration

- [ ] Task 1: Add terminal state to App
  - [ ] Create `TerminalState` struct in `src/terminal/mod.rs`:
    - `emulator: TerminalEmulator`
    - `pty: Option<PtyProcess>`
    - `visible: bool`
    - `height_percent: u16` (default 30)
    - `scroll_offset: usize` (scrollback position)
  - [ ] Add `terminal_state: TerminalState` to `App` struct
  - [ ] Add `Terminal` variant to `FocusedPanel` enum
  - [ ] Add `terminal_area: Rect` to App for mouse mapping
  - [ ] Implement `App::toggle_terminal()` — spawn PTY on first open, toggle visibility
  - [ ] Implement `App::resize_terminal_up/down()` — adjust `height_percent` (min 10%, max 80%)

- [ ] Task 2: Modify UI layout for bottom terminal panel
  - [ ] In `ui.rs`, add conditional vertical split: `[main_area, terminal_area]` when visible
  - [ ] When terminal hidden, use existing layout unchanged
  - [ ] When terminal visible: `[Min(3), Length(term_height), Length(1)]` for tree+preview / terminal / status bar
  - [ ] Store `terminal_area` on App for mouse click mapping

- [ ] Task 3: Create TerminalWidget component
  - [ ] Create `src/components/terminal.rs`
  - [ ] Implement `TerminalWidget` following existing widget pattern (new + block builder)
  - [ ] Render emulator's `render_lines()` output with scrollback support
  - [ ] Show cursor position when terminal is focused
  - [ ] Register in `src/components/mod.rs`
  - [ ] Write tests for widget rendering

- [ ] Task 4: Conductor - User Manual Verification 'Phase 3' (Protocol in workflow.md)

## Phase 4: Input Routing & Focus Management

- [ ] Task 1: Update focus cycling logic
  - [ ] Modify `App::toggle_focus()` to cycle: Tree → Preview → Terminal (when visible) → Tree
  - [ ] When terminal is hidden, cycle as before: Tree → Preview → Tree
  - [ ] Update border styling in `ui.rs` for 3-panel focus (tree, preview, terminal)

- [ ] Task 2: Implement terminal input routing in handler
  - [ ] Add `handle_terminal_keys()` function in `handler.rs`
  - [ ] When `FocusedPanel::Terminal`: forward all keystrokes to PTY as raw bytes
  - [ ] Intercept reserved global keys BEFORE forwarding:
    - `Ctrl+T` → toggle terminal visibility
    - `Ctrl+↑` → resize terminal smaller
    - `Ctrl+↓` → resize terminal larger
    - `Esc` → return focus to Tree panel
  - [ ] Map crossterm `KeyEvent` to PTY byte sequences (handle special keys: arrows, Home, End, etc.)
  - [ ] Write tests for key routing logic

- [ ] Task 3: Handle terminal mouse events
  - [ ] Mouse click in terminal area → set `FocusedPanel::Terminal`
  - [ ] Mouse scroll in terminal area → scroll terminal scrollback
  - [ ] Update `handle_mouse_event()` in handler.rs

- [ ] Task 4: Conductor - User Manual Verification 'Phase 4' (Protocol in workflow.md)

## Phase 5: Integration, Polish & Cleanup

- [ ] Task 1: Working directory sync on open
  - [ ] On terminal toggle-open (first spawn): set PTY cwd to `app.current_dir()`
  - [ ] On subsequent toggle-open (re-show): do not change directory
  - [ ] On close + reopen (new spawn after exit): use current directory

- [ ] Task 2: Process lifecycle management
  - [ ] Detect shell exit (PTY read returns EOF) → show "[Process exited]" in panel
  - [ ] On next toggle: respawn shell in current directory
  - [ ] On `App::should_quit`: send SIGHUP, close PTY, wait briefly for cleanup
  - [ ] Handle PTY errors gracefully (show error message, don't crash)

- [ ] Task 3: Scrollback navigation
  - [ ] `Shift+↑/↓` scrolls through scrollback buffer when terminal is focused
  - [ ] `Shift+PageUp/PageDown` for fast scrollback navigation
  - [ ] Any new output auto-scrolls to bottom (reset scroll offset)

- [ ] Task 4: Theme integration and visual polish
  - [ ] Use `ThemeColors` for terminal panel border, title, and cursor
  - [ ] Terminal panel title: " Terminal " (or " Terminal [exited] " when process ended)
  - [ ] Focused border matches existing cyan/theme color scheme

- [ ] Task 5: Update help overlay
  - [ ] Add terminal keybindings section to help overlay (`?` key)
  - [ ] Document: Ctrl+T (toggle), Ctrl+↑/↓ (resize), Esc (unfocus), Tab (cycle)

- [ ] Task 6: CLI flag and config support
  - [ ] Add `--no-terminal` CLI flag to disable terminal feature
  - [ ] Add `[terminal]` section to TOML config: `enabled`, `default_shell`, `scrollback_lines`
  - [ ] Merge config chain: defaults → file → CLI

- [ ] Task 7: Conductor - User Manual Verification 'Phase 5' (Protocol in workflow.md)
