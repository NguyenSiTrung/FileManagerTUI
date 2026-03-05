# Plan: Double-Click Line Selection in Preview Panel

## Phase 1: Double-Click Detection Infrastructure

- [x] Task 1: Add double-click tracking state to App (commit: 4c1d80d)
  - Add `last_preview_click: Option<(std::time::Instant, u16, u16)>` field to `App`
    struct to store the timestamp and screen position of the last left-click in the
    preview panel.
  - Initialize to `None` in `App::new()`.
  - Sub-tasks:
    - [x] Add field to `App` struct in `app.rs`
    - [x] Initialize in `App::new()`
    - [x] Write unit test: verify field initializes to `None`

- [x] Task 2: Implement double-click detection in `handle_mouse_event` (commit: 4c1d80d)
  - In the `MouseEventKind::Down(MouseButton::Left)` branch for the preview panel,
    check if the current click is within 500ms and at the same screen position as
    `last_preview_click`.
  - If double-click detected:
    - Compute clicked line via `mouse_to_preview_coord`
    - Get line length from `app.preview_state.content_lines[line]`
    - Set `app.preview_selection` anchor to `(line, 0)` and endpoint to `(line, line_length)`
    - Clear `last_preview_click` (consumed)
  - If not double-click:
    - Store `(Instant::now(), col, row)` in `last_preview_click`
    - Proceed with existing single-click drag-start behavior
  - Sub-tasks:
    - [x] Add double-click detection logic before existing preview click handling
    - [x] Compute line content length for endpoint column
    - [x] Set selection anchor and endpoint for full-line select
    - [x] Update `last_preview_click` on single-click
    - [x] Write unit test: double-click within timeout selects full line
    - [x] Write unit test: clicks beyond timeout threshold treated as single-clicks
    - [x] Write unit test: double-click at different positions treated as two single-clicks

- [x] Task: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

## Phase 2: Integration Testing and Polish

- [x] Task 1: End-to-end integration verification (commit: 23d77d6)
  - Verify double-click works with scrolled content.
  - Verify right-click copy after double-click selection works.
  - Verify single-click after double-click clears selection.
  - Verify double-click on tree/terminal panels does not affect preview selection.
  - Sub-tasks:
    - [x] Write integration test: double-click with non-zero scroll_offset
    - [x] Write integration test: selection persists across scroll
    - [x] Manual test: build and run app, double-click lines in preview

- [x] Task 2: Run CI checks (commit: 23d77d6)
  - Sub-tasks:
    - [x] Run `cargo test` — 537 passed
    - [x] Run `cargo clippy -- -D warnings` — clean
    - [x] Run `cargo fmt --check` — clean

- [x] Task: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)
