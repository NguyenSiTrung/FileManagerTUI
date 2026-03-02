# Plan: Terminal Panel Mouse Actions and Copy

## Phase 1: Terminal Selection State Foundations

- [x] Task 1: Extend terminal state with selection data structures
  - [x] Add terminal selection anchor/endpoint fields to `TerminalState`
  - [x] Add helpers for clear/set/normalize selection ranges
  - [x] Ensure defaults and debug formatting include new fields safely

- [x] Task 2: Add selection text extraction utilities
  - [x] Add utility to read viewport/scrollback lines from terminal emulator
  - [x] Implement normalized multi-line selection extraction
  - [x] Trim/normalize line endings for clipboard copy compatibility

- [x] Task 3: Add unit tests for selection normalization and extraction
  - [x] Forward drag vs backward drag normalization
  - [x] Single-cell, single-line, and multi-line extraction
  - [x] Empty/invalid range handling

- [x] Task 4: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

## Phase 2: Mouse Event Routing for Terminal Panel

- [x] Task 1: Implement terminal mouse event handling path
  - [x] Add dedicated terminal mouse handler in `handler.rs`
  - [x] Route `Down/Drag/Up` events in terminal area to selection logic
  - [x] Preserve current tree/preview routing behavior

- [x] Task 2: Integrate scroll and selection interactions
  - [x] Keep existing terminal scrollback behavior for wheel events
  - [x] Ensure selection coordinates account for terminal border/inner area offsets
  - [x] Clear selection on click-without-drag

- [x] Task 3: Add mouse routing regression tests
  - [x] Terminal click/drag updates selection state
  - [x] Tree and preview clicks remain unaffected
  - [x] Dialog/Edit mode still bypasses terminal mouse handling

- [x] Task 4: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)

## Phase 3: Clipboard Copy Action Integration

- [x] Task 1: Add terminal copy command in App/handler
  - [x] Add `App` method to copy terminal selection via system clipboard helper
  - [x] Return actionable status text for success/failure/no-selection
  - [x] Keep behavior safe when terminal is hidden or PTY exited

- [x] Task 2: Wire keybinding for terminal copy
  - [x] Handle `Ctrl+Shift+C` in terminal-focused key handling
  - [x] Ensure terminal forwarding does not swallow copy shortcut
  - [x] Keep `Esc` behavior consistent while clearing selection first

- [x] Task 3: Add copy behavior tests
  - [x] Shortcut triggers copy path when selection exists
  - [x] No-selection path shows status hint
  - [x] Clipboard error path surfaces failure message

- [x] Task 4: Conductor - User Manual Verification 'Phase 3' (Protocol in workflow.md)

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
