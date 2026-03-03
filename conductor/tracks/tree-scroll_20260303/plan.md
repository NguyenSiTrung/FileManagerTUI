# Implementation Plan: Tree Scroll Improvements

## Phase 1: Core Scroll Mechanics
<!-- execution: parallel -->

### - [ ] Task 1: Add `scroll_lines` config option
<!-- files: src/config.rs -->
- Add `scroll_lines: Option<u16>` to `AppConfig` in `config.rs`
- Add `pub fn scroll_lines(&self) -> usize` accessor (default: 3)
- Add `DEFAULT_SCROLL_LINES` constant
- Write unit test for default value and custom value parsing

### - [ ] Task 2: Viewport-based mouse wheel scrolling
<!-- files: src/handler.rs, src/app.rs -->
<!-- depends: task1 -->
- Change `handle_mouse_event` ScrollUp/ScrollDown in tree area:
  - Instead of `app.select_previous()` / `app.select_next()`, directly adjust
    `app.tree_state.scroll_offset` by `app.config.scroll_lines()`
  - Clamp scroll_offset to valid range: `0..=(total_items.saturating_sub(visible_height))`
- Add `tree_visible_height: usize` field to `App` (set during render in ui.rs)
  so handler can compute max scroll without re-deriving from tree_area
- Add `App::tree_scroll_up(n)` and `App::tree_scroll_down(n)` methods for clean API

### - [ ] Task 3: Keyboard navigation snap-back
<!-- files: src/fs/tree.rs -->
- Verify that existing `update_scroll()` correctly snaps viewport back to selection
  when j/k/Up/Down is pressed after mouse-scrolling away
- The existing logic should already work since keyboard keys move `selected_index`,
  and `update_scroll()` adjusts `scroll_offset` to keep selection visible
- Write tests to confirm snap-back behavior:
  - Scroll viewport far down → press Up → viewport snaps to selection
  - Scroll viewport far up → press Down → viewport snaps to selection

### - [ ] Task 4: PageUp / PageDown support for tree panel
<!-- files: src/app.rs, src/handler.rs -->
- Add `tree_page_up()` and `tree_page_down()` methods to `App`:
  - `tree_page_down()`: move `selected_index` by `tree_visible_height` items, clamp at end
  - `tree_page_up()`: move `selected_index` by `tree_visible_height` items, clamp at 0
- Handle `KeyCode::PageUp` and `KeyCode::PageDown` in `handle_tree_keys`
- Write tests for PageUp/PageDown at boundaries

### - [ ] Task: Conductor - User Manual Verification 'Core Scroll Mechanics' (Protocol in workflow.md)

---

## Phase 2: Visual Scrollbar Rendering
<!-- execution: parallel -->

### - [ ] Task 1: Add scrollbar theme colors
<!-- files: src/theme.rs -->
- Add `scrollbar_track_fg` and `scrollbar_thumb_fg` fields to `ThemeColors`
- Set dark theme defaults: track = dark gray (Color::Rgb(60,60,60)), thumb = accent cyan
- Set light theme defaults: track = light gray (Color::Rgb(200,200,200)), thumb = accent blue
- Update both `dark()` and `light()` constructors

### - [ ] Task 2: Render scrollbar in TreeWidget
<!-- files: src/components/tree.rs, src/app.rs -->
<!-- depends: task1 -->
- In `TreeWidget::render()`:
  - Calculate if scrollbar is needed: `total_items > visible_height`
  - If needed, reduce content render width by 1 (reserve rightmost column for scrollbar)
  - Calculate thumb position and size:
    - `thumb_size = max(1, visible_height * visible_height / total_items)`
    - `max_scroll = total_items.saturating_sub(visible_height)`
    - `thumb_pos = if max_scroll > 0 { scroll_offset * (visible_height - thumb_size) / max_scroll } else { 0 }`
  - Render track (`░`) on the scrollbar column for all visible rows
  - Render thumb (`█`) over the track at calculated position for `thumb_size` rows
- Add `scrollbar_column: Option<u16>` field to `App` to store the x-coordinate of the
  scrollbar column for mouse hit-testing in Phase 3
- Set `scrollbar_column` during render in `ui.rs` (after tree widget renders)

### - [ ] Task: Conductor - User Manual Verification 'Visual Scrollbar Rendering' (Protocol in workflow.md)

---

## Phase 3: Scrollbar Mouse Interaction

### - [ ] Task 1: Scrollbar click-to-jump
<!-- files: src/handler.rs, src/app.rs -->
- In `handle_mouse_event` MouseDown(Left) for tree area:
  - Check if click column matches `app.scrollbar_column`
  - If on scrollbar: calculate proportional scroll position:
    - `inner_y = row - tree_area.y - 1` (account for border)
    - `ratio = inner_y as f64 / visible_height as f64`
    - `scroll_offset = (ratio * total_items as f64) as usize`
  - Clamp to valid range
  - Set `app.scrollbar_dragging = true`
  - Do NOT change selection
  - Skip normal tree click handling

### - [ ] Task 2: Scrollbar drag-to-scroll
<!-- files: src/handler.rs, src/app.rs -->
- Add `scrollbar_dragging: bool` field to `App` (default: false)
- On MouseDrag when `scrollbar_dragging`:
  - Recalculate proportional scroll position from current mouse row
  - Clamp to valid range
- On MouseUp when `scrollbar_dragging`:
  - Set `scrollbar_dragging = false`
- Handle drag even when mouse moves outside tree area (clamp row to tree bounds)
- Write tests for click-to-jump position mapping

### - [ ] Task: Conductor - User Manual Verification 'Scrollbar Mouse Interaction' (Protocol in workflow.md)

---

## Phase 4: Polish & Integration
<!-- execution: parallel -->

### - [ ] Task 1: Add `scroll_lines` to Settings panel
<!-- files: src/components/settings.rs, src/handler.rs -->
- Register `scroll_lines` in the settings entries list
- Map to `SettingValueKind::Integer` with bounds (1..=10)
- Wire through `apply_settings_live()` to update `app.config` immediately
- Wire through `save_settings_to_file()` for persistence

### - [ ] Task 2: Update documentation and hints
<!-- files: conductor/product.md, src/ui.rs -->
- Update `product.md` Tree Navigation description to mention scrollbar and scroll behavior
- Update status bar key hints for tree panel to include PgUp/PgDn indicator

### - [ ] Task 3: Comprehensive tests
<!-- files: src/app.rs, src/fs/tree.rs, src/handler.rs -->
- Test viewport scroll without selection movement
- Test scroll clamping at boundaries (top and bottom)
- Test PageUp/PageDown boundary behavior
- Test scrollbar thumb position calculation accuracy
- Test scrollbar click-to-jump position mapping
- Test snap-back on keyboard navigation after mouse scroll
- Test scrollbar hidden when all items fit in viewport
- Test with filtered tree views
- Test with paginated (LoadMore) trees

### - [ ] Task: Conductor - User Manual Verification 'Polish & Integration' (Protocol in workflow.md)
