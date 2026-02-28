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

- [x] Task 4: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

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

- [x] Task 3: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)

## Phase 3: App State & UI Layout Integration

- [x] Task 1: Add terminal state to App
  - [x] Create `TerminalState` struct in `src/terminal/mod.rs`
  - [x] Add `terminal_state: TerminalState` to `App` struct
  - [x] Add `Terminal` variant to `FocusedPanel` enum
  - [x] Add `terminal_area: Rect` to App for mouse mapping
  - [x] Implement `App::toggle_terminal()` — spawn PTY on first open, toggle visibility
  - [x] Implement `App::resize_terminal_up/down()` — adjust `height_percent` (min 10%, max 80%)

- [x] Task 2: Modify UI layout for bottom terminal panel
  - [x] Conditional vertical split when visible
  - [x] Store `terminal_area` on App for mouse click mapping

- [x] Task 3: Create TerminalWidget component
  - [x] `src/components/terminal.rs` with existing widget pattern
  - [x] Render emulator output, show cursor when focused
  - [x] Register in `src/components/mod.rs`
  - [x] Write tests for widget rendering

- [x] Task 4: Conductor - User Manual Verification 'Phase 3' (Protocol in workflow.md)

## Phase 4: Input Routing & Focus Management

- [x] Task 1: Update focus cycling logic (3-panel: Tree → Preview → Terminal)
- [x] Task 2: Terminal input routing (handle_terminal_keys, key_event_to_bytes)
- [x] Task 3: Mouse events for terminal area
- [x] Task 4: Conductor - User Manual Verification 'Phase 4' (Protocol in workflow.md)

## Phase 5: Integration, Polish & Cleanup

- [x] Task 1: Working directory sync on open
- [x] Task 2: Process lifecycle management
- [x] Task 3: Scrollback navigation (Shift+↑/↓, Shift+PageUp/PageDown)
- [x] Task 4: Theme integration and visual polish
- [x] Task 5: Update help overlay with terminal keybindings
- [x] Task 6: CLI flag (`--no-terminal`) and config (`[terminal]` section)
- [x] Task 7: Conductor - User Manual Verification 'Phase 5' (Protocol in workflow.md)
