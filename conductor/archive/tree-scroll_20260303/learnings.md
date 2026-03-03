# Tree Scroll – Learnings

## Inherited Codebase Patterns
- All config fields use `Option<T>` for composable merging via `.or()`
- Store layout `Rect` on App during render → handler uses these for mouse coordinate mapping
- Must account for border offset (y+1) when mapping mouse click row to flat_items index in bordered widgets
- Widget builder pattern: `WidgetName::new(state, theme).block(block)`
- Settings panel entries follow a strict section/key/description/value/default pattern
- `apply_settings_live` maps `(section, key)` pairs to live config updates

## Implementation-Specific Learnings

### Viewport vs Selection Scrolling
- Mouse wheel scrolling now moves `scroll_offset` (viewport) independently from `selected_index` (selection)
- Keyboard navigation (j/k/arrows) continues to move selection, and `update_scroll()` ensures the selection stays visible by adjusting scroll_offset as needed
- This decoupling gives a much better UX for browsing large directories

### Scrollbar Rendering Strategy
- Scrollbar only renders when `total_items > visible_height` (content overflows)
- Content width is reduced by 1 column when scrollbar is present
- Thumb size is proportional: `(visible_height² / total_items).max(1)`
- Thumb position: `scroll * (visible_height - thumb_size) / max_scroll`
- Uses block characters: `█` for thumb, `░` for track
- Scrollbar x-coordinate is stored on App for mouse hit-testing

### Scrollbar Mouse Interaction
- Click-to-jump: maps click row to scroll offset via `inner_y * max_scroll / visible_height`
- Drag-to-scroll: same formula applied continuously during drag events
- `scrollbar_dragging` flag prevents drag events from being handled by other drag handlers (terminal/preview selection)
- Scrollbar clicks return early to avoid triggering normal tree click handling

### Testing Changes
- `mouse_scroll_tree_navigates` test updated to verify scroll_offset changes instead of selected_index changes
- Test also sets `tree_visible_height` (which is normally set during render) to ensure max_scroll calculation works correctly in tests

### Config Integration
- `scroll_lines` added as `Option<u16>` in TreeConfig, default 3, clamped 1..=10
- Settings panel entry uses `UInt` kind for inline numeric editing
- Live-apply handler maps the value directly to `app.config.tree.scroll_lines`
