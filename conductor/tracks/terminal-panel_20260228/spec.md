# Spec: Embedded Terminal Panel

## Overview

Add an integrated terminal panel to FileManagerTUI that appears at the bottom of the screen, providing a full interactive PTY shell (bash/zsh). This allows users to run commands, scripts, and interactive tools without leaving the file manager — a critical workflow feature for ML engineers and developers in KubeFlow/Jupyter environments.

## Functional Requirements

### FR-1: PTY Shell Spawning
- Spawn a full pseudo-terminal (PTY) using the system's default shell (`$SHELL` or fallback to `/bin/sh`)
- The PTY must support interactive features: line editing, tab completion, signal handling (Ctrl+C), escape sequences, cursor movement, colors (ANSI 256 / truecolor)
- Support running interactive programs (vim, htop, python REPL, etc.)

### FR-2: Terminal Panel Layout
- Terminal panel appears at the bottom of the screen, below the tree+preview area
- Default height: ~30% of screen (configurable via resize)
- Toggle visibility with a keybinding (`Ctrl+T` or configurable)
- Resize with `Ctrl+↑` (shrink terminal / grow main area) and `Ctrl+↓` (grow terminal / shrink main area)
- Minimum height: 3 lines; Maximum height: 80% of screen
- When hidden, layout reverts to the current tree+preview split

### FR-3: Input Routing & Focus Management
- Add `Terminal` variant to `FocusedPanel` enum
- When terminal is focused, ALL keystrokes are forwarded to the PTY except reserved global keys:
  - `Ctrl+T` — toggle terminal panel visibility
  - `Ctrl+↑` / `Ctrl+↓` — resize terminal panel
  - `Esc` (when terminal focused) — return focus to file manager (tree panel)
- When file manager panels (tree/preview) are focused, existing keybindings work as before
- `Tab` cycles focus: Tree → Preview → Terminal → Tree (when terminal is visible)
- Clear visual indicator of focused panel (border color change, matching existing theme)

### FR-4: Working Directory Sync
- When the terminal is first opened/spawned, set its working directory to the currently selected directory in the tree
- Once opened, the terminal's directory is managed independently by the user
- If the terminal is closed and reopened, it starts fresh in the currently selected directory

### FR-5: Terminal Rendering
- Render terminal output in the bottom panel area using a virtual terminal emulator (parse ANSI escape sequences, maintain a screen buffer)
- Support scrollback buffer (at least 1000 lines)
- Handle terminal resize (send SIGWINCH to the PTY when panel is resized)
- Render using ratatui widgets — the terminal screen buffer maps to styled ratatui `Span`s

### FR-6: Process Lifecycle
- Shell process is spawned on first toggle-open
- Shell process persists when panel is hidden (background), output is buffered
- On app exit, send SIGHUP to the shell process and clean up PTY file descriptors
- Detect shell exit (e.g., user types `exit`) and show "[Process exited]" message

## Non-Functional Requirements

### NFR-1: Performance
- Terminal output must not block the main TUI event loop
- Use async I/O for PTY read/write (tokio integration)
- Buffer PTY output and render at frame rate (~30fps), not per-byte

### NFR-2: Platform Compatibility
- Must work on Linux (primary target: KubeFlow, Jupyter terminals)
- macOS support is nice-to-have
- Use `portable-pty` crate for PTY operations

### NFR-3: Binary Size
- New dependency budget: < 500KB additional to binary size
- Prefer lightweight PTY crates over full terminal emulator frameworks

## Acceptance Criteria

1. User can toggle the terminal panel with `Ctrl+T`
2. Terminal spawns user's default shell in the selected directory
3. User can type commands, see output, use tab completion, run interactive programs
4. `Ctrl+↑/↓` resizes the terminal panel smoothly
5. Focus switching between tree/preview/terminal works correctly with `Tab` and `Esc`
6. Terminal output renders with correct colors and formatting
7. Shell persists when panel is hidden; output is buffered
8. App exits cleanly without orphaned shell processes
9. Scrollback buffer works (Shift+↑/↓ or mouse scroll)
10. All existing functionality remains unaffected when terminal is hidden

## Out of Scope

- Multiple terminal tabs/splits (future enhancement)
- Built-in command palette / custom shell commands
- Automatic `cd` sync when navigating the tree
- Windows support (no PTY on native Windows without ConPTY)
- SSH / remote terminal connections
