# Tree Panel Scroll Improvements

## Overview

Improve the tree panel's scrolling UX by decoupling viewport scrolling from selection,
adding a visual scrollbar with mouse interaction, PageUp/PageDown support, and
configurable scroll speed. Currently, mouse wheel scrolling moves the selection cursor
one item at a time — painfully slow for large trees with hundreds of visible items.

## Functional Requirements

### FR1: Viewport-based Mouse Wheel Scrolling
- Mouse scroll wheel moves the viewport (`scroll_offset`) by N lines (configurable,
  default: 3) **without** moving the selection cursor
- Selection stays at its current position even if scrolled off-screen
- When selection is off-screen and user presses a navigation key (j/k/Up/Down), the
  viewport snaps back to the selection and navigation resumes from there
- Keyboard navigation (j/k/Up/Down/g/G) continues to move selection AND scroll as
  currently implemented (no change to keyboard behavior)

### FR2: PageUp / PageDown Support
- PageUp: Move selection up by one visible page height
- PageDown: Move selection down by one visible page height
- Both clamp at tree boundaries (first/last item)
- Viewport follows selection (same as j/k behavior)

### FR3: Visual Scrollbar
- Render a 1-column-wide scrollbar on the right edge of the tree panel inner area
- Scrollbar track character: `░` (light shade U+2591) in dim/muted color
- Scrollbar thumb character: `█` (full block U+2588) in accent/highlight color
- Thumb size is proportional to (visible_height / total_items), minimum 1 row
- Thumb position represents scroll_offset relative to total scrollable range
- Only rendered when total items exceed visible height (no scrollbar if all items fit)
- Tree content width is reduced by 1 column to accommodate the scrollbar

### FR4: Scrollbar Mouse Interaction
- **Click on track**: Viewport jumps to the proportional position in the tree
  (clicking middle of track scrolls to middle of tree)
- **Click on thumb + drag**: Continuously scroll proportionally as mouse moves
  up/down along the scrollbar column
- **Right-side detection**: Clicks on the scrollbar column are handled by the
  scrollbar, not by tree item selection
- Scrollbar interaction only moves viewport, never moves selection

### FR5: Configurable Scroll Speed
- New config option: `scroll_lines` (integer, default: 3)
- Controls how many lines the viewport moves per mouse scroll tick
- Applied to tree panel mouse scroll only
- Accessible in Settings panel for live editing

## Non-Functional Requirements

- Scrollbar rendering must not impact render performance (O(1) per frame)
- Drag scrolling must be responsive (no perceptible lag)
- Must not break existing keyboard navigation patterns
- Must work correctly with filtered views and paginated (LoadMore) trees

## Acceptance Criteria

1. Mouse scroll wheel moves viewport by `scroll_lines` items without moving selection
2. PageUp/PageDown move selection by visible page height
3. Scrollbar is visible when tree has more items than visible height
4. Scrollbar thumb position accurately reflects scroll position
5. Clicking scrollbar track jumps to proportional position
6. Dragging scrollbar thumb provides smooth proportional scrolling
7. Keyboard navigation (j/k) snaps viewport back to selection after mouse scroll
8. `scroll_lines` config option works with default value of 3
9. All existing tree keyboard shortcuts continue to work identically
10. Tree content width adjusts to accommodate scrollbar column

## Out of Scope

- Momentum/inertia scrolling (over-engineered for TUI)
- Horizontal scrollbar for long filenames
- Scrollbar for preview or terminal panels (separate track if desired)
- Custom scrollbar characters via config
