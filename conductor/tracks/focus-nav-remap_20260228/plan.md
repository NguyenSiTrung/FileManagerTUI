# Plan: Focus Navigation Keybinding Remap

## Phase 1: Directional Focus Navigation & Resize Remap

- [x] Task 1: Add directional focus methods to `App`
  - [x] Add `focus_left()` — moves focus: Preview→Tree, Terminal→Tree
  - [x] Add `focus_right()` — moves focus: Tree→Preview, Terminal→Preview
  - [x] Add `focus_up()` — moves focus: Terminal→Tree (or last horizontal panel)
  - [x] Add `focus_down()` — moves focus to Terminal (if visible)
  - [x] Write unit tests for each method

- [x] Task 2: Update key handling in `handler.rs`
  - [x] Remap `Ctrl+↑/↓` from `resize_terminal_up/down` to `focus_up/down`
  - [x] Add `Ctrl+←/→` handlers calling `focus_left/right`
  - [x] Add `Ctrl+Shift+↑/↓` handlers for terminal resize (replacing old `Ctrl+↑/↓`)
  - [x] Ensure all `Ctrl+Arrow` and `Ctrl+Shift+Arrow` variants are intercepted before terminal key forwarding
  - [x] Write handler-level tests for new keybindings

- [x] Task 3: Update help overlay in `help.rs`
  - [x] Change `Ctrl+↑` / `Ctrl+↓` entries from resize to focus navigation
  - [x] Add `Ctrl+←` / `Ctrl+→` entries for horizontal focus
  - [x] Add `Ctrl+Shift+↑` / `Ctrl+Shift+↓` entries for terminal resize

- [ ] Task 4: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)
