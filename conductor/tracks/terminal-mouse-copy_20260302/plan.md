# Plan: Terminal Panel Mouse Actions and Copy

## Phase 1: Terminal Selection State Foundations

- [ ] Task 1: Extend terminal state with selection data structures
  - [ ] Add terminal selection anchor/endpoint fields to `TerminalState`
  - [ ] Add helpers for clear/set/normalize selection ranges
  - [ ] Ensure defaults and debug formatting include new fields safely

- [ ] Task 2: Add selection text extraction utilities
  - [ ] Add utility to read viewport/scrollback lines from terminal emulator
  - [ ] Implement normalized multi-line selection extraction
  - [ ] Trim/normalize line endings for clipboard copy compatibility

- [ ] Task 3: Add unit tests for selection normalization and extraction
  - [ ] Forward drag vs backward drag normalization
  - [ ] Single-cell, single-line, and multi-line extraction
  - [ ] Empty/invalid range handling

- [ ] Task 4: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

## Phase 2: Mouse Event Routing for Terminal Panel

- [ ] Task 1: Implement terminal mouse event handling path
  - [ ] Add dedicated terminal mouse handler in `handler.rs`
  - [ ] Route `Down/Drag/Up` events in terminal area to selection logic
  - [ ] Preserve current tree/preview routing behavior

- [ ] Task 2: Integrate scroll and selection interactions
  - [ ] Keep existing terminal scrollback behavior for wheel events
  - [ ] Ensure selection coordinates account for terminal border/inner area offsets
  - [ ] Clear selection on click-without-drag

- [ ] Task 3: Add mouse routing regression tests
  - [ ] Terminal click/drag updates selection state
  - [ ] Tree and preview clicks remain unaffected
  - [ ] Dialog/Edit mode still bypasses terminal mouse handling

- [ ] Task 4: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)

## Phase 3: Clipboard Copy Action Integration

- [ ] Task 1: Add terminal copy command in App/handler
  - [ ] Add `App` method to copy terminal selection via system clipboard helper
  - [ ] Return actionable status text for success/failure/no-selection
  - [ ] Keep behavior safe when terminal is hidden or PTY exited

- [ ] Task 2: Wire keybinding for terminal copy
  - [ ] Handle `Ctrl+Shift+C` in terminal-focused key handling
  - [ ] Ensure terminal forwarding does not swallow copy shortcut
  - [ ] Keep `Esc` behavior consistent while clearing selection first

- [ ] Task 3: Add copy behavior tests
  - [ ] Shortcut triggers copy path when selection exists
  - [ ] No-selection path shows status hint
  - [ ] Clipboard error path surfaces failure message

- [ ] Task 4: Conductor - User Manual Verification 'Phase 3' (Protocol in workflow.md)

## Phase 4: Rendering and UX Polish

- [ ] Task 1: Render terminal selection highlight in `TerminalWidget`
  - [ ] Apply selection style while drawing selected cells
  - [ ] Keep cursor rendering readable atop selected content
  - [ ] Verify style contrast for light and dark themes

- [ ] Task 2: Update help text for discoverability
  - [ ] Document terminal mouse drag selection
  - [ ] Document `Ctrl+Shift+C` for terminal copy
  - [ ] Keep keybinding list aligned with actual behavior

- [ ] Task 3: Add rendering/help tests
  - [ ] Selection highlight smoke test in terminal widget tests
  - [ ] Help overlay includes new terminal copy/selection hints

- [ ] Task 4: Conductor - User Manual Verification 'Phase 4' (Protocol in workflow.md)

## Phase 5: Verification and Hardening

- [ ] Task 1: Run quality gates for this track
  - [ ] `cargo test`
  - [ ] `cargo clippy -- -D warnings`
  - [ ] `cargo fmt --check`

- [ ] Task 2: Perform regression checks on terminal lifecycle
  - [ ] Toggle terminal on/off with existing shortcuts
  - [ ] Verify PTY output/input still works after copy interactions
  - [ ] Verify app exit cleanup still shuts terminal down cleanly

- [ ] Task 3: Conductor - User Manual Verification 'Phase 5' (Protocol in workflow.md)
