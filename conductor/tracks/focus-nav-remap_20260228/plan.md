# Plan: Focus Navigation Keybinding Remap

## Phase 1: Directional Focus Navigation & Resize Remap

- [ ] Task 1: Add directional focus methods to `App`
  - [ ] Add `focus_left()` — moves focus: Preview→Tree, Terminal→Tree
  - [ ] Add `focus_right()` — moves focus: Tree→Preview, Terminal→Preview
  - [ ] Add `focus_up()` — moves focus: Terminal→Tree (or last horizontal panel)
  - [ ] Add `focus_down()` — moves focus to Terminal (if visible)
  - [ ] Write unit tests for each method

- [ ] Task 2: Update key handling in `handler.rs`
  - [ ] Remap `Ctrl+↑/↓` from `resize_terminal_up/down` to `focus_up/down`
  - [ ] Add `Ctrl+←/→` handlers calling `focus_left/right`
  - [ ] Add `Ctrl+Shift+↑/↓` handlers for terminal resize (replacing old `Ctrl+↑/↓`)
  - [ ] Ensure all `Ctrl+Arrow` and `Ctrl+Shift+Arrow` variants are intercepted before terminal key forwarding
  - [ ] Write handler-level tests for new keybindings

- [ ] Task 3: Update help overlay in `help.rs`
  - [ ] Change `Ctrl+↑` / `Ctrl+↓` entries from resize to focus navigation
  - [ ] Add `Ctrl+←` / `Ctrl+→` entries for horizontal focus
  - [ ] Add `Ctrl+Shift+↑` / `Ctrl+Shift+↓` entries for terminal resize

- [ ] Task 4: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)
