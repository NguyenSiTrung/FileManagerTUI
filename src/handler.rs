use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use tokio::sync::mpsc;

use crate::app::{App, AppMode, DialogKind, FocusedPanel};
use crate::components::help::HelpOverlay;
use crate::event::Event;
use crate::fs::operations;
use crate::fs::tree::NodeType;

/// Handle a mouse event.
pub fn handle_mouse_event(
    app: &mut App,
    mouse: MouseEvent,
    event_tx: &mpsc::UnboundedSender<Event>,
) {
    // Handle mouse in Edit mode for editor cursor positioning
    if app.mode == AppMode::Edit {
        handle_editor_mouse(app, mouse);
        return;
    }

    // Only handle mouse in Normal mode for other panels
    if app.mode != AppMode::Normal {
        return;
    }

    let col = mouse.column;
    let row = mouse.row;

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Determine which panel was clicked
            if is_in_rect(col, row, app.tree_area) {
                // Check if click is on the scrollbar column
                if let Some(sb_col) = app.scrollbar_column {
                    if col == sb_col {
                        // Scrollbar click-to-jump: map click row to scroll offset
                        app.focused_panel = FocusedPanel::Tree;
                        app.terminal_state.selection.clear();
                        app.preview_selection.clear();
                        app.scrollbar_dragging = true;
                        app.tree_viewport_locked = true;

                        let inner_y = row.saturating_sub(app.tree_area.y + 1) as usize;
                        let visible_height = app.tree_visible_height;
                        let total = app.tree_state.flat_items.len();
                        let max_scroll = total.saturating_sub(visible_height);

                        if visible_height > 1 && max_scroll > 0 {
                            let track_max = visible_height - 1;
                            let clamped_y = inner_y.min(track_max);
                            let new_offset = clamped_y * max_scroll / track_max;
                            app.tree_state.scroll_offset = new_offset.min(max_scroll);
                        }
                        // Don't fall through to normal tree click handling
                        return;
                    }
                }

                // Clear any terminal selection when clicking elsewhere
                app.terminal_state.selection.clear();
                app.preview_selection.clear();
                // Switch focus to tree
                app.focused_panel = FocusedPanel::Tree;
                // Unlock viewport so update_scroll works for clicked item
                app.tree_viewport_locked = false;

                // Map click to tree item index
                // Inner area: subtract border (1 top, 1 left)
                let inner_y = row.saturating_sub(app.tree_area.y + 1);
                let clicked_index = app.tree_state.scroll_offset + inner_y as usize;

                if clicked_index < app.tree_state.flat_items.len() {
                    let already_selected = app.tree_state.selected_index == clicked_index;
                    app.tree_state.selected_index = clicked_index;
                    app.last_previewed_index = None; // Force preview update

                    // If clicking already-selected item, toggle expand/collapse or load more
                    if already_selected {
                        if let Some(item) = app.tree_state.flat_items.get(clicked_index) {
                            if item.node_type == NodeType::LoadMore {
                                if let Some(parent_path) = item.load_more_parent.clone() {
                                    let loaded = app.tree_state.load_next_page(&parent_path);
                                    if loaded > 0 {
                                        app.set_status_message(format!(
                                            "Loaded {} more entries",
                                            loaded
                                        ));
                                        app.invalidate_search_cache();
                                    }
                                }
                            } else if item.node_type == NodeType::Directory {
                                if item.is_expanded {
                                    app.collapse_selected();
                                } else {
                                    app.expand_selected_async(event_tx);
                                }
                            }
                        }
                    }
                }
            } else if is_in_rect(col, row, app.preview_area) {
                // Clear any terminal selection when clicking elsewhere
                app.terminal_state.selection.clear();
                // Switch focus to preview
                app.focused_panel = FocusedPanel::Preview;

                // Double-click detection: check if this click is within 500ms
                // and at the same screen position as the last preview click.
                let now = std::time::Instant::now();
                let is_double_click = app
                    .last_preview_click
                    .take()
                    .map(|(ts, prev_col, prev_row)| {
                        now.duration_since(ts).as_millis() <= 500
                            && prev_col == col
                            && prev_row == row
                    })
                    .unwrap_or(false);

                if is_double_click {
                    // Double-click: select entire line
                    if let Some(coord) = mouse_to_preview_coord(app, col, row, false) {
                        let line_len = app
                            .preview_state
                            .content_lines
                            .get(coord.line)
                            .map(|l| {
                                // Get the plain text width of the line
                                l.spans.iter().map(|s| s.content.len()).sum::<usize>()
                            })
                            .unwrap_or(0);
                        app.preview_selection
                            .set_anchor(crate::terminal::TerminalCoord {
                                line: coord.line,
                                col: 0,
                            });
                        app.preview_selection
                            .set_endpoint(crate::terminal::TerminalCoord {
                                line: coord.line,
                                col: line_len,
                            });
                        // Don't store last_preview_click — consumed
                    } else {
                        app.preview_selection.clear();
                    }
                } else {
                    // Single click: start drag selection
                    app.last_preview_click = Some((now, col, row));
                    if let Some(coord) = mouse_to_preview_coord(app, col, row, false) {
                        app.preview_selection.begin_drag(coord);
                    } else {
                        app.preview_selection.clear();
                    }
                }
            } else if app.terminal_state.visible && is_in_rect(col, row, app.terminal_area) {
                // Switch focus to terminal and start/clear selection
                app.focused_panel = FocusedPanel::Terminal;
                app.preview_selection.clear();
                if let Some(coord) = mouse_to_terminal_coord(app, col, row, false) {
                    // Click sets anchor (clears any previous selection by overwriting)
                    app.terminal_state.selection.begin_drag(coord);
                }
            }
        }
        MouseEventKind::Down(MouseButton::Right) => {
            if app.terminal_state.visible && is_in_rect(col, row, app.terminal_area) {
                app.focused_panel = FocusedPanel::Terminal;
                app.copy_terminal_selection(event_tx);
            } else if is_in_rect(col, row, app.preview_area) {
                app.focused_panel = FocusedPanel::Preview;
                app.copy_preview_selection(event_tx);
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            // Scrollbar drag-to-scroll
            if app.scrollbar_dragging {
                let inner_y = row.saturating_sub(app.tree_area.y + 1) as usize;
                let visible_height = app.tree_visible_height;
                let total = app.tree_state.flat_items.len();
                let max_scroll = total.saturating_sub(visible_height);

                if visible_height > 1 && max_scroll > 0 {
                    let track_max = visible_height - 1;
                    let clamped_y = inner_y.min(track_max);
                    let new_offset = clamped_y * max_scroll / track_max;
                    app.tree_state.scroll_offset = new_offset.min(max_scroll);
                }
            } else if app.terminal_state.visible && app.terminal_state.selection.dragging {
                if let Some(coord) = mouse_to_terminal_coord(app, col, row, true) {
                    app.terminal_state.selection.set_endpoint(coord);
                }
            } else if app.preview_selection.dragging {
                if let Some(coord) = mouse_to_preview_coord(app, col, row, true) {
                    app.preview_selection.set_endpoint(coord);
                }
            }
        }
        MouseEventKind::Moved => {
            // Fallback for terminals that emit Moved (not Drag) during left-button drag.
            if app.scrollbar_dragging {
                let inner_y = row.saturating_sub(app.tree_area.y + 1) as usize;
                let visible_height = app.tree_visible_height;
                let total = app.tree_state.flat_items.len();
                let max_scroll = total.saturating_sub(visible_height);

                if visible_height > 1 && max_scroll > 0 {
                    let track_max = visible_height - 1;
                    let clamped_y = inner_y.min(track_max);
                    let new_offset = clamped_y * max_scroll / track_max;
                    app.tree_state.scroll_offset = new_offset.min(max_scroll);
                }
            } else if app.terminal_state.visible && app.terminal_state.selection.dragging {
                if let Some(coord) = mouse_to_terminal_coord(app, col, row, true) {
                    app.terminal_state.selection.set_endpoint(coord);
                }
            } else if app.preview_selection.dragging {
                if let Some(coord) = mouse_to_preview_coord(app, col, row, true) {
                    app.preview_selection.set_endpoint(coord);
                }
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            // End scrollbar drag
            if app.scrollbar_dragging {
                app.scrollbar_dragging = false;
            }
            if app.terminal_state.selection.dragging {
                // Final endpoint update allows releasing outside panel bounds.
                if let Some(coord) = mouse_to_terminal_coord(app, col, row, true) {
                    app.terminal_state.selection.set_endpoint(coord);
                }
                app.terminal_state.selection.end_drag();
            } else if app.preview_selection.dragging {
                if let Some(coord) = mouse_to_preview_coord(app, col, row, true) {
                    app.preview_selection.set_endpoint(coord);
                }
                app.preview_selection.end_drag();
            }
            // If anchor == endpoint after click-release (no drag), clear selection
            if let Some((start, end)) = app.terminal_state.selection.normalized() {
                if start == end {
                    app.terminal_state.selection.clear();
                }
            }
            if let Some((start, end)) = app.preview_selection.normalized() {
                if start == end {
                    app.preview_selection.clear();
                }
            }
        }
        MouseEventKind::ScrollUp => {
            if is_in_rect(col, row, app.tree_area) {
                app.focused_panel = FocusedPanel::Tree;
                let n = app.config.scroll_lines();
                app.tree_scroll_up(n);
            } else if is_in_rect(col, row, app.preview_area) {
                app.focused_panel = FocusedPanel::Preview;
                app.preview_scroll_up();
            } else if app.terminal_state.visible && is_in_rect(col, row, app.terminal_area) {
                // Scroll up in terminal scrollback
                let max = app
                    .terminal_state
                    .emulator
                    .total_lines()
                    .saturating_sub(app.terminal_state.emulator.visible_rows());
                if app.terminal_state.scroll_offset < max {
                    app.terminal_state.scroll_offset += 1;
                }
            }
        }
        MouseEventKind::ScrollDown => {
            if is_in_rect(col, row, app.tree_area) {
                app.focused_panel = FocusedPanel::Tree;
                let n = app.config.scroll_lines();
                app.tree_scroll_down(n);
            } else if is_in_rect(col, row, app.preview_area) {
                app.focused_panel = FocusedPanel::Preview;
                app.preview_scroll_down();
            } else if app.terminal_state.visible && is_in_rect(col, row, app.terminal_area) {
                app.terminal_state.scroll_offset =
                    app.terminal_state.scroll_offset.saturating_sub(1);
            }
        }
        _ => {}
    }
}

/// Convert mouse screen coordinates to terminal-local absolute coordinates.
/// Returns None if the position is outside the terminal inner area.
fn mouse_to_terminal_coord(
    app: &App,
    mouse_col: u16,
    mouse_row: u16,
    clamp_to_inner: bool,
) -> Option<crate::terminal::TerminalCoord> {
    let area = app.terminal_area;
    // Account for border offset (1 pixel on each side)
    let inner_x = area.x + 1;
    let inner_y = area.y + 1;
    let inner_w = area.width.saturating_sub(2);
    let inner_h = area.height.saturating_sub(2);

    if inner_w == 0 || inner_h == 0 {
        return None;
    }

    let (effective_col, effective_row) = if clamp_to_inner {
        let max_x = inner_x + inner_w - 1;
        let max_y = inner_y + inner_h - 1;
        (
            mouse_col.clamp(inner_x, max_x),
            mouse_row.clamp(inner_y, max_y),
        )
    } else {
        if mouse_col < inner_x
            || mouse_row < inner_y
            || mouse_col >= inner_x + inner_w
            || mouse_row >= inner_y + inner_h
        {
            return None;
        }
        (mouse_col, mouse_row)
    };

    let local_col = (effective_col - inner_x) as usize;
    let local_row = (effective_row - inner_y) as usize;

    // Convert viewport row to absolute line, accounting for scroll offset.
    // scroll_offset=0 means we're at the bottom (live view).
    // The viewport shows lines from (total - visible_rows - scroll_offset) to
    // (total - 1 - scroll_offset).
    let total = app.terminal_state.emulator.total_lines();
    let visible = app.terminal_state.emulator.visible_rows();
    let first_visible_abs = total.saturating_sub(visible + app.terminal_state.scroll_offset);
    let abs_line = first_visible_abs + local_row;

    Some(crate::terminal::TerminalCoord {
        line: abs_line,
        col: local_col,
    })
}

/// Convert mouse screen coordinates to preview-local absolute coordinates.
/// Returns None if the position is outside the preview inner area or preview is empty.
fn mouse_to_preview_coord(
    app: &App,
    mouse_col: u16,
    mouse_row: u16,
    clamp_to_inner: bool,
) -> Option<crate::terminal::TerminalCoord> {
    let area = app.preview_area;
    let inner_x = area.x + 1;
    let inner_y = area.y + 1;
    let inner_w = area.width.saturating_sub(2);
    let inner_h = area.height.saturating_sub(2);

    if inner_w == 0 || inner_h == 0 || app.preview_state.content_lines.is_empty() {
        return None;
    }

    let (effective_col, effective_row) = if clamp_to_inner {
        let max_x = inner_x + inner_w - 1;
        let max_y = inner_y + inner_h - 1;
        (
            mouse_col.clamp(inner_x, max_x),
            mouse_row.clamp(inner_y, max_y),
        )
    } else {
        if mouse_col < inner_x
            || mouse_row < inner_y
            || mouse_col >= inner_x + inner_w
            || mouse_row >= inner_y + inner_h
        {
            return None;
        }
        (mouse_col, mouse_row)
    };

    let local_col = (effective_col - inner_x) as usize;
    let local_row = (effective_row - inner_y) as usize;
    let visible_height = inner_h as usize;
    let max_start = app
        .preview_state
        .content_lines
        .len()
        .saturating_sub(visible_height);
    let start = app.preview_state.scroll_offset.min(max_start);
    let abs_line = (start + local_row).min(app.preview_state.content_lines.len() - 1);

    Some(crate::terminal::TerminalCoord {
        line: abs_line,
        col: local_col,
    })
}

/// Handle mouse events when in editor mode.
fn handle_editor_mouse(app: &mut App, mouse: MouseEvent) {
    let col = mouse.column;
    let row = mouse.row;

    // Only handle clicks within the preview/editor area
    if !is_in_rect(col, row, app.preview_area) {
        return;
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(ref mut editor) = app.editor_state {
                let (target_line, target_col) =
                    mouse_to_editor_pos(editor, app.preview_area, col, row);
                // Place cursor and start a new selection anchor
                editor.set_cursor_position(target_line, target_col);
                // Set anchor at the click point so dragging will create a selection
                editor.selection = Some(crate::editor::Selection::new(
                    editor.cursor_line,
                    editor.cursor_col,
                ));
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(ref mut editor) = app.editor_state {
                let (target_line, target_col) =
                    mouse_to_editor_pos(editor, app.preview_area, col, row);
                // Move cursor without clearing selection — anchor stays put
                editor.set_cursor_position_for_selection(target_line, target_col);
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            // If anchor == cursor after click-release (no drag), clear selection
            if let Some(ref mut editor) = app.editor_state {
                if let Some(ref sel) = editor.selection {
                    if sel.anchor_line == editor.cursor_line && sel.anchor_col == editor.cursor_col
                    {
                        editor.selection = None;
                    }
                }
            }
        }
        MouseEventKind::ScrollUp => {
            if let Some(ref mut editor) = app.editor_state {
                editor.scroll_offset = editor.scroll_offset.saturating_sub(3);
                editor.ensure_cursor_visible();
            }
        }
        MouseEventKind::ScrollDown => {
            if let Some(ref mut editor) = app.editor_state {
                let max_scroll = editor.line_count().saturating_sub(1);
                editor.scroll_offset = (editor.scroll_offset + 3).min(max_scroll);
                editor.ensure_cursor_visible();
            }
        }
        _ => {}
    }
}

/// Convert mouse screen coordinates to editor (line, col) position.
fn mouse_to_editor_pos(
    editor: &crate::editor::EditorState,
    preview_area: ratatui::layout::Rect,
    col: u16,
    row: u16,
) -> (usize, usize) {
    let inner_x = preview_area.x + 1;
    let inner_y = preview_area.y + 1;
    let gutter_w = editor.gutter_width();
    let code_x = inner_x + gutter_w;

    let click_row = row.saturating_sub(inner_y) as usize;
    let target_line = editor.scroll_offset + click_row;

    let target_col = if col >= code_x {
        (col - code_x) as usize
    } else {
        0
    };

    (target_line, target_col)
}

/// Check if a position (col, row) is inside a Rect.
fn is_in_rect(col: u16, row: u16, rect: ratatui::layout::Rect) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}

/// Handle a key event and dispatch to the appropriate app method.
pub fn handle_key_event(app: &mut App, key: KeyEvent, event_tx: &mpsc::UnboundedSender<Event>) {
    // Ignore key release events to prevent duplicate actions from press/release pairs.
    if key.kind == KeyEventKind::Release {
        return;
    }

    match &app.mode {
        AppMode::Normal => handle_normal_mode(app, key, event_tx),
        AppMode::Dialog(_) => handle_dialog_mode(app, key),
        AppMode::Search => handle_search_mode(app, key),
        AppMode::SearchAction => handle_search_action_mode(app, key, event_tx),
        AppMode::Filter => handle_filter_mode(app, key),
        AppMode::Help => handle_help_mode(app, key),
        AppMode::Edit => handle_editor_keys(app, key),
        AppMode::CopyOverlay => {
            // Only Esc or Enter close the copy overlay.
            // Mouse capture will be re-enabled by the main loop.
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                let path_msg = app.copy_overlay_text.take().unwrap_or_default();
                app.mode = AppMode::Normal;
                app.set_status_message(format!("📋 Path: {}", path_msg));
                // Signal the main loop to re-enable mouse capture
                let _ = event_tx.send(Event::ClipboardCopyComplete(String::new()));
            }
        }
    }
}

/// Handle keys when in Edit mode (editing a file in the preview panel).
fn handle_editor_keys(app: &mut App, key: KeyEvent) {
    // If find bar is active, handle find/replace keys first
    if app
        .editor_state
        .as_ref()
        .is_some_and(|e| e.find_state.active)
    {
        handle_editor_find_keys(app, key);
        return;
    }

    match key.code {
        // Exit edit mode
        KeyCode::Esc => {
            let is_modified = app.editor_state.as_ref().is_some_and(|e| e.modified);
            if is_modified {
                // Show save confirmation dialog
                app.mode = AppMode::Dialog(DialogKind::SaveConfirm);
            } else {
                app.exit_edit_mode();
            }
        }

        // Save
        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let _ = app.save_editor_buffer();
        }

        // Undo/Redo
        KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.undo();
            }
        }
        KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.redo();
            }
        }

        // Select all (Ctrl+A)
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.select_all();
            }
        }

        // Find / Replace
        KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.open_find();
            }
        }
        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.open_find_replace();
            }
        }

        // Editor clipboard
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.copy_line();
            }
        }
        KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.cut_line();
            }
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.paste();
            }
        }

        // Selection-aware navigation (Shift+Arrow) — must match before plain navigation
        KeyCode::Home
            if key
                .modifiers
                .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT) =>
        {
            if let Some(ref mut editor) = app.editor_state {
                editor.select_to_top();
            }
        }
        KeyCode::End
            if key
                .modifiers
                .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT) =>
        {
            if let Some(ref mut editor) = app.editor_state {
                editor.select_to_bottom();
            }
        }
        KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.select_up();
            }
        }
        KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.select_down();
            }
        }
        KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.select_left();
            }
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.select_right();
            }
        }
        KeyCode::Home if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.select_home();
            }
        }
        KeyCode::End if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.select_end();
            }
        }
        KeyCode::PageUp if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.select_page_up();
            }
        }
        KeyCode::PageDown if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.select_page_down();
            }
        }

        // Navigation with Ctrl modifiers
        KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.move_to_top();
            }
        }
        KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.move_to_bottom();
            }
        }

        // Basic navigation
        KeyCode::Up => {
            if let Some(ref mut editor) = app.editor_state {
                editor.move_up();
            }
        }
        KeyCode::Down => {
            if let Some(ref mut editor) = app.editor_state {
                editor.move_down();
            }
        }
        KeyCode::Left => {
            if let Some(ref mut editor) = app.editor_state {
                editor.move_left();
            }
        }
        KeyCode::Right => {
            if let Some(ref mut editor) = app.editor_state {
                editor.move_right();
            }
        }
        KeyCode::Home => {
            if let Some(ref mut editor) = app.editor_state {
                editor.move_home();
            }
        }
        KeyCode::End => {
            if let Some(ref mut editor) = app.editor_state {
                editor.move_end();
            }
        }
        KeyCode::PageUp => {
            if let Some(ref mut editor) = app.editor_state {
                editor.page_up();
            }
        }
        KeyCode::PageDown => {
            if let Some(ref mut editor) = app.editor_state {
                editor.page_down();
            }
        }

        // Editing
        KeyCode::Enter => {
            if let Some(ref mut editor) = app.editor_state {
                editor.insert_newline();
                editor.ensure_cursor_visible();
            }
        }
        KeyCode::Backspace => {
            if let Some(ref mut editor) = app.editor_state {
                editor.delete_char_before();
                editor.ensure_cursor_visible();
            }
        }
        KeyCode::Delete => {
            if let Some(ref mut editor) = app.editor_state {
                editor.delete_char_at();
            }
        }
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.dedent();
            }
        }
        KeyCode::Tab => {
            if let Some(ref mut editor) = app.editor_state {
                editor.insert_tab();
            }
        }

        // Character input
        KeyCode::Char(c) => {
            if let Some(ref mut editor) = app.editor_state {
                editor.insert_char(c);
                editor.ensure_cursor_visible();
            }
        }

        _ => {}
    }
}

/// Handle keys when the find/replace bar is active in editor mode.
fn handle_editor_find_keys(app: &mut App, key: KeyEvent) {
    let editor = match app.editor_state.as_mut() {
        Some(e) => e,
        None => return,
    };

    match key.code {
        KeyCode::Esc => {
            editor.close_find();
            app.mode = AppMode::Edit;
        }
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
            editor.find_previous();
        }
        KeyCode::Enter => {
            if editor.find_state.replace_mode && editor.find_state.in_replace_field {
                editor.replace_current();
            } else {
                editor.find_next();
            }
        }
        KeyCode::Tab => {
            if editor.find_state.replace_mode {
                editor.find_state.in_replace_field = !editor.find_state.in_replace_field;
            }
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if editor.find_state.replace_mode {
                let count = editor.replace_all();
                app.set_status_message(format!(
                    "Replaced {} occurrence{}",
                    count,
                    if count == 1 { "" } else { "s" }
                ));
            }
        }
        KeyCode::Backspace => {
            if editor.find_state.in_replace_field {
                if editor.find_state.replacement_cursor > 0 {
                    let pos = editor.find_state.replacement_cursor;
                    let prev_char = editor.find_state.replacement[..pos]
                        .chars()
                        .next_back()
                        .expect("cursor > 0 guarantees at least one char");
                    editor.find_state.replacement_cursor -= prev_char.len_utf8();
                    editor
                        .find_state
                        .replacement
                        .remove(editor.find_state.replacement_cursor);
                }
            } else if editor.find_state.query_cursor > 0 {
                let pos = editor.find_state.query_cursor;
                let prev_char = editor.find_state.query[..pos]
                    .chars()
                    .next_back()
                    .expect("cursor > 0 guarantees at least one char");
                editor.find_state.query_cursor -= prev_char.len_utf8();
                editor
                    .find_state
                    .query
                    .remove(editor.find_state.query_cursor);
                editor.update_find_matches();
            }
        }
        KeyCode::Char(c) => {
            if editor.find_state.in_replace_field {
                let pos = editor.find_state.replacement_cursor;
                editor.find_state.replacement.insert(pos, c);
                editor.find_state.replacement_cursor += c.len_utf8();
            } else {
                let pos = editor.find_state.query_cursor;
                editor.find_state.query.insert(pos, c);
                editor.find_state.query_cursor += c.len_utf8();
                editor.update_find_matches();
            }
        }
        _ => {}
    }
}

fn handle_normal_mode(app: &mut App, key: KeyEvent, event_tx: &mpsc::UnboundedSender<Event>) {
    // Reserved global keys (must check BEFORE terminal forwarding)
    // These keys are intercepted regardless of which panel is focused.
    match key.code {
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let _ = app.toggle_terminal(event_tx);
            return;
        }
        // Directional focus navigation: Ctrl+Arrow
        KeyCode::Left
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            app.focus_left();
            return;
        }
        KeyCode::Right
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            app.focus_right();
            return;
        }
        KeyCode::Up
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            app.focus_up();
            return;
        }
        KeyCode::Down
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            app.focus_down();
            return;
        }
        // Terminal resize: Ctrl+Shift+Arrow
        KeyCode::Up
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            app.resize_terminal_up();
            return;
        }
        KeyCode::Down
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            app.resize_terminal_down();
            return;
        }
        _ => {}
    }

    // If terminal is focused, forward all other keys to the PTY
    if app.focused_panel == FocusedPanel::Terminal {
        handle_terminal_keys(app, key, event_tx);
        return;
    }

    // Global keys (work regardless of focus for tree/preview panels)
    match key.code {
        // Copy preview selection when preview is focused.
        // Supports Ctrl+Shift+C, Ctrl+C (when selection exists), Cmd+C, and Ctrl+Insert.
        KeyCode::Char('C')
            if app.focused_panel == FocusedPanel::Preview
                && (key.modifiers.contains(KeyModifiers::CONTROL)
                    || key.modifiers.contains(KeyModifiers::SUPER)) =>
        {
            app.copy_preview_selection(event_tx);
            return;
        }
        KeyCode::Char('c')
            if app.focused_panel == FocusedPanel::Preview
                && ((key.modifiers.contains(KeyModifiers::CONTROL)
                    && (key.modifiers.contains(KeyModifiers::SHIFT)
                        || app.preview_selection.is_active()))
                    || key.modifiers.contains(KeyModifiers::SUPER)) =>
        {
            app.copy_preview_selection(event_tx);
            return;
        }
        KeyCode::Insert
            if app.focused_panel == FocusedPanel::Preview
                && key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            app.copy_preview_selection(event_tx);
            return;
        }
        KeyCode::Char('q') => {
            app.quit();
            return;
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.quit();
            return;
        }
        // Bare 't' toggles terminal (alternative to Ctrl+T, avoids browser conflict)
        KeyCode::Char('t') if key.modifiers.is_empty() => {
            let _ = app.toggle_terminal(event_tx);
            return;
        }
        KeyCode::Tab => {
            app.toggle_focus();
            return;
        }
        KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.undo();
            return;
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.open_search();
            return;
        }
        KeyCode::Char('/') => {
            app.start_filter();
            return;
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.toggle_watcher();
            return;
        }
        KeyCode::F(5) => {
            app.full_refresh();
            return;
        }
        KeyCode::Char('?') => {
            app.help_state.scroll_offset = 0;
            app.mode = AppMode::Help;
            return;
        }
        _ => {}
    }

    // Dispatch based on focused panel
    match app.focused_panel {
        FocusedPanel::Tree => handle_tree_keys(app, key, event_tx),
        FocusedPanel::Preview => handle_preview_keys(app, key, event_tx),
        FocusedPanel::Terminal => {} // Already handled above
    }
}

fn handle_tree_keys(app: &mut App, key: KeyEvent, event_tx: &mpsc::UnboundedSender<Event>) {
    // Any keyboard action in the tree unlocks the viewport so it follows selection
    app.tree_viewport_locked = false;

    // S3 mode: block write operations with user-friendly message
    if app.is_s3_mode() {
        match key.code {
            KeyCode::Char('a') | KeyCode::Char('A') | KeyCode::Char('d') | KeyCode::Char('r')
            | KeyCode::Char('p') | KeyCode::Char('x') => {
                app.set_status_message("☁ S3 mode is read-only".to_string());
                return;
            }
            _ => {}
        }
    }

    match key.code {
        // Navigation
        KeyCode::Char('j') | KeyCode::Down => app.select_next(),
        KeyCode::Char('k') | KeyCode::Up => app.select_previous(),
        KeyCode::Char('g') | KeyCode::Home => app.select_first(),
        KeyCode::Char('G') | KeyCode::End => app.select_last(),
        KeyCode::PageUp => app.tree_page_up(),
        KeyCode::PageDown => app.tree_page_down(),

        // Tree expand/collapse / Load more
        KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
            if let Some(item) = app.tree_state.flat_items.get(app.tree_state.selected_index) {
                if item.node_type == NodeType::LoadMore {
                    // Trigger load_next_page on the parent directory
                    if let Some(parent_path) = item.load_more_parent.clone() {
                        let loaded = app.tree_state.load_next_page(&parent_path);
                        if loaded > 0 {
                            app.set_status_message(format!("Loaded {} more entries", loaded));
                            app.invalidate_search_cache();
                        }
                    }
                } else if app.is_s3_mode() && item.node_type == NodeType::Directory {
                    // S3 expand: use S3 listing instead of filesystem
                    let s3_uri = item.path.to_string_lossy().to_string();
                    app.spawn_s3_expand(s3_uri, event_tx);
                } else {
                    app.expand_selected_async(event_tx);
                }
            }
        }
        KeyCode::Backspace | KeyCode::Char('h') | KeyCode::Left => app.collapse_selected(),

        // Toggle hidden files
        KeyCode::Char('.') => app.toggle_hidden(),

        // Multi-select toggle
        KeyCode::Char(' ') => app.tree_state.toggle_multi_select(),

        // Clear multi-selection
        KeyCode::Esc => app.tree_state.clear_multi_select(),

        // Clipboard operations (skip LoadMore nodes)
        // Note: y (copy) works in S3 mode — copies S3 URI to internal clipboard
        KeyCode::Char('y') => {
            if app
                .tree_state
                .flat_items
                .get(app.tree_state.selected_index)
                .is_some_and(|i| i.node_type != NodeType::LoadMore)
            {
                app.copy_to_clipboard();
            }
        }
        KeyCode::Char('x') => {
            if app
                .tree_state
                .flat_items
                .get(app.tree_state.selected_index)
                .is_some_and(|i| i.node_type != NodeType::LoadMore)
            {
                app.cut_to_clipboard();
            }
        }
        KeyCode::Char('p') => app.paste_clipboard_async(event_tx.clone()),

        // File operations — open dialogs
        // Note: these are unreachable in S3 mode (blocked by guard above)
        KeyCode::Char('a') => app.open_dialog(DialogKind::CreateFile),
        KeyCode::Char('A') => app.open_dialog(DialogKind::CreateDirectory),
        KeyCode::Char('r') => {
            if let Some(item) = app.tree_state.flat_items.get(app.tree_state.selected_index) {
                if item.node_type == NodeType::LoadMore {
                    return; // Can't rename a virtual node
                }
                let original = item.path.clone();
                app.open_dialog(DialogKind::Rename { original });
            }
        }
        KeyCode::Char('d') => {
            if let Some(item) = app.tree_state.flat_items.get(app.tree_state.selected_index) {
                // Don't allow deleting the root or LoadMore nodes
                if item.depth > 0 && item.node_type != NodeType::LoadMore {
                    let targets = vec![item.path.clone()];
                    app.open_dialog(DialogKind::DeleteConfirm { targets });
                }
            }
        }

        // Sort options
        KeyCode::Char('s') => {
            app.tree_state.cycle_sort();
            app.set_status_message(format!("Sort: {}", app.tree_state.sort_by.label()));
        }
        KeyCode::Char('S') => {
            app.tree_state.toggle_dirs_first();
            app.set_status_message(format!(
                "Dirs first: {}",
                if app.tree_state.dirs_first {
                    "on"
                } else {
                    "off"
                }
            ));
        }

        // Copy path to system clipboard
        // In S3 mode, copies the S3 URI
        KeyCode::Char('Y') => {
            app.copy_path_to_system_clipboard(event_tx);
        }

        // Open selected item's directory in the terminal panel
        KeyCode::Char('T') => {
            if app.is_s3_mode() {
                app.set_status_message("☁ Terminal unavailable in S3 mode".to_string());
            } else {
                app.open_terminal_at_selected(event_tx);
            }
        }

        // Open file/directory with system default application
        KeyCode::Char('o') => {
            if app.is_s3_mode() {
                app.set_status_message("☁ System open unavailable in S3 mode".to_string());
            } else {
                app.open_in_system();
            }
        }

        _ => {}
    }
}

fn handle_preview_keys(app: &mut App, key: KeyEvent, event_tx: &mpsc::UnboundedSender<Event>) {
    match key.code {
        // Enter edit mode (not available in S3 mode)
        KeyCode::Char('e') => {
            if app.is_s3_mode() {
                app.set_status_message("☁ Editing unavailable in S3 mode".to_string());
            } else {
                app.enter_edit_mode();
            }
        }
        // Deep scan trigger (only when showing shallow preview)
        KeyCode::Char('D') => {
            if app.preview_state.is_shallow_preview {
                if let Some(ref path) = app.preview_state.current_path.clone() {
                    app.preview_state.is_shallow_preview = false;
                    // Show scanning placeholder
                    let dir_name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.to_string_lossy().to_string());
                    let placeholder = format!("📁 Directory: {}\n\n  Deep scanning...", dir_name);
                    app.preview_state.content_lines = placeholder
                        .lines()
                        .map(|l| ratatui::text::Line::raw(l.to_string()))
                        .collect();
                    app.preview_state.total_lines = app.preview_state.content_lines.len();
                    app.spawn_async_dir_summary(path, event_tx);
                    app.set_status_message("Deep scan started...".to_string());
                }
            }
        }
        // Line-by-line scroll
        KeyCode::Char('j') | KeyCode::Down => app.preview_scroll_down(),
        KeyCode::Char('k') | KeyCode::Up => app.preview_scroll_up(),
        // Jump to top/bottom
        KeyCode::Char('g') | KeyCode::Home => app.preview_jump_top(),
        KeyCode::Char('G') | KeyCode::End => app.preview_jump_bottom(),
        // Half-page scroll
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.preview_half_page_down(30);
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.preview_half_page_up(30);
        }

        // Adjust head/tail line counts
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.adjust_preview_lines(crate::preview_content::LINE_COUNT_STEP as isize);
        }
        KeyCode::Char('-') => {
            app.adjust_preview_lines(-(crate::preview_content::LINE_COUNT_STEP as isize));
        }

        _ => {}
    }
}

/// Handle keys when terminal panel is focused.
/// All non-reserved keys are forwarded to the PTY as raw bytes.
fn handle_terminal_keys(app: &mut App, key: KeyEvent, event_tx: &mpsc::UnboundedSender<Event>) {
    match key.code {
        // Esc: if selection active, clear it first; otherwise return focus to tree
        KeyCode::Esc => {
            if app.terminal_state.selection.is_active() {
                app.terminal_state.selection.clear();
            } else {
                app.focused_panel = FocusedPanel::Tree;
            }
            return;
        }
        // Copy terminal selection to system clipboard.
        // Supports Ctrl+Shift+C, Ctrl+C (when selection exists), and Cmd+C on macOS terminals
        // that pass SUPER-modified keys through to the app.
        KeyCode::Char('C')
            if key.modifiers.contains(KeyModifiers::CONTROL)
                || key.modifiers.contains(KeyModifiers::SUPER) =>
        {
            app.copy_terminal_selection(event_tx);
            return;
        }
        KeyCode::Char('c')
            if (key.modifiers.contains(KeyModifiers::CONTROL)
                && (key.modifiers.contains(KeyModifiers::SHIFT)
                    || app.terminal_state.selection.is_active()))
                || key.modifiers.contains(KeyModifiers::SUPER) =>
        {
            app.copy_terminal_selection(event_tx);
            return;
        }
        // Legacy terminal copy shortcut.
        KeyCode::Insert if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.copy_terminal_selection(event_tx);
            return;
        }
        // Note: Tab is NOT intercepted here — it is forwarded to the PTY
        // for shell autocompletion (e.g. `cd <Tab>`).
        // Use Esc or Ctrl+T to leave the terminal panel.
        // (Bare 't' is NOT intercepted here — it types into the PTY.)
        //
        // Scrollback navigation (Shift+Up/Down)
        KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if app.terminal_state.scroll_offset
                < app
                    .terminal_state
                    .emulator
                    .total_lines()
                    .saturating_sub(app.terminal_state.emulator.visible_rows())
            {
                app.terminal_state.scroll_offset += 1;
            }
            return;
        }
        KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
            app.terminal_state.scroll_offset = app.terminal_state.scroll_offset.saturating_sub(1);
            return;
        }
        KeyCode::PageUp if key.modifiers.contains(KeyModifiers::SHIFT) => {
            let jump = app.terminal_state.emulator.visible_rows() / 2;
            let max = app
                .terminal_state
                .emulator
                .total_lines()
                .saturating_sub(app.terminal_state.emulator.visible_rows());
            app.terminal_state.scroll_offset = (app.terminal_state.scroll_offset + jump).min(max);
            return;
        }
        KeyCode::PageDown if key.modifiers.contains(KeyModifiers::SHIFT) => {
            let jump = app.terminal_state.emulator.visible_rows() / 2;
            app.terminal_state.scroll_offset =
                app.terminal_state.scroll_offset.saturating_sub(jump);
            return;
        }
        _ => {}
    }

    // Any non-scroll key input clears selection and resets scroll
    app.terminal_state.selection.clear();
    app.terminal_state.scroll_offset = 0;

    // Convert KeyEvent to bytes and send to PTY
    let bytes = key_event_to_bytes(&key);
    if !bytes.is_empty() {
        if let Some(ref pty) = app.terminal_state.pty {
            let _ = pty.write(&bytes);
        }
    }
}

/// Convert a crossterm KeyEvent into the byte sequence expected by a PTY.
fn key_event_to_bytes(key: &KeyEvent) -> Vec<u8> {
    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                // Ctrl+A..Z → 0x01..0x1A
                let ctrl_byte = (c.to_ascii_lowercase() as u8)
                    .wrapping_sub(b'a')
                    .wrapping_add(1);
                if ctrl_byte <= 26 {
                    return vec![ctrl_byte];
                }
            }
            let mut buf = [0u8; 4];
            let s = c.encode_utf8(&mut buf);
            s.as_bytes().to_vec()
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::F(n) => match n {
            1 => b"\x1bOP".to_vec(),
            2 => b"\x1bOQ".to_vec(),
            3 => b"\x1bOR".to_vec(),
            4 => b"\x1bOS".to_vec(),
            5 => b"\x1b[15~".to_vec(),
            6 => b"\x1b[17~".to_vec(),
            7 => b"\x1b[18~".to_vec(),
            8 => b"\x1b[19~".to_vec(),
            9 => b"\x1b[20~".to_vec(),
            10 => b"\x1b[21~".to_vec(),
            11 => b"\x1b[23~".to_vec(),
            12 => b"\x1b[24~".to_vec(),
            _ => vec![],
        },
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Esc => vec![0x1b],
        _ => vec![],
    }
}

fn handle_search_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.close_search(),
        KeyCode::Enter => app.search_confirm(),
        KeyCode::Down | KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.search_select_next();
        }
        KeyCode::Up | KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.search_select_previous();
        }
        KeyCode::Down => app.search_select_next(),
        KeyCode::Up => app.search_select_previous(),
        KeyCode::Backspace => app.search_delete_char(),
        KeyCode::Char(c) => app.search_input_char(c),
        _ => {}
    }
}

fn handle_search_action_mode(
    app: &mut App,
    key: KeyEvent,
    event_tx: &mpsc::UnboundedSender<Event>,
) {
    let state = match &app.search_action_state {
        Some(s) => s.clone(),
        None => {
            app.close_search_action();
            return;
        }
    };

    match key.code {
        KeyCode::Esc => app.search_action_back(),
        // Navigate (Go to) — always available
        KeyCode::Enter => {
            app.search_action_navigate();
        }
        // Preview — hidden for directories
        KeyCode::Char('p') if !state.is_directory => {
            app.search_action_preview();
        }
        // Edit — hidden for directories and binary files
        KeyCode::Char('e') if !state.is_directory && !state.is_binary => {
            app.search_action_edit();
        }
        // Copy path — always available
        KeyCode::Char('y') => {
            app.search_action_copy_path(event_tx);
        }
        // Rename — always available
        KeyCode::Char('r') => {
            app.search_action_rename();
        }
        // Delete — always available
        KeyCode::Char('d') => {
            app.search_action_delete();
        }
        // Copy (clipboard) — always available
        KeyCode::Char('c') => {
            app.search_action_copy_clipboard();
        }
        // Cut (clipboard) — always available
        KeyCode::Char('x') => {
            app.search_action_cut_clipboard();
        }
        // Open in terminal — always available
        KeyCode::Char('t') => {
            app.search_action_open_terminal(event_tx);
        }
        _ => {}
    }
}

fn handle_filter_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.clear_filter(),
        KeyCode::Enter => app.accept_filter(),
        KeyCode::Backspace => app.filter_delete_char(),
        KeyCode::Char(c) => app.filter_input_char(c),
        _ => {}
    }
}

fn handle_help_mode(app: &mut App, key: KeyEvent) {
    use crate::components::help::HelpTab;
    use crate::components::settings::SettingsState;

    let theme = app.theme_colors.clone();
    let total = HelpOverlay::new(&theme, &app.help_state).total_lines_for_tab();

    // If in settings tab and editing, route keys to edit mode
    if app.help_state.active_tab == HelpTab::Settings {
        if let Some(ref mut settings) = app.help_state.settings_state {
            if settings.editing {
                match key.code {
                    KeyCode::Enter => {
                        settings.confirm_edit();
                    }
                    KeyCode::Esc => {
                        settings.cancel_edit();
                    }
                    KeyCode::Backspace => {
                        settings.edit_buffer.pop();
                    }
                    KeyCode::Char(c) => {
                        settings.edit_buffer.push(c);
                    }
                    _ => {}
                }
                return;
            }
        }
    }

    match key.code {
        KeyCode::Char('?') => {
            app.mode = AppMode::Normal;
        }
        KeyCode::Esc => {
            if app.help_state.active_tab == HelpTab::Settings {
                // Check for unsaved changes
                let has_changes = app
                    .help_state
                    .settings_state
                    .as_ref()
                    .is_some_and(|s| s.modified_count() > 0);
                if has_changes {
                    app.set_status_message(
                        "Discard unsaved settings changes? Press Esc again to confirm, or Ctrl+S to save".to_string(),
                    );
                    // Clear modifications and close
                    if let Some(ref mut settings) = app.help_state.settings_state {
                        settings.clear_modifications();
                    }
                }
            }
            app.mode = AppMode::Normal;
        }
        // Tab switching: Tab, Shift+Tab, Left, Right
        KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right
            if app.help_state.active_tab == HelpTab::Keybindings
                || !matches!(key.code, KeyCode::Left | KeyCode::Right) =>
        {
            let new_tab = app.help_state.active_tab.toggle();
            app.help_state.active_tab = new_tab;
            app.help_state.scroll_offset = 0; // Reset scroll on tab switch

            // Lazily initialize settings state
            if new_tab == HelpTab::Settings && app.help_state.settings_state.is_none() {
                app.help_state.settings_state = Some(SettingsState::from_config(&app.config));
            }
        }
        // Scroll keys
        KeyCode::Char('j') | KeyCode::Down => {
            if app.help_state.active_tab == HelpTab::Settings {
                if let Some(ref mut settings) = app.help_state.settings_state {
                    if settings.selected_index < settings.entries.len().saturating_sub(1) {
                        settings.select_next();
                    } else {
                        // Already at last entry: keep scrolling the view
                        settings.scroll_offset += 1;
                    }
                }
            } else if app.help_state.scroll_offset < total.saturating_sub(1) {
                app.help_state.scroll_offset += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.help_state.active_tab == HelpTab::Settings {
                if let Some(ref mut settings) = app.help_state.settings_state {
                    if settings.selected_index > 0 {
                        settings.select_prev();
                    } else {
                        // Already at first entry: scroll the view up
                        settings.scroll_offset = settings.scroll_offset.saturating_sub(1);
                    }
                }
            } else {
                app.help_state.scroll_offset = app.help_state.scroll_offset.saturating_sub(1);
            }
        }
        KeyCode::Char('g') | KeyCode::Home => {
            if app.help_state.active_tab == HelpTab::Settings {
                if let Some(ref mut settings) = app.help_state.settings_state {
                    settings.select_first();
                }
            } else {
                app.help_state.scroll_offset = 0;
            }
        }
        KeyCode::Char('G') | KeyCode::End => {
            if app.help_state.active_tab == HelpTab::Settings {
                if let Some(ref mut settings) = app.help_state.settings_state {
                    settings.select_last();
                    // Scroll all the way to the bottom of the content
                    settings.scroll_offset = usize::MAX; // will be clamped
                }
            } else {
                app.help_state.scroll_offset = total.saturating_sub(1);
            }
        }
        // Settings-specific keys
        KeyCode::Enter | KeyCode::Char(' ') if app.help_state.active_tab == HelpTab::Settings => {
            if let Some(ref mut settings) = app.help_state.settings_state {
                if let Some(entry) = settings.entries.get(settings.selected_index) {
                    match &entry
                        .modified_value
                        .as_ref()
                        .unwrap_or(&entry.current_value)
                    {
                        crate::components::settings::SettingValueKind::Bool(_) => {
                            settings.toggle_bool();
                        }
                        crate::components::settings::SettingValueKind::Enum(_, _) => {
                            settings.cycle_enum();
                        }
                        crate::components::settings::SettingValueKind::UInt(_)
                        | crate::components::settings::SettingValueKind::Str(_) => {
                            settings.start_editing();
                        }
                    }
                }
            }
        }
        // Reset to default
        KeyCode::Backspace | KeyCode::Delete if app.help_state.active_tab == HelpTab::Settings => {
            if let Some(ref mut settings) = app.help_state.settings_state {
                settings.reset_to_default();
            }
        }
        // Save settings
        KeyCode::Char('s')
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && app.help_state.active_tab == HelpTab::Settings =>
        {
            if let Some(ref settings) = app.help_state.settings_state {
                if settings.modified_count() > 0 {
                    app.open_dialog(DialogKind::SaveSettings);
                } else {
                    app.set_status_message("No settings modified".to_string());
                }
            }
        }
        // Open config file in editor
        KeyCode::Char('o') if app.help_state.active_tab == HelpTab::Settings => {
            let config_path = dirs::config_dir().map(|d| d.join("fm-tui").join("config.toml"));
            if let Some(path) = config_path {
                if path.exists() {
                    app.mode = AppMode::Normal;
                    // Navigate to config file and enter edit mode
                    app.preview_state.current_path = Some(path.clone());
                    app.update_preview();
                    app.enter_edit_mode();
                } else {
                    app.set_status_message(
                        "Config file does not exist yet. Save settings first (Ctrl+S)".to_string(),
                    );
                }
            }
        }
        _ => {}
    }
}

fn handle_dialog_mode(app: &mut App, key: KeyEvent) {
    let kind = match &app.mode {
        AppMode::Dialog(kind) => kind.clone(),
        _ => return,
    };

    match &kind {
        DialogKind::DeleteConfirm { targets } => {
            handle_delete_confirm(app, key, targets.clone());
        }
        DialogKind::Error { .. } => {
            handle_error_dialog(app, key);
        }
        DialogKind::Progress { .. } => {
            handle_progress_dialog(app, key);
        }
        DialogKind::SaveConfirm => {
            handle_save_confirm(app, key);
        }
        DialogKind::SaveSettings => {
            handle_save_settings_dialog(app, key);
        }
        _ => {
            handle_input_dialog(app, key, kind);
        }
    }
}

fn handle_input_dialog(app: &mut App, key: KeyEvent, kind: DialogKind) {
    match key.code {
        KeyCode::Esc => app.close_dialog(),
        KeyCode::Enter => {
            let input = app.dialog_state.input.clone();
            if input.is_empty() {
                app.close_dialog();
                return;
            }
            execute_input_operation(app, &kind, &input);
        }
        KeyCode::Char(c) => app.dialog_input_char(c),
        KeyCode::Backspace => app.dialog_delete_char(),
        KeyCode::Left => app.dialog_move_cursor_left(),
        KeyCode::Right => app.dialog_move_cursor_right(),
        KeyCode::Home => app.dialog_cursor_home(),
        KeyCode::End => app.dialog_cursor_end(),
        KeyCode::Delete => {
            // Forward delete: move right then backspace
            if app.dialog_state.cursor_position < app.dialog_state.input.len() {
                app.dialog_move_cursor_right();
                app.dialog_delete_char();
            }
        }
        _ => {}
    }
}

fn execute_input_operation(app: &mut App, kind: &DialogKind, input: &str) {
    match kind {
        DialogKind::CreateFile => {
            let dir = app.current_dir();
            let path = dir.join(input);
            match operations::create_file(&path) {
                Ok(()) => {
                    app.set_status_message(format!("Created file: {}", input));
                    app.tree_state.reload_dir(&dir);
                    app.invalidate_search_cache();
                }
                Err(e) => {
                    app.set_status_message(format!("Error: {}", e));
                }
            }
        }
        DialogKind::CreateDirectory => {
            let dir = app.current_dir();
            let path = dir.join(input);
            match operations::create_dir(&path) {
                Ok(()) => {
                    app.set_status_message(format!("Created directory: {}", input));
                    app.tree_state.reload_dir(&dir);
                    app.invalidate_search_cache();
                }
                Err(e) => {
                    app.set_status_message(format!("Error: {}", e));
                }
            }
        }
        DialogKind::Rename { original } => {
            if let Some(parent) = original.parent() {
                let new_path = parent.join(input);
                match operations::rename(original, &new_path) {
                    Ok(()) => {
                        app.last_undo = Some(crate::app::UndoAction::Rename {
                            from: original.clone(),
                            to: new_path,
                        });
                        app.set_status_message(format!("Renamed to: {}", input));
                        app.tree_state.reload_dir(parent);
                        app.invalidate_search_cache();
                    }
                    Err(e) => {
                        app.set_status_message(format!("Error: {}", e));
                    }
                }
            }
        }
        _ => {}
    }
    app.close_dialog();
}

fn handle_delete_confirm(app: &mut App, key: KeyEvent, targets: Vec<std::path::PathBuf>) {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            let mut errors = Vec::new();
            for target in &targets {
                if let Err(e) = operations::delete(target) {
                    errors.push(format!("{}: {}", target.display(), e));
                }
            }
            if errors.is_empty() {
                let names: Vec<String> = targets
                    .iter()
                    .filter_map(|t| t.file_name().map(|n| n.to_string_lossy().to_string()))
                    .collect();
                app.set_status_message(format!("Deleted: {}", names.join(", ")));
                // Reload parent directories
                for target in &targets {
                    if let Some(parent) = target.parent() {
                        app.tree_state.reload_dir(parent);
                    }
                }
                app.invalidate_search_cache();
            } else {
                app.set_status_message(format!("Error: {}", errors.join("; ")));
            }
            app.close_dialog();
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            app.close_dialog();
        }
        _ => {}
    }
}

fn handle_error_dialog(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter | KeyCode::Esc => app.close_dialog(),
        _ => {}
    }
}

fn handle_progress_dialog(app: &mut App, key: KeyEvent) {
    if key.code == KeyCode::Esc {
        app.cancel_operation();
        app.close_dialog();
        app.set_status_message("Operation cancelled".to_string());
    }
}

/// Handle the save confirmation dialog when exiting edit mode with unsaved changes.
/// Y/y = Save and exit, N/n = Discard and exit, Esc/C/c = Cancel (stay in edit mode).
fn handle_save_confirm(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            if app.save_editor_buffer().is_ok() {
                app.close_dialog();
                app.exit_edit_mode();
            }
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            app.close_dialog();
            app.exit_edit_mode();
            app.set_status_message("Changes discarded".to_string());
        }
        KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('C') => {
            // Cancel — return to edit mode
            app.mode = AppMode::Edit;
        }
        _ => {}
    }
}

/// Handle the save settings dialog (Global / Local / Cancel).
fn handle_save_settings_dialog(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('g') | KeyCode::Char('G') => {
            // Save to global config
            let path = dirs::config_dir().map(|d| d.join("fm-tui").join("config.toml"));
            if let Some(path) = path {
                save_settings_to_file(app, &path);
            } else {
                app.set_status_message("Could not determine config directory".to_string());
                app.close_dialog();
            }
        }
        KeyCode::Char('l') | KeyCode::Char('L') => {
            // Save to local config (.fm-tui.toml in current directory)
            let cwd = app.tree_state.root.path.clone();
            let path = cwd.join(".fm-tui.toml");
            save_settings_to_file(app, &path);
        }
        KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('C') => {
            // Back to help/settings
            app.mode = AppMode::Help;
        }
        _ => {}
    }
}

/// Save modified settings to a TOML file and apply them live.
fn save_settings_to_file(app: &mut App, path: &std::path::Path) {
    use crate::components::settings::SettingValueKind;

    let settings = match &app.help_state.settings_state {
        Some(s) => s,
        None => {
            app.set_status_message("No settings state to save".to_string());
            app.close_dialog();
            return;
        }
    };

    // Build a TOML table from modified entries, merging with existing file content
    let existing_content = if path.exists() {
        std::fs::read_to_string(path).unwrap_or_default()
    } else {
        String::new()
    };

    let mut doc: toml::Table = existing_content.parse::<toml::Table>().unwrap_or_default();

    for entry in &settings.entries {
        let value = match &entry.modified_value {
            Some(v) => v,
            None => continue, // Not modified, skip
        };

        // Get or create the section table
        let section = doc
            .entry(entry.section.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));

        if let toml::Value::Table(ref mut table) = section {
            let toml_value = match value {
                SettingValueKind::Bool(b) => toml::Value::Boolean(*b),
                SettingValueKind::UInt(n) => toml::Value::Integer(*n as i64),
                SettingValueKind::Str(s) => toml::Value::String(s.clone()),
                SettingValueKind::Enum(s, _) => toml::Value::String(s.clone()),
            };
            table.insert(entry.key.to_string(), toml_value);
        }
    }

    // Serialize the table to a TOML string
    let toml_string =
        toml::to_string_pretty(&doc).unwrap_or_else(|e| format!("# Failed to serialize: {}\n", e));

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                app.set_status_message(format!("Failed to create config directory: {}", e));
                app.close_dialog();
                return;
            }
        }
    }

    // Write the file
    match std::fs::write(path, &toml_string) {
        Ok(()) => {
            // Apply live: update app.config with the modified values
            apply_settings_live(app);

            // Clear modification markers
            if let Some(ref mut settings) = app.help_state.settings_state {
                settings.clear_modifications();
            }

            app.set_status_message(format!("✓ Settings saved to {}", path.display()));
            app.mode = AppMode::Help;
        }
        Err(e) => {
            app.set_status_message(format!("Failed to save settings: {}", e));
            app.close_dialog();
        }
    }
}

/// Apply modified settings from the settings state into the live app config.
fn apply_settings_live(app: &mut App) {
    use crate::components::settings::SettingValueKind;

    let settings = match &app.help_state.settings_state {
        Some(s) => s,
        None => return,
    };

    for entry in &settings.entries {
        let value = match &entry.modified_value {
            Some(v) => v,
            None => continue,
        };

        match (entry.section, entry.key) {
            ("general", "show_hidden") => {
                if let SettingValueKind::Bool(b) = value {
                    app.config.general.show_hidden = Some(*b);
                    app.tree_state.show_hidden = *b;
                    app.tree_state.sort_all_children();
                    app.tree_state.flatten();
                }
            }
            ("general", "confirm_delete") => {
                if let SettingValueKind::Bool(b) = value {
                    app.config.general.confirm_delete = Some(*b);
                }
            }
            ("general", "mouse") => {
                if let SettingValueKind::Bool(b) = value {
                    app.config.general.mouse = Some(*b);
                }
            }
            ("general", "max_entries_per_page") => {
                if let SettingValueKind::UInt(n) = value {
                    app.config.general.max_entries_per_page = Some(*n as u32);
                }
            }
            ("general", "search_max_entries") => {
                if let SettingValueKind::UInt(n) = value {
                    app.config.general.search_max_entries = Some(*n as u32);
                }
            }
            ("general", "snapshot_max_entries") => {
                if let SettingValueKind::UInt(n) = value {
                    app.config.general.snapshot_max_entries = Some(*n as u32);
                }
            }
            ("general", "max_editor_bytes") => {
                if let SettingValueKind::UInt(n) = value {
                    app.config.general.max_editor_bytes = Some(*n);
                }
            }
            ("general", "max_editor_lines") => {
                if let SettingValueKind::UInt(n) = value {
                    app.config.general.max_editor_lines = Some(*n);
                }
            }
            ("preview", "enabled") => {
                if let SettingValueKind::Bool(b) = value {
                    app.config.preview.enabled = Some(*b);
                }
            }
            ("preview", "max_full_preview_bytes") => {
                if let SettingValueKind::UInt(n) = value {
                    app.config.preview.max_full_preview_bytes = Some(*n);
                }
            }
            ("preview", "head_lines") => {
                if let SettingValueKind::UInt(n) = value {
                    app.config.preview.head_lines = Some(*n as usize);
                }
            }
            ("preview", "tail_lines") => {
                if let SettingValueKind::UInt(n) = value {
                    app.config.preview.tail_lines = Some(*n as usize);
                }
            }
            ("preview", "default_view_mode") => {
                if let SettingValueKind::Enum(s, _) = value {
                    app.config.preview.default_view_mode = Some(s.clone());
                }
            }
            ("preview", "tab_width") => {
                if let SettingValueKind::UInt(n) = value {
                    app.config.preview.tab_width = Some(*n as usize);
                }
            }
            ("preview", "line_wrap") => {
                if let SettingValueKind::Bool(b) = value {
                    app.config.preview.line_wrap = Some(*b);
                }
            }
            ("preview", "syntax_theme") => {
                if let SettingValueKind::Str(s) = value {
                    if s.trim().is_empty() {
                        app.config.preview.syntax_theme = None;
                    } else {
                        app.config.preview.syntax_theme = Some(s.clone());
                    }
                    let syntax_theme_name = app
                        .config
                        .syntax_theme_name(app.config.theme_scheme())
                        .to_string();
                    app.syntax_theme = crate::preview_content::load_theme(Some(&syntax_theme_name));
                    app.last_previewed_index = None;
                }
            }
            ("tree", "sort_by") => {
                if let SettingValueKind::Enum(s, _) = value {
                    app.config.tree.sort_by = Some(s.clone());
                    app.tree_state.sort_by = crate::fs::tree::SortBy::from_str(s);
                    app.tree_state.sort_all_children();
                    app.tree_state.flatten();
                }
            }
            ("tree", "dirs_first") => {
                if let SettingValueKind::Bool(b) = value {
                    app.config.tree.dirs_first = Some(*b);
                    app.tree_state.dirs_first = *b;
                    app.tree_state.sort_all_children();
                    app.tree_state.flatten();
                }
            }
            ("tree", "use_icons") => {
                if let SettingValueKind::Bool(b) = value {
                    app.config.tree.use_icons = Some(*b);
                }
            }
            ("tree", "scroll_lines") => {
                if let SettingValueKind::UInt(n) = value {
                    app.config.tree.scroll_lines = Some(*n as u16);
                }
            }
            ("watcher", "enabled") => {
                if let SettingValueKind::Bool(b) = value {
                    app.config.watcher.enabled = Some(*b);
                }
            }
            ("watcher", "debounce_ms") => {
                if let SettingValueKind::UInt(n) = value {
                    app.config.watcher.debounce_ms = Some(*n);
                }
            }
            ("watcher", "auto_refresh") => {
                if let SettingValueKind::Bool(b) = value {
                    app.config.watcher.auto_refresh = Some(*b);
                    app.watcher_active = *b;
                }
            }
            ("terminal", "enabled") => {
                if let SettingValueKind::Bool(b) = value {
                    app.config.terminal.enabled = Some(*b);
                }
            }
            ("terminal", "default_shell") => {
                if let SettingValueKind::Str(s) = value {
                    app.config.terminal.default_shell = Some(s.clone());
                }
            }
            ("terminal", "scrollback_lines") => {
                if let SettingValueKind::UInt(n) = value {
                    app.config.terminal.scrollback_lines = Some(*n as usize);
                }
            }
            ("theme", "scheme") => {
                if let SettingValueKind::Enum(s, _) = value {
                    app.config.theme.scheme = Some(s.clone());
                    app.theme_colors = crate::theme::resolve_theme(&app.config.theme);
                    let syntax_theme_name = app.config.syntax_theme_name(s).to_string();
                    app.syntax_theme = crate::preview_content::load_theme(Some(&syntax_theme_name));
                    app.last_previewed_index = None;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use std::fs::{self, File};
    use tempfile::TempDir;

    fn make_key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn make_key_with_modifiers(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn make_event_tx() -> mpsc::UnboundedSender<Event> {
        let (tx, _rx) = mpsc::unbounded_channel();
        tx
    }

    /// Test helper: handle_key_event with a dummy event sender.
    fn handle_key(app: &mut App, key: KeyEvent) {
        let tx = make_event_tx();
        handle_key_event(app, key, &tx);
    }

    /// Synchronously load root children for an App created with deferred loading.
    /// Tests use this because they can't await async events.
    fn sync_load_root(app: &mut App) {
        let page_size = app.tree_state.page_size;
        let sort_by = app.tree_state.sort_by.clone();
        let dirs_first = app.tree_state.dirs_first;
        let root = &mut app.tree_state.root;
        let _ = root.load_children_paged_with_sort(page_size, &sort_by, dirs_first);
        root.is_loading = false;
        root.is_expanded = true;
        crate::fs::tree::TreeState::sort_children_of_pub(root, &sort_by, dirs_first);
        app.tree_state.sort_all_children();
        app.tree_state.flatten();
    }

    fn setup_app() -> (TempDir, App) {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("alpha")).unwrap();
        fs::create_dir(dir.path().join("beta")).unwrap();
        File::create(dir.path().join("file_a.txt")).unwrap();
        File::create(dir.path().join(".hidden")).unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        sync_load_root(&mut app);
        // Enable watcher for tests that assert watcher_active state.
        app.watcher_active = true;
        (dir, app)
    }

    // === Normal mode tests (existing) ===

    #[test]
    fn key_j_moves_down() {
        let (_dir, mut app) = setup_app();
        handle_key(&mut app, make_key(KeyCode::Char('j')));
        assert_eq!(app.tree_state.selected_index, 1);
    }

    #[test]
    fn key_k_moves_up() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 2;
        handle_key(&mut app, make_key(KeyCode::Char('k')));
        assert_eq!(app.tree_state.selected_index, 1);
    }

    #[test]
    fn key_down_arrow_moves_down() {
        let (_dir, mut app) = setup_app();
        handle_key(&mut app, make_key(KeyCode::Down));
        assert_eq!(app.tree_state.selected_index, 1);
    }

    #[test]
    fn key_up_arrow_moves_up() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 1;
        handle_key(&mut app, make_key(KeyCode::Up));
        assert_eq!(app.tree_state.selected_index, 0);
    }

    #[test]
    fn key_g_jumps_to_first() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 3;
        handle_key(&mut app, make_key(KeyCode::Char('g')));
        assert_eq!(app.tree_state.selected_index, 0);
    }

    #[test]
    fn key_shift_g_jumps_to_last() {
        let (_dir, mut app) = setup_app();
        handle_key(&mut app, make_key(KeyCode::Char('G')));
        assert_eq!(
            app.tree_state.selected_index,
            app.tree_state.flat_items.len() - 1
        );
    }

    #[test]
    fn key_enter_expands_directory() {
        let (_dir, mut app) = setup_app();
        handle_key(&mut app, make_key(KeyCode::Char('j')));
        assert_eq!(app.tree_state.flat_items[1].name, "alpha");
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert!(app.tree_state.flat_items[1].is_expanded);
    }

    #[test]
    fn key_release_event_is_ignored() {
        let (_dir, mut app) = setup_app();
        assert_eq!(app.tree_state.selected_index, 0);

        handle_key(&mut app, make_key(KeyCode::Char('j')));
        assert_eq!(app.tree_state.selected_index, 1);

        let mut release_j = make_key(KeyCode::Char('j'));
        release_j.kind = KeyEventKind::Release;
        handle_key(&mut app, release_j);

        // Selection should not move again on key release.
        assert_eq!(app.tree_state.selected_index, 1);
    }

    #[test]
    fn key_backspace_collapses_directory() {
        let (_dir, mut app) = setup_app();
        handle_key(&mut app, make_key(KeyCode::Char('j')));
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert!(app.tree_state.flat_items[1].is_expanded);
        handle_key(&mut app, make_key(KeyCode::Backspace));
        assert!(!app.tree_state.flat_items[1].is_expanded);
    }

    #[test]
    fn key_dot_toggles_hidden() {
        let (_dir, mut app) = setup_app();
        let before = app.tree_state.flat_items.len();
        handle_key(&mut app, make_key(KeyCode::Char('.')));
        assert!(app.tree_state.flat_items.len() > before);
    }

    #[test]
    fn key_q_quits() {
        let (_dir, mut app) = setup_app();
        handle_key(&mut app, make_key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn key_ctrl_c_quits() {
        let (_dir, mut app) = setup_app();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(app.should_quit);
    }

    // === Dialog opener tests ===

    #[test]
    fn key_a_opens_create_file_dialog() {
        let (_dir, mut app) = setup_app();
        handle_key(&mut app, make_key(KeyCode::Char('a')));
        assert!(matches!(app.mode, AppMode::Dialog(DialogKind::CreateFile)));
    }

    #[test]
    fn key_shift_a_opens_create_dir_dialog() {
        let (_dir, mut app) = setup_app();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('A'), KeyModifiers::SHIFT),
        );
        assert!(matches!(
            app.mode,
            AppMode::Dialog(DialogKind::CreateDirectory)
        ));
    }

    #[test]
    fn key_r_opens_rename_dialog() {
        let (_dir, mut app) = setup_app();
        // Select a file
        app.tree_state.selected_index = 3; // file_a.txt
        handle_key(&mut app, make_key(KeyCode::Char('r')));
        assert!(matches!(
            app.mode,
            AppMode::Dialog(DialogKind::Rename { .. })
        ));
        assert_eq!(app.dialog_state.input, "file_a.txt");
    }

    #[test]
    fn key_d_opens_delete_dialog() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 3; // file_a.txt
        handle_key(&mut app, make_key(KeyCode::Char('d')));
        assert!(matches!(
            app.mode,
            AppMode::Dialog(DialogKind::DeleteConfirm { .. })
        ));
    }

    #[test]
    fn key_d_on_root_is_noop() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 0; // root
        handle_key(&mut app, make_key(KeyCode::Char('d')));
        assert!(matches!(app.mode, AppMode::Normal));
    }

    // === Dialog input tests ===

    #[test]
    fn dialog_esc_closes() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn dialog_typing_inputs_chars() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        handle_key(&mut app, make_key(KeyCode::Char('t')));
        handle_key(&mut app, make_key(KeyCode::Char('e')));
        handle_key(&mut app, make_key(KeyCode::Char('s')));
        handle_key(&mut app, make_key(KeyCode::Char('t')));
        assert_eq!(app.dialog_state.input, "test");
    }

    #[test]
    fn dialog_backspace_deletes() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        handle_key(&mut app, make_key(KeyCode::Char('a')));
        handle_key(&mut app, make_key(KeyCode::Char('b')));
        handle_key(&mut app, make_key(KeyCode::Backspace));
        assert_eq!(app.dialog_state.input, "a");
    }

    #[test]
    fn save_confirm_yes_exits_edit_mode_on_success() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("editable.txt");
        fs::write(&file, "old").unwrap();

        let mut editor = crate::editor::EditorState::new("new", file.clone());
        editor.modified = true;
        app.editor_state = Some(editor);
        app.mode = AppMode::Dialog(DialogKind::SaveConfirm);

        handle_key(&mut app, make_key(KeyCode::Char('y')));

        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.editor_state.is_none());
        assert_eq!(fs::read_to_string(file).unwrap(), "new");
    }

    #[test]
    fn save_confirm_yes_stays_in_dialog_on_save_error() {
        let (dir, mut app) = setup_app();
        let invalid_path = dir.path().join("missing").join("editable.txt");

        let mut editor = crate::editor::EditorState::new("new", invalid_path);
        editor.modified = true;
        app.editor_state = Some(editor);
        app.mode = AppMode::Dialog(DialogKind::SaveConfirm);

        handle_key(&mut app, make_key(KeyCode::Char('y')));

        assert!(matches!(app.mode, AppMode::Dialog(DialogKind::SaveConfirm)));
        assert!(app.editor_state.is_some());
        assert!(app.status_message.is_some());
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(msg.contains("Save failed"));
    }

    #[test]
    fn editor_find_query_handles_multibyte_input_and_backspace() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("unicode_find.txt");
        fs::write(&file, "abc").unwrap();

        app.editor_state = Some(crate::editor::EditorState::new("abc", file));
        app.mode = AppMode::Edit;
        app.editor_state.as_mut().unwrap().open_find();

        handle_key(&mut app, make_key(KeyCode::Char('é')));
        {
            let editor = app.editor_state.as_ref().unwrap();
            assert_eq!(editor.find_state.query, "é");
            assert_eq!(editor.find_state.query_cursor, 'é'.len_utf8());
        }

        handle_key(&mut app, make_key(KeyCode::Backspace));
        let editor = app.editor_state.as_ref().unwrap();
        assert_eq!(editor.find_state.query, "");
        assert_eq!(editor.find_state.query_cursor, 0);
    }

    #[test]
    fn editor_find_replace_field_handles_multibyte_input_and_backspace() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("unicode_replace.txt");
        fs::write(&file, "abc").unwrap();

        app.editor_state = Some(crate::editor::EditorState::new("abc", file));
        app.mode = AppMode::Edit;
        {
            let editor = app.editor_state.as_mut().unwrap();
            editor.open_find_replace();
            editor.find_state.in_replace_field = true;
        }

        handle_key(&mut app, make_key(KeyCode::Char('한')));
        {
            let editor = app.editor_state.as_ref().unwrap();
            assert_eq!(editor.find_state.replacement, "한");
            assert_eq!(editor.find_state.replacement_cursor, '한'.len_utf8());
        }

        handle_key(&mut app, make_key(KeyCode::Backspace));
        let editor = app.editor_state.as_ref().unwrap();
        assert_eq!(editor.find_state.replacement, "");
        assert_eq!(editor.find_state.replacement_cursor, 0);
    }

    // === Integration tests: actual file operations ===

    #[test]
    fn create_file_via_dialog() {
        let (dir, mut app) = setup_app();
        // Open create file dialog
        handle_key(&mut app, make_key(KeyCode::Char('a')));
        // Type filename
        for c in "new_file.txt".chars() {
            handle_key(&mut app, make_key(KeyCode::Char(c)));
        }
        // Confirm
        handle_key(&mut app, make_key(KeyCode::Enter));
        // Verify file was created
        assert!(dir.path().join("new_file.txt").exists());
        assert!(matches!(app.mode, AppMode::Normal));
        assert!(app.status_message.is_some());
    }

    #[test]
    fn create_dir_via_dialog() {
        let (dir, mut app) = setup_app();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('A'), KeyModifiers::SHIFT),
        );
        for c in "new_dir".chars() {
            handle_key(&mut app, make_key(KeyCode::Char(c)));
        }
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert!(dir.path().join("new_dir").exists());
        assert!(dir.path().join("new_dir").is_dir());
    }

    #[test]
    fn rename_file_via_dialog() {
        let (dir, mut app) = setup_app();
        // Select file_a.txt (index 3)
        app.tree_state.selected_index = 3;
        handle_key(&mut app, make_key(KeyCode::Char('r')));
        // Clear existing name and type new one
        for _ in 0..app.dialog_state.input.len() {
            handle_key(&mut app, make_key(KeyCode::Backspace));
        }
        for c in "renamed.txt".chars() {
            handle_key(&mut app, make_key(KeyCode::Char(c)));
        }
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert!(!dir.path().join("file_a.txt").exists());
        assert!(dir.path().join("renamed.txt").exists());
    }

    #[test]
    fn delete_file_via_dialog() {
        let (dir, mut app) = setup_app();
        // Select file_a.txt (index 3)
        app.tree_state.selected_index = 3;
        handle_key(&mut app, make_key(KeyCode::Char('d')));
        // Confirm delete
        handle_key(&mut app, make_key(KeyCode::Char('y')));
        assert!(!dir.path().join("file_a.txt").exists());
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn delete_cancel_preserves_file() {
        let (dir, mut app) = setup_app();
        app.tree_state.selected_index = 3;
        handle_key(&mut app, make_key(KeyCode::Char('d')));
        handle_key(&mut app, make_key(KeyCode::Char('n')));
        assert!(dir.path().join("file_a.txt").exists());
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn normal_keys_ignored_in_dialog() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        let idx = app.tree_state.selected_index;
        // 'j' should type 'j', not navigate
        handle_key(&mut app, make_key(KeyCode::Char('j')));
        assert_eq!(app.dialog_state.input, "j");
        assert_eq!(app.tree_state.selected_index, idx);
    }

    #[test]
    fn error_dialog_dismiss_on_enter() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::Error {
            message: "test error".to_string(),
        });
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn error_dialog_dismiss_on_esc() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::Error {
            message: "test error".to_string(),
        });
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn tree_refreshes_after_create() {
        let (_dir, mut app) = setup_app();
        let before_count = app.tree_state.flat_items.len();
        handle_key(&mut app, make_key(KeyCode::Char('a')));
        for c in "brand_new.txt".chars() {
            handle_key(&mut app, make_key(KeyCode::Char(c)));
        }
        handle_key(&mut app, make_key(KeyCode::Enter));
        // Tree should have one more item
        assert_eq!(app.tree_state.flat_items.len(), before_count + 1);
    }

    // === Focus management tests ===

    #[test]
    fn tab_toggles_focus() {
        let (_dir, mut app) = setup_app();
        assert_eq!(app.focused_panel, FocusedPanel::Tree);
        handle_key(&mut app, make_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Preview);
        handle_key(&mut app, make_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Tree);
    }

    #[test]
    fn q_quits_from_preview_focus() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        handle_key(&mut app, make_key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_quits_from_preview_focus() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_with_selection_in_preview_triggers_copy() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        app.preview_state.content_lines = vec![ratatui::text::Line::raw("hello preview")];
        app.preview_state.total_lines = 1;
        app.preview_selection
            .set_anchor(crate::terminal::TerminalCoord { line: 0, col: 0 });
        app.preview_selection
            .set_endpoint(crate::terminal::TerminalCoord { line: 0, col: 4 });

        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );

        assert!(!app.should_quit);
        assert!(app.status_message.is_some());
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(
            msg.contains("Copying selection"),
            "expected 'Copying selection' but got: {}",
            msg
        );
    }

    #[test]
    fn preview_j_scrolls_down() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        app.preview_state.total_lines = 100;
        handle_key(&mut app, make_key(KeyCode::Char('j')));
        assert_eq!(app.preview_state.scroll_offset, 1);
    }

    #[test]
    fn preview_k_scrolls_up() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        app.preview_state.total_lines = 100;
        app.preview_state.scroll_offset = 5;
        handle_key(&mut app, make_key(KeyCode::Char('k')));
        assert_eq!(app.preview_state.scroll_offset, 4);
    }

    #[test]
    fn preview_g_jumps_top() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        app.preview_state.total_lines = 100;
        app.preview_state.scroll_offset = 50;
        handle_key(&mut app, make_key(KeyCode::Char('g')));
        assert_eq!(app.preview_state.scroll_offset, 0);
    }

    #[test]
    fn preview_shift_g_jumps_bottom() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        app.preview_state.total_lines = 100;
        handle_key(&mut app, make_key(KeyCode::Char('G')));
        assert_eq!(app.preview_state.scroll_offset, 99);
    }

    #[test]
    fn preview_j_does_not_navigate_tree() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        let idx = app.tree_state.selected_index;
        handle_key(&mut app, make_key(KeyCode::Char('j')));
        assert_eq!(app.tree_state.selected_index, idx);
    }

    // === Multi-select tests ===

    #[test]
    fn space_toggles_multi_select() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 1;
        handle_key(&mut app, make_key(KeyCode::Char(' ')));
        assert!(app.tree_state.multi_selected.contains(&1));
        handle_key(&mut app, make_key(KeyCode::Char(' ')));
        assert!(!app.tree_state.multi_selected.contains(&1));
    }

    #[test]
    fn esc_clears_multi_select() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 1;
        handle_key(&mut app, make_key(KeyCode::Char(' ')));
        app.tree_state.selected_index = 2;
        handle_key(&mut app, make_key(KeyCode::Char(' ')));
        assert_eq!(app.tree_state.multi_selected.len(), 2);
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert!(app.tree_state.multi_selected.is_empty());
    }

    #[test]
    fn navigation_preserves_multi_select() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 1;
        handle_key(&mut app, make_key(KeyCode::Char(' ')));
        // Navigate down
        handle_key(&mut app, make_key(KeyCode::Char('j')));
        // Selection should persist
        assert!(app.tree_state.multi_selected.contains(&1));
    }

    // === Clipboard tests ===

    #[test]
    fn y_copies_focused_item_to_clipboard() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 3; // file_a.txt
        handle_key(&mut app, make_key(KeyCode::Char('y')));
        assert_eq!(app.clipboard.len(), 1);
        assert_eq!(
            app.clipboard.operation,
            Some(crate::fs::clipboard::ClipboardOp::Copy)
        );
    }

    #[test]
    fn x_cuts_focused_item_to_clipboard() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 3;
        handle_key(&mut app, make_key(KeyCode::Char('x')));
        assert_eq!(app.clipboard.len(), 1);
        assert_eq!(
            app.clipboard.operation,
            Some(crate::fs::clipboard::ClipboardOp::Cut)
        );
    }

    #[test]
    fn y_copies_multi_selected_items() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 1;
        handle_key(&mut app, make_key(KeyCode::Char(' ')));
        app.tree_state.selected_index = 3;
        handle_key(&mut app, make_key(KeyCode::Char(' ')));
        handle_key(&mut app, make_key(KeyCode::Char('y')));
        assert_eq!(app.clipboard.len(), 2);
        assert_eq!(
            app.clipboard.operation,
            Some(crate::fs::clipboard::ClipboardOp::Copy)
        );
    }

    #[test]
    fn copy_sets_status_message() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 3;
        handle_key(&mut app, make_key(KeyCode::Char('y')));
        assert!(app.status_message.is_some());
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(msg.contains("copied"));
    }

    #[test]
    fn cut_sets_status_message() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 3;
        handle_key(&mut app, make_key(KeyCode::Char('x')));
        assert!(app.status_message.is_some());
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(msg.contains("cut"));
    }

    // === Paste tests ===

    #[tokio::test]
    async fn paste_copy_creates_duplicate() {
        let (dir, mut app) = setup_app();
        let (tx, mut rx) = mpsc::unbounded_channel::<Event>();
        // Copy file_a.txt (index 3)
        app.tree_state.selected_index = 3;
        app.copy_to_clipboard();
        // Navigate to beta dir (index 2) and paste
        app.tree_state.selected_index = 2;
        app.expand_selected();
        app.paste_clipboard_async(tx);
        // Wait for completion
        loop {
            if let Some(evt) = rx.recv().await {
                if let Event::OperationComplete(result) = evt {
                    app.handle_operation_complete(result);
                    break;
                }
            }
        }
        assert!(dir.path().join("beta").join("file_a.txt").exists());
        // Original still exists
        assert!(dir.path().join("file_a.txt").exists());
    }

    #[tokio::test]
    async fn paste_cut_moves_file() {
        let (dir, mut app) = setup_app();
        let (tx, mut rx) = mpsc::unbounded_channel::<Event>();
        // Cut file_a.txt (index 3)
        app.tree_state.selected_index = 3;
        app.cut_to_clipboard();
        // Navigate to beta dir
        app.tree_state.selected_index = 2;
        app.expand_selected();
        app.paste_clipboard_async(tx);
        loop {
            if let Some(evt) = rx.recv().await {
                if let Event::OperationComplete(result) = evt {
                    app.handle_operation_complete(result);
                    break;
                }
            }
        }
        assert!(dir.path().join("beta").join("file_a.txt").exists());
        // Original removed
        assert!(!dir.path().join("file_a.txt").exists());
        // Clipboard should be cleared after cut-paste
        assert!(app.clipboard.is_empty());
    }

    #[test]
    fn paste_empty_clipboard_shows_message() {
        let (_dir, mut app) = setup_app();
        handle_key(&mut app, make_key(KeyCode::Char('p')));
        assert!(app.status_message.is_some());
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(msg.contains("empty"));
    }

    #[tokio::test]
    async fn paste_copy_preserves_clipboard() {
        let (dir, mut app) = setup_app();
        let (tx, mut rx) = mpsc::unbounded_channel::<Event>();
        app.tree_state.selected_index = 3;
        app.copy_to_clipboard();
        // Paste into beta
        app.tree_state.selected_index = 2;
        app.expand_selected();
        app.paste_clipboard_async(tx);
        loop {
            if let Some(evt) = rx.recv().await {
                if let Event::OperationComplete(result) = evt {
                    app.handle_operation_complete(result);
                    break;
                }
            }
        }
        assert!(dir.path().join("beta").join("file_a.txt").exists());
        // Clipboard still populated (copy doesn't clear it)
        assert!(!app.clipboard.is_empty());
    }

    // === Undo tests ===

    #[test]
    fn undo_rename() {
        let (dir, mut app) = setup_app();
        // Rename file_a.txt -> renamed.txt
        app.tree_state.selected_index = 3;
        handle_key(&mut app, make_key(KeyCode::Char('r')));
        // Clear and type new name
        for _ in 0..app.dialog_state.input.len() {
            handle_key(&mut app, make_key(KeyCode::Backspace));
        }
        for c in "renamed.txt".chars() {
            handle_key(&mut app, make_key(KeyCode::Char(c)));
        }
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert!(dir.path().join("renamed.txt").exists());
        assert!(!dir.path().join("file_a.txt").exists());
        // Undo
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('z'), KeyModifiers::CONTROL),
        );
        assert!(dir.path().join("file_a.txt").exists());
        assert!(!dir.path().join("renamed.txt").exists());
    }

    #[tokio::test]
    async fn undo_copy_paste() {
        let (dir, mut app) = setup_app();
        let (tx, mut rx) = mpsc::unbounded_channel::<Event>();
        app.tree_state.selected_index = 3;
        app.copy_to_clipboard();
        app.tree_state.selected_index = 2;
        app.expand_selected();
        app.paste_clipboard_async(tx);
        loop {
            if let Some(evt) = rx.recv().await {
                if let Event::OperationComplete(result) = evt {
                    app.handle_operation_complete(result);
                    break;
                }
            }
        }
        assert!(dir.path().join("beta").join("file_a.txt").exists());
        // Undo should delete the copy
        app.undo();
        assert!(!dir.path().join("beta").join("file_a.txt").exists());
        // Original still exists
        assert!(dir.path().join("file_a.txt").exists());
    }

    #[test]
    fn undo_nothing_shows_message() {
        let (_dir, mut app) = setup_app();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('z'), KeyModifiers::CONTROL),
        );
        assert!(app.status_message.is_some());
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(msg.contains("Nothing to undo"));
    }

    #[test]
    fn undo_only_works_once() {
        let (dir, mut app) = setup_app();
        // Rename
        app.tree_state.selected_index = 3;
        handle_key(&mut app, make_key(KeyCode::Char('r')));
        for _ in 0..app.dialog_state.input.len() {
            handle_key(&mut app, make_key(KeyCode::Backspace));
        }
        for c in "renamed.txt".chars() {
            handle_key(&mut app, make_key(KeyCode::Char(c)));
        }
        handle_key(&mut app, make_key(KeyCode::Enter));
        // Undo once
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('z'), KeyModifiers::CONTROL),
        );
        assert!(dir.path().join("file_a.txt").exists());
        // Second undo should say "nothing"
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('z'), KeyModifiers::CONTROL),
        );
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(msg.contains("Nothing to undo"));
    }

    // === Search (Ctrl+P) handler tests ===

    #[test]
    fn ctrl_p_opens_search() {
        let (_dir, mut app) = setup_app();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.mode, AppMode::Search);
    }

    #[test]
    fn search_esc_closes() {
        let (_dir, mut app) = setup_app();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert_eq!(app.mode, AppMode::Normal);
    }

    #[test]
    fn search_typing_updates_query() {
        let (_dir, mut app) = setup_app();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        handle_key(&mut app, make_key(KeyCode::Char('f')));
        handle_key(&mut app, make_key(KeyCode::Char('i')));
        assert_eq!(app.search_state.query, "fi");
    }

    #[test]
    fn search_enter_navigates() {
        let (dir, mut app) = setup_app();
        // Create a file for search
        std::fs::write(dir.path().join("file_a.txt"), "hello").unwrap();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        handle_key(&mut app, make_key(KeyCode::Char('f')));
        handle_key(&mut app, make_key(KeyCode::Char('i')));
        handle_key(&mut app, make_key(KeyCode::Char('l')));
        handle_key(&mut app, make_key(KeyCode::Char('e')));
        assert!(!app.search_state.results.is_empty());
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert_eq!(app.mode, AppMode::SearchAction);
        // Press Enter again to navigate
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert_eq!(app.mode, AppMode::Normal);
    }

    #[test]
    fn search_action_y_copy_path_is_non_blocking() {
        let (_dir, mut app) = setup_app();

        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        for c in "file".chars() {
            handle_key(&mut app, make_key(KeyCode::Char(c)));
        }
        assert!(!app.search_state.results.is_empty());

        handle_key(&mut app, make_key(KeyCode::Enter));
        assert_eq!(app.mode, AppMode::SearchAction);

        handle_key(&mut app, make_key(KeyCode::Char('y')));
        assert_eq!(app.mode, AppMode::Normal);

        let (msg, _) = app
            .status_message
            .as_ref()
            .expect("status message should exist");
        assert!(msg.contains("Copying path to clipboard"));
    }

    #[test]
    fn search_arrow_navigates_results() {
        let (dir, mut app) = setup_app();
        std::fs::write(dir.path().join("file_a.txt"), "a").unwrap();
        std::fs::write(dir.path().join("file_b.rs"), "b").unwrap();
        // Reload tree so newly created files appear in loaded nodes
        app.tree_state.reload_dir(dir.path());
        app.invalidate_search_cache();

        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        for c in "file".chars() {
            handle_key(&mut app, make_key(KeyCode::Char(c)));
        }
        assert!(app.search_state.results.len() >= 2);
        handle_key(&mut app, make_key(KeyCode::Down));
        assert_eq!(app.search_state.selected_index, 1);
        handle_key(&mut app, make_key(KeyCode::Up));
        assert_eq!(app.search_state.selected_index, 0);
    }

    #[test]
    fn search_backspace_removes_char() {
        let (_dir, mut app) = setup_app();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        handle_key(&mut app, make_key(KeyCode::Char('a')));
        handle_key(&mut app, make_key(KeyCode::Char('b')));
        handle_key(&mut app, make_key(KeyCode::Backspace));
        assert_eq!(app.search_state.query, "a");
    }

    // === Filter (/) handler tests ===

    #[test]
    fn slash_opens_filter() {
        let (_dir, mut app) = setup_app();
        handle_key(&mut app, make_key(KeyCode::Char('/')));
        assert_eq!(app.mode, AppMode::Filter);
    }

    #[test]
    fn filter_esc_clears() {
        let (_dir, mut app) = setup_app();
        handle_key(&mut app, make_key(KeyCode::Char('/')));
        handle_key(&mut app, make_key(KeyCode::Char('f')));
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert_eq!(app.mode, AppMode::Normal);
        assert!(!app.tree_state.is_filtering);
    }

    #[test]
    fn filter_enter_accepts() {
        let (_dir, mut app) = setup_app();
        handle_key(&mut app, make_key(KeyCode::Char('/')));
        handle_key(&mut app, make_key(KeyCode::Char('f')));
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert_eq!(app.mode, AppMode::Normal);
        // Filter view should persist
        assert!(app.tree_state.is_filtering);
    }

    #[test]
    fn filter_typing_filters_tree() {
        let (_dir, mut app) = setup_app();
        let total = app.tree_state.flat_items.len();
        handle_key(&mut app, make_key(KeyCode::Char('/')));
        handle_key(&mut app, make_key(KeyCode::Char('a')));
        handle_key(&mut app, make_key(KeyCode::Char('l')));
        handle_key(&mut app, make_key(KeyCode::Char('p')));
        assert!(app.tree_state.flat_items.len() <= total);
    }

    #[test]
    fn filter_backspace_updates() {
        let (_dir, mut app) = setup_app();
        handle_key(&mut app, make_key(KeyCode::Char('/')));
        handle_key(&mut app, make_key(KeyCode::Char('z')));
        handle_key(&mut app, make_key(KeyCode::Backspace));
        // Filter cleared, back to full tree
        assert!(!app.tree_state.is_filtering);
    }

    // === Integration tests ===

    #[test]
    fn search_then_navigate_end_to_end() {
        let (dir, mut app) = setup_app();
        // Create nested file
        fs::create_dir_all(dir.path().join("alpha").join("nested")).unwrap();
        File::create(dir.path().join("alpha").join("nested").join("deep.txt")).unwrap();
        app.invalidate_search_cache();

        // Open search
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.mode, AppMode::Search);

        // Type query
        for c in "deep".chars() {
            handle_key(&mut app, make_key(KeyCode::Char(c)));
        }
        assert!(!app.search_state.results.is_empty());

        // Confirm -> goes to SearchAction
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert_eq!(app.mode, AppMode::SearchAction);

        // Navigate from action menu
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert_eq!(app.mode, AppMode::Normal);

        // Verify tree selection
        let selected = &app.tree_state.flat_items[app.tree_state.selected_index];
        assert_eq!(selected.name, "deep.txt");
    }

    #[test]
    fn filter_then_navigate_end_to_end() {
        let (_dir, mut app) = setup_app();
        let total = app.tree_state.flat_items.len();

        // Activate filter
        handle_key(&mut app, make_key(KeyCode::Char('/')));
        for c in "file".chars() {
            handle_key(&mut app, make_key(KeyCode::Char(c)));
        }
        assert!(app.tree_state.flat_items.len() <= total);
        assert!(app.tree_state.is_filtering);

        // Accept filter
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.tree_state.is_filtering);

        // Navigate in filtered view
        handle_key(&mut app, make_key(KeyCode::Char('j')));
    }

    #[test]
    fn search_cache_invalidated_after_create() {
        let (dir, mut app) = setup_app();
        // Directly set a cached path list to simulate a prior search
        app.search_state.cached_paths = Some(vec![dir.path().join("file_a.txt")]);
        assert!(app.search_state.cached_paths.is_some());

        // Create a file via dialog
        handle_key(&mut app, make_key(KeyCode::Char('a')));
        for c in "new_file.txt".chars() {
            handle_key(&mut app, make_key(KeyCode::Char(c)));
        }
        handle_key(&mut app, make_key(KeyCode::Enter));

        // Cache should be invalidated
        assert!(app.search_state.cached_paths.is_none());
        assert!(dir.path().join("new_file.txt").exists());
    }

    #[test]
    fn search_cache_invalidated_after_delete() {
        let (_dir, mut app) = setup_app();
        // Directly set a cached path list to simulate a prior search
        app.search_state.cached_paths = Some(vec![]);
        assert!(app.search_state.cached_paths.is_some());

        // Select file_a.txt (index 3) and delete
        app.tree_state.selected_index = 3;
        handle_key(&mut app, make_key(KeyCode::Char('d')));
        handle_key(&mut app, make_key(KeyCode::Char('y')));

        // Cache should be invalidated
        assert!(app.search_state.cached_paths.is_none());
    }

    #[test]
    fn search_special_characters_in_filename() {
        let (dir, mut app) = setup_app();
        // Create file with special characters
        File::create(dir.path().join("test (1).txt")).unwrap();
        // Reload tree so the new file appears in loaded nodes
        app.tree_state.reload_dir(dir.path());
        app.invalidate_search_cache();

        app.open_search();
        for c in "test (1)".chars() {
            app.search_input_char(c);
        }
        assert!(!app.search_state.results.is_empty());
    }

    #[test]
    fn ctrl_p_and_slash_work_from_preview_focus() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = crate::app::FocusedPanel::Preview;

        // Ctrl+P should work from preview panel (global key)
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.mode, AppMode::Search);
        handle_key(&mut app, make_key(KeyCode::Esc));

        // / should work from preview panel (global key)
        handle_key(&mut app, make_key(KeyCode::Char('/')));
        assert_eq!(app.mode, AppMode::Filter);
    }

    #[test]
    fn no_regression_tree_navigation() {
        let (_dir, mut app) = setup_app();
        // Basic navigation should still work
        handle_key(&mut app, make_key(KeyCode::Char('j')));
        assert_eq!(app.tree_state.selected_index, 1);
        handle_key(&mut app, make_key(KeyCode::Char('k')));
        assert_eq!(app.tree_state.selected_index, 0);
        handle_key(&mut app, make_key(KeyCode::Char('G')));
        assert_eq!(
            app.tree_state.selected_index,
            app.tree_state.flat_items.len() - 1
        );
    }

    // === Watcher keybinding tests ===

    #[test]
    fn ctrl_r_toggles_watcher() {
        let (_dir, mut app) = setup_app();
        assert!(app.watcher_active);
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('r'), KeyModifiers::CONTROL),
        );
        assert!(!app.watcher_active);
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('r'), KeyModifiers::CONTROL),
        );
        assert!(app.watcher_active);
    }

    #[test]
    fn f5_triggers_full_refresh() {
        let (dir, mut app) = setup_app();
        let before = app.tree_state.flat_items.len();
        // Create a file that won't show until refresh
        File::create(dir.path().join("f5_test.txt")).unwrap();
        handle_key(&mut app, make_key(KeyCode::F(5)));
        assert!(app.tree_state.flat_items.len() > before);
        assert!(app.status_message.is_some());
    }

    #[test]
    fn ctrl_r_works_from_preview_panel() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        assert!(app.watcher_active);
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('r'), KeyModifiers::CONTROL),
        );
        assert!(!app.watcher_active);
    }

    #[test]
    fn f5_works_from_preview_panel() {
        let (dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        File::create(dir.path().join("f5_preview.txt")).unwrap();
        handle_key(&mut app, make_key(KeyCode::F(5)));
        let names: Vec<&str> = app
            .tree_state
            .flat_items
            .iter()
            .map(|i| i.name.as_str())
            .collect();
        assert!(names.contains(&"f5_preview.txt"));
    }

    // === Help mode tests ===

    #[test]
    fn question_mark_opens_help() {
        let (_dir, mut app) = setup_app();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('?'), KeyModifiers::SHIFT),
        );
        assert_eq!(app.mode, AppMode::Help);
    }

    #[test]
    fn question_mark_toggles_help() {
        let (_dir, mut app) = setup_app();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('?'), KeyModifiers::SHIFT),
        );
        assert_eq!(app.mode, AppMode::Help);
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('?'), KeyModifiers::SHIFT),
        );
        assert_eq!(app.mode, AppMode::Normal);
    }

    #[test]
    fn esc_closes_help() {
        let (_dir, mut app) = setup_app();
        app.mode = AppMode::Help;
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert_eq!(app.mode, AppMode::Normal);
    }

    #[test]
    fn help_scroll_down_and_up() {
        let (_dir, mut app) = setup_app();
        app.mode = AppMode::Help;
        handle_key(&mut app, make_key(KeyCode::Char('j')));
        assert_eq!(app.help_state.scroll_offset, 1);
        handle_key(&mut app, make_key(KeyCode::Char('k')));
        assert_eq!(app.help_state.scroll_offset, 0);
    }

    #[test]
    fn help_keys_do_not_navigate_tree() {
        let (_dir, mut app) = setup_app();
        app.mode = AppMode::Help;
        let idx = app.tree_state.selected_index;
        handle_key(&mut app, make_key(KeyCode::Char('j')));
        handle_key(&mut app, make_key(KeyCode::Char('k')));
        assert_eq!(app.tree_state.selected_index, idx);
    }

    // === Mouse handler tests ===

    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    fn make_mouse_click(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn make_mouse_scroll_down(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn make_mouse_scroll_up(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_click_tree_selects_item() {
        let (_dir, mut app) = setup_app();
        // Simulate tree area: starts at (0,0) with width 40, height 20
        app.tree_area = ratatui::layout::Rect::new(0, 0, 40, 20);
        app.preview_area = ratatui::layout::Rect::new(40, 0, 60, 20);
        assert_eq!(app.tree_state.selected_index, 0);

        // Click on row 2 (inner row 1 = index 1, accounting for top border)
        let tx = make_event_tx();
        handle_mouse_event(&mut app, make_mouse_click(10, 2), &tx);
        assert_eq!(app.tree_state.selected_index, 1);
        assert_eq!(app.focused_panel, FocusedPanel::Tree);
    }

    #[test]
    fn mouse_click_preview_switches_focus() {
        let (_dir, mut app) = setup_app();
        app.tree_area = ratatui::layout::Rect::new(0, 0, 40, 20);
        app.preview_area = ratatui::layout::Rect::new(40, 0, 60, 20);
        assert_eq!(app.focused_panel, FocusedPanel::Tree);

        let tx = make_event_tx();
        handle_mouse_event(&mut app, make_mouse_click(50, 5), &tx);
        assert_eq!(app.focused_panel, FocusedPanel::Preview);
    }

    #[test]
    fn mouse_scroll_tree_navigates() {
        let (_dir, mut app) = setup_app();
        app.tree_area = ratatui::layout::Rect::new(0, 0, 40, 20);
        app.preview_area = ratatui::layout::Rect::new(40, 0, 60, 20);
        // Set tree_visible_height so max_scroll calculation works
        app.tree_visible_height = 18; // 20 - 2 border

        // Mouse scroll now moves viewport (scroll_offset), not selection
        let tx = make_event_tx();
        let initial_offset = app.tree_state.scroll_offset;
        handle_mouse_event(&mut app, make_mouse_scroll_down(10, 5), &tx);
        // scroll_offset should increase (viewport scrolls down)
        // Note: may be clamped if total items < visible_height
        let total = app.tree_state.flat_items.len();
        if total > app.tree_visible_height {
            assert!(app.tree_state.scroll_offset > initial_offset);
        }
        // selection should NOT change from mouse scroll
        assert_eq!(app.tree_state.selected_index, 0);

        handle_mouse_event(&mut app, make_mouse_scroll_up(10, 5), &tx);
        // Should scroll back to 0
        assert_eq!(app.tree_state.scroll_offset, 0);
        assert_eq!(app.tree_state.selected_index, 0);
    }

    #[test]
    fn mouse_scroll_preview_scrolls() {
        let (_dir, mut app) = setup_app();
        app.tree_area = ratatui::layout::Rect::new(0, 0, 40, 20);
        app.preview_area = ratatui::layout::Rect::new(40, 0, 60, 20);
        app.preview_state.total_lines = 100;

        let tx = make_event_tx();
        handle_mouse_event(&mut app, make_mouse_scroll_down(50, 5), &tx);
        assert_eq!(app.preview_state.scroll_offset, 1);
        assert_eq!(app.focused_panel, FocusedPanel::Preview);
    }

    #[test]
    fn mouse_drag_in_preview_updates_selection() {
        let (_dir, mut app) = setup_app();
        app.preview_area = ratatui::layout::Rect::new(40, 0, 60, 20);
        app.preview_state.content_lines = vec![
            ratatui::text::Line::raw("line 1"),
            ratatui::text::Line::raw("line 2"),
        ];
        app.preview_state.total_lines = 2;

        let tx = make_event_tx();
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 42,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, down, &tx);

        let drag = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 47,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, drag, &tx);

        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 47,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, up, &tx);

        assert!(app.preview_selection.is_active());
    }

    #[test]
    fn mouse_ignored_in_dialog_mode() {
        let (_dir, mut app) = setup_app();
        app.tree_area = ratatui::layout::Rect::new(0, 0, 40, 20);
        app.mode = AppMode::Dialog(DialogKind::CreateFile);
        let idx = app.tree_state.selected_index;

        let tx = make_event_tx();
        handle_mouse_event(&mut app, make_mouse_click(10, 2), &tx);
        assert_eq!(app.tree_state.selected_index, idx);
    }

    // === Directional focus keybinding tests ===

    #[test]
    fn ctrl_left_moves_focus_left() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Left, KeyModifiers::CONTROL),
        );
        assert_eq!(app.focused_panel, FocusedPanel::Tree);
    }

    #[test]
    fn ctrl_right_moves_focus_right() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Tree;
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Right, KeyModifiers::CONTROL),
        );
        assert_eq!(app.focused_panel, FocusedPanel::Preview);
    }

    #[test]
    fn ctrl_up_moves_focus_up_from_terminal() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.focused_panel = FocusedPanel::Terminal;
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Up, KeyModifiers::CONTROL),
        );
        assert_eq!(app.focused_panel, FocusedPanel::Tree);
    }

    #[test]
    fn ctrl_down_moves_focus_down_to_terminal() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.focused_panel = FocusedPanel::Tree;
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Down, KeyModifiers::CONTROL),
        );
        assert_eq!(app.focused_panel, FocusedPanel::Terminal);
    }

    #[test]
    fn ctrl_shift_up_resizes_terminal_smaller() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.terminal_state.height_percent = 30;
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        );
        assert_eq!(app.terminal_state.height_percent, 25);
    }

    #[test]
    fn ctrl_shift_down_resizes_terminal_larger() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.terminal_state.height_percent = 30;
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Down, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        );
        assert_eq!(app.terminal_state.height_percent, 35);
    }

    #[test]
    fn ctrl_arrow_intercepted_when_terminal_focused() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.focused_panel = FocusedPanel::Terminal;
        // Ctrl+Right should switch focus to Preview even when terminal is focused
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Right, KeyModifiers::CONTROL),
        );
        assert_eq!(app.focused_panel, FocusedPanel::Preview);
    }

    #[test]
    fn tab_still_cycles_focus() {
        let (_dir, mut app) = setup_app();
        assert_eq!(app.focused_panel, FocusedPanel::Tree);
        handle_key(&mut app, make_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Preview);
    }

    // === Terminal mouse selection tests ===

    #[test]
    fn terminal_click_sets_selection_anchor() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        // Set terminal area with border
        app.terminal_area = ratatui::layout::Rect::new(0, 20, 80, 10);

        let tx = make_event_tx();
        // Click inside terminal inner area (accounting for border)
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 22, // inner_y starts at 21 (20+1)
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, mouse, &tx);

        assert_eq!(app.focused_panel, FocusedPanel::Terminal);
        assert!(app.terminal_state.selection.anchor.is_some());
    }

    #[test]
    fn terminal_drag_updates_selection_endpoint() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.terminal_area = ratatui::layout::Rect::new(0, 20, 80, 10);

        let tx = make_event_tx();
        // Click to set anchor
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 22,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, down, &tx);

        // Drag to update endpoint
        let drag = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 20,
            row: 24,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, drag, &tx);

        let (start, end) = app.terminal_state.selection.normalized().unwrap();
        // Anchor and endpoint should differ
        assert!(start != end || start.col != end.col);
    }

    #[test]
    fn terminal_moved_updates_selection_endpoint_while_dragging() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.terminal_area = ratatui::layout::Rect::new(0, 20, 80, 10);

        let tx = make_event_tx();
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 22,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, down, &tx);

        // Some terminals emit Moved instead of Drag while left button is held.
        let moved = MouseEvent {
            kind: MouseEventKind::Moved,
            column: 20,
            row: 24,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, moved, &tx);

        let (start, end) = app.terminal_state.selection.normalized().unwrap();
        assert!(start != end || start.col != end.col);
    }

    #[test]
    fn terminal_moved_after_drag_end_does_not_change_selection() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.terminal_area = ratatui::layout::Rect::new(0, 20, 80, 10);

        let tx = make_event_tx();
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 22,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, down, &tx);

        let drag = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 10,
            row: 22,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, drag, &tx);

        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 10,
            row: 22,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, up, &tx);

        let before = app.terminal_state.selection.normalized();
        assert!(before.is_some());

        let moved = MouseEvent {
            kind: MouseEventKind::Moved,
            column: 40,
            row: 25,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, moved, &tx);

        assert_eq!(app.terminal_state.selection.normalized(), before);
    }

    #[test]
    fn terminal_click_without_drag_clears_selection() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.terminal_area = ratatui::layout::Rect::new(0, 20, 80, 10);

        let tx = make_event_tx();
        // Click down
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 22,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, down, &tx);
        // Selection should be set after down
        assert!(app.terminal_state.selection.is_active());

        // Release without drag (anchor == endpoint)
        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 5,
            row: 22,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, up, &tx);
        // Selection should be cleared (click without drag)
        assert!(!app.terminal_state.selection.is_active());
    }

    #[test]
    fn tree_click_clears_terminal_selection() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.terminal_area = ratatui::layout::Rect::new(0, 20, 80, 10);
        app.tree_area = ratatui::layout::Rect::new(0, 0, 40, 20);

        // Set up a fake selection
        app.terminal_state
            .selection
            .set_anchor(crate::terminal::TerminalCoord { line: 0, col: 0 });
        app.terminal_state
            .selection
            .set_endpoint(crate::terminal::TerminalCoord { line: 2, col: 5 });
        assert!(app.terminal_state.selection.is_active());

        let tx = make_event_tx();
        // Click on tree area
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, click, &tx);
        assert!(!app.terminal_state.selection.is_active());
    }

    #[test]
    fn esc_clears_terminal_selection_before_leaving() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.focused_panel = FocusedPanel::Terminal;

        // Set up selection
        app.terminal_state
            .selection
            .set_anchor(crate::terminal::TerminalCoord { line: 0, col: 0 });
        app.terminal_state
            .selection
            .set_endpoint(crate::terminal::TerminalCoord { line: 1, col: 5 });

        // First Esc should clear selection, keep terminal focus
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert!(!app.terminal_state.selection.is_active());
        assert_eq!(app.focused_panel, FocusedPanel::Terminal);

        // Second Esc should leave terminal
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert_eq!(app.focused_panel, FocusedPanel::Tree);
    }

    #[test]
    fn ctrl_shift_c_in_terminal_triggers_copy() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.focused_panel = FocusedPanel::Terminal;

        // No selection — should show hint (uppercase C variant)
        handle_key(
            &mut app,
            make_key_with_modifiers(
                KeyCode::Char('C'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert!(app.status_message.is_some());
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(msg.contains("No terminal text selected"));
    }

    #[test]
    fn ctrl_shift_c_lowercase_in_terminal_triggers_copy() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.focused_panel = FocusedPanel::Terminal;

        // Some terminals send lowercase 'c' with shift modifier
        handle_key(
            &mut app,
            make_key_with_modifiers(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert!(app.status_message.is_some());
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(msg.contains("No terminal text selected"));
    }

    #[test]
    fn ctrl_c_with_selection_in_terminal_triggers_copy() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.focused_panel = FocusedPanel::Terminal;
        app.status_message = None;

        app.terminal_state.emulator.process(b"Hello");
        let sb = app.terminal_state.emulator.scrollback_len();
        app.terminal_state
            .selection
            .set_anchor(crate::terminal::TerminalCoord { line: sb, col: 0 });
        app.terminal_state
            .selection
            .set_endpoint(crate::terminal::TerminalCoord { line: sb, col: 4 });

        // Some terminals emit Ctrl+Shift+C as lowercase 'c' with only CONTROL modifier.
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );

        assert!(app.status_message.is_some());
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(!msg.contains("No terminal text selected"));
    }

    #[test]
    fn ctrl_c_without_selection_in_terminal_is_not_copy() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.focused_panel = FocusedPanel::Terminal;
        app.status_message = None;

        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );

        assert!(app.status_message.is_none());
    }

    #[test]
    fn cmd_c_in_terminal_triggers_copy() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.focused_panel = FocusedPanel::Terminal;

        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('c'), KeyModifiers::SUPER),
        );

        assert!(app.status_message.is_some());
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(msg.contains("No terminal text selected"));
    }

    #[test]
    fn ctrl_insert_in_terminal_triggers_copy() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.focused_panel = FocusedPanel::Terminal;

        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Insert, KeyModifiers::CONTROL),
        );

        assert!(app.status_message.is_some());
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(msg.contains("No terminal text selected"));
    }

    #[test]
    fn terminal_right_click_copies_selection() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.terminal_area = ratatui::layout::Rect::new(0, 20, 80, 10);
        app.focused_panel = FocusedPanel::Terminal;

        app.terminal_state.emulator.process(b"Hello");
        let sb = app.terminal_state.emulator.scrollback_len();
        app.terminal_state
            .selection
            .set_anchor(crate::terminal::TerminalCoord { line: sb, col: 0 });
        app.terminal_state
            .selection
            .set_endpoint(crate::terminal::TerminalCoord { line: sb, col: 4 });
        app.status_message = None;

        let tx = make_event_tx();
        let right_click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 5,
            row: 22,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, right_click, &tx);

        assert!(app.status_message.is_some());
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(
            msg.contains("Copying selection"),
            "expected 'Copying selection' but got: {}",
            msg
        );
    }

    #[test]
    fn preview_right_click_copies_selection() {
        let (_dir, mut app) = setup_app();
        app.preview_area = ratatui::layout::Rect::new(40, 0, 60, 20);
        app.focused_panel = FocusedPanel::Preview;
        app.preview_state.content_lines = vec![ratatui::text::Line::raw("hello preview")];
        app.preview_state.total_lines = 1;
        app.preview_selection
            .set_anchor(crate::terminal::TerminalCoord { line: 0, col: 0 });
        app.preview_selection
            .set_endpoint(crate::terminal::TerminalCoord { line: 0, col: 4 });

        let tx = make_event_tx();
        let right_click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 45,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, right_click, &tx);

        assert!(app.status_message.is_some());
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(
            msg.contains("Copying selection"),
            "expected 'Copying selection' but got: {}",
            msg
        );
    }

    #[test]
    fn typing_in_terminal_clears_selection() {
        let (_dir, mut app) = setup_app();
        app.terminal_state.visible = true;
        app.focused_panel = FocusedPanel::Terminal;

        // Set up selection
        app.terminal_state
            .selection
            .set_anchor(crate::terminal::TerminalCoord { line: 0, col: 0 });
        app.terminal_state
            .selection
            .set_endpoint(crate::terminal::TerminalCoord { line: 1, col: 5 });
        assert!(app.terminal_state.selection.is_active());

        // Type a character — should clear selection
        handle_key(&mut app, make_key(KeyCode::Char('a')));
        assert!(!app.terminal_state.selection.is_active());
    }

    // === Double-click line selection tests ===

    #[test]
    fn last_preview_click_initializes_to_none() {
        let (_dir, app) = setup_app();
        assert!(app.last_preview_click.is_none());
    }

    /// Helper: create a MouseEvent (Down/Left) at (col, row).
    fn make_mouse_down_left(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row: row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// Helper: create a MouseEvent (Up/Left) at (col, row).
    fn make_mouse_up_left(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: col,
            row: row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// Set up an App with a fake preview area and content for mouse tests.
    fn setup_app_with_preview() -> (TempDir, App) {
        let (dir, mut app) = setup_app();
        // Set a preview area that encompasses clickable content.
        // preview_area starts at (20, 0) with width=40, height=10
        // Inner area is (21, 1) to (58, 8) — accounting for borders
        app.preview_area = ratatui::layout::Rect::new(20, 0, 40, 10);
        // Add some content lines to preview
        use ratatui::text::{Line, Span};
        app.preview_state.content_lines = vec![
            Line::from(Span::raw("Hello World")),      // 11 chars
            Line::from(Span::raw("Second Line Here")), // 16 chars
            Line::from(Span::raw("fn main() {}")),     // 12 chars
        ];
        app.preview_state.scroll_offset = 0;
        (dir, app)
    }

    #[test]
    fn double_click_within_timeout_selects_full_line() {
        let (_dir, mut app) = setup_app_with_preview();
        let tx = make_event_tx();

        // Click position inside preview inner area:
        // preview_area is (20, 0, 40, 10), inner starts at (21, 1)
        let click_col = 25;
        let click_row = 1; // inner_y=0 → line 0

        // First click
        handle_mouse_event(&mut app, make_mouse_down_left(click_col, click_row), &tx);
        // First click should store last_preview_click (not yet consumed)
        // and begin_drag (which sets anchor+endpoint at same point, dragging=true)
        assert!(app.preview_selection.dragging);

        // Simulate mouse up
        handle_mouse_event(&mut app, make_mouse_up_left(click_col, click_row), &tx);
        // After up at same spot, anchor==endpoint → selection gets cleared
        assert!(!app.preview_selection.is_active());

        // Second click at same position (within timeout — immediate)
        handle_mouse_event(&mut app, make_mouse_down_left(click_col, click_row), &tx);

        // Should have selected the full line
        assert!(app.preview_selection.is_active());
        let (start, end) = app.preview_selection.normalized().unwrap();
        assert_eq!(start.line, 0);
        assert_eq!(start.col, 0);
        assert_eq!(end.line, 0);
        assert_eq!(end.col, 11); // "Hello World" = 11 chars
                                 // last_preview_click should be consumed (None)
        assert!(app.last_preview_click.is_none());
    }

    #[test]
    fn clicks_beyond_timeout_treated_as_single_clicks() {
        let (_dir, mut app) = setup_app_with_preview();
        let tx = make_event_tx();

        let click_col = 25;
        let click_row = 1;

        // Simulate a first click that happened long ago by manually setting
        // last_preview_click to an old timestamp.
        app.last_preview_click = Some((
            std::time::Instant::now() - std::time::Duration::from_millis(1000),
            click_col,
            click_row,
        ));

        // Second click at same position but >500ms later
        handle_mouse_event(&mut app, make_mouse_down_left(click_col, click_row), &tx);

        // Should NOT be a double-click — should be a regular drag start
        assert!(app.preview_selection.dragging);
        // last_preview_click should be set to the new click
        assert!(app.last_preview_click.is_some());
    }

    #[test]
    fn double_click_at_different_positions_treated_as_single_clicks() {
        let (_dir, mut app) = setup_app_with_preview();
        let tx = make_event_tx();

        // First click at position A
        let col_a = 25;
        let row_a = 1;
        handle_mouse_event(&mut app, make_mouse_down_left(col_a, row_a), &tx);
        handle_mouse_event(&mut app, make_mouse_up_left(col_a, row_a), &tx);

        // Second click at different position B (within timeout but different coord)
        let col_b = 30;
        let row_b = 2;
        handle_mouse_event(&mut app, make_mouse_down_left(col_b, row_b), &tx);

        // Should NOT be a double-click — should be a regular drag start
        assert!(app.preview_selection.dragging);
        // last_preview_click should record position B
        let (_, stored_col, stored_row) = app.last_preview_click.unwrap();
        assert_eq!(stored_col, col_b);
        assert_eq!(stored_row, row_b);
    }

    #[test]
    fn double_click_selects_second_line() {
        let (_dir, mut app) = setup_app_with_preview();
        let tx = make_event_tx();

        // Click on row 2 → inner_y=1 → line 1 ("Second Line Here", 16 chars)
        let click_col = 25;
        let click_row = 2;

        // First click + release
        handle_mouse_event(&mut app, make_mouse_down_left(click_col, click_row), &tx);
        handle_mouse_event(&mut app, make_mouse_up_left(click_col, click_row), &tx);

        // Second click (double-click)
        handle_mouse_event(&mut app, make_mouse_down_left(click_col, click_row), &tx);

        assert!(app.preview_selection.is_active());
        let (start, end) = app.preview_selection.normalized().unwrap();
        assert_eq!(start.line, 1);
        assert_eq!(start.col, 0);
        assert_eq!(end.line, 1);
        assert_eq!(end.col, 16); // "Second Line Here" = 16 chars
    }

    // === Phase 2: Integration tests ===

    #[test]
    fn double_click_with_nonzero_scroll_offset() {
        let (dir, mut app) = setup_app();
        let tx = make_event_tx();

        // Set up preview area and content with many lines
        app.preview_area = ratatui::layout::Rect::new(20, 0, 40, 10);
        use ratatui::text::{Line, Span};
        app.preview_state.content_lines = (0..20)
            .map(|i| Line::from(Span::raw(format!("Line number {:02}", i))))
            .collect();

        // Scroll down so line 0 is no longer visible
        // inner_h = 10 - 2 = 8 visible lines
        // With scroll_offset = 5, visible lines are 5..12
        app.preview_state.scroll_offset = 5;

        // Click on row 1 (inner_y=0) → should map to line 5 (scroll_offset + 0)
        let click_col = 25;
        let click_row = 1;

        // First click + release
        handle_mouse_event(&mut app, make_mouse_down_left(click_col, click_row), &tx);
        handle_mouse_event(&mut app, make_mouse_up_left(click_col, click_row), &tx);
        // Second click (double-click)
        handle_mouse_event(&mut app, make_mouse_down_left(click_col, click_row), &tx);

        assert!(app.preview_selection.is_active());
        let (start, end) = app.preview_selection.normalized().unwrap();
        assert_eq!(start.line, 5); // scroll_offset + inner_row 0
        assert_eq!(start.col, 0);
        assert_eq!(end.line, 5);
        assert_eq!(end.col, 14); // "Line number 05" = 14 chars
    }

    #[test]
    fn double_click_selection_persists_across_scroll() {
        let (_dir, mut app) = setup_app_with_preview();
        let tx = make_event_tx();

        // Double-click to select line 0
        let click_col = 25;
        let click_row = 1;
        handle_mouse_event(&mut app, make_mouse_down_left(click_col, click_row), &tx);
        handle_mouse_event(&mut app, make_mouse_up_left(click_col, click_row), &tx);
        handle_mouse_event(&mut app, make_mouse_down_left(click_col, click_row), &tx);

        assert!(app.preview_selection.is_active());
        let (start, end) = app.preview_selection.normalized().unwrap();
        assert_eq!(start.line, 0);
        assert_eq!(end.line, 0);

        // Scroll the preview panel (simulate ScrollDown event on preview area)
        let scroll_event = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 25,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, scroll_event, &tx);

        // Selection should still be active after scroll
        assert!(app.preview_selection.is_active());
        let (start2, end2) = app.preview_selection.normalized().unwrap();
        assert_eq!(start2.line, 0);
        assert_eq!(end2.line, 0);
        assert_eq!(end2.col, 11);
    }

    #[test]
    fn single_click_after_double_click_clears_selection() {
        let (_dir, mut app) = setup_app_with_preview();
        let tx = make_event_tx();

        // Double-click to select line 0
        let click_col = 25;
        let click_row = 1;
        handle_mouse_event(&mut app, make_mouse_down_left(click_col, click_row), &tx);
        handle_mouse_event(&mut app, make_mouse_up_left(click_col, click_row), &tx);
        handle_mouse_event(&mut app, make_mouse_down_left(click_col, click_row), &tx);
        assert!(app.preview_selection.is_active());

        // Now single-click somewhere else (a different row to avoid triple-click)
        let new_row = 3;
        handle_mouse_event(&mut app, make_mouse_down_left(click_col, new_row), &tx);
        handle_mouse_event(&mut app, make_mouse_up_left(click_col, new_row), &tx);

        // Single click + release at same point → selection should be cleared
        assert!(!app.preview_selection.is_active());
    }

    #[test]
    fn double_click_on_tree_does_not_affect_preview_selection() {
        let (_dir, mut app) = setup_app_with_preview();
        let tx = make_event_tx();

        // Set up a tree area
        app.tree_area = ratatui::layout::Rect::new(0, 0, 20, 10);

        // Pre-set a preview selection
        app.preview_selection
            .set_anchor(crate::terminal::TerminalCoord { line: 0, col: 0 });
        app.preview_selection
            .set_endpoint(crate::terminal::TerminalCoord { line: 0, col: 5 });
        assert!(app.preview_selection.is_active());

        // Click in tree area — should clear preview selection (existing behavior)
        handle_mouse_event(&mut app, make_mouse_down_left(5, 2), &tx);
        assert!(!app.preview_selection.is_active());
        // But last_preview_click should NOT be set (click was on tree)
        assert!(app.last_preview_click.is_none());
    }
}
