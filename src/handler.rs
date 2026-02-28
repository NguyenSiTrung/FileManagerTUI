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
    _event_tx: &mpsc::UnboundedSender<Event>,
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
                // Switch focus to tree
                app.focused_panel = FocusedPanel::Tree;

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
                                    app.expand_selected();
                                }
                            }
                        }
                    }
                }
            } else if is_in_rect(col, row, app.preview_area) {
                // Switch focus to preview
                app.focused_panel = FocusedPanel::Preview;
            } else if app.terminal_state.visible && is_in_rect(col, row, app.terminal_area) {
                // Switch focus to terminal
                app.focused_panel = FocusedPanel::Terminal;
            }
        }
        MouseEventKind::ScrollUp => {
            if is_in_rect(col, row, app.tree_area) {
                app.focused_panel = FocusedPanel::Tree;
                app.select_previous();
            } else if is_in_rect(col, row, app.preview_area) {
                app.focused_panel = FocusedPanel::Preview;
                app.preview_scroll_up();
            }
        }
        MouseEventKind::ScrollDown => {
            if is_in_rect(col, row, app.tree_area) {
                app.focused_panel = FocusedPanel::Tree;
                app.select_next();
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
                    editor.find_state.replacement.remove(pos - 1);
                    editor.find_state.replacement_cursor -= 1;
                }
            } else if editor.find_state.query_cursor > 0 {
                let pos = editor.find_state.query_cursor;
                editor.find_state.query.remove(pos - 1);
                editor.find_state.query_cursor -= 1;
                editor.update_find_matches();
            }
        }
        KeyCode::Char(c) => {
            if editor.find_state.in_replace_field {
                let pos = editor.find_state.replacement_cursor;
                editor.find_state.replacement.insert(pos, c);
                editor.find_state.replacement_cursor += 1;
            } else {
                let pos = editor.find_state.query_cursor;
                editor.find_state.query.insert(pos, c);
                editor.find_state.query_cursor += 1;
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
            app.toggle_terminal(event_tx);
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
        handle_terminal_keys(app, key);
        return;
    }

    // Global keys (work regardless of focus for tree/preview panels)
    match key.code {
        KeyCode::Char('q') => {
            app.quit();
            return;
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.quit();
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
        FocusedPanel::Preview => handle_preview_keys(app, key),
        FocusedPanel::Terminal => {} // Already handled above
    }
}

fn handle_tree_keys(app: &mut App, key: KeyEvent, event_tx: &mpsc::UnboundedSender<Event>) {
    match key.code {
        // Navigation
        KeyCode::Char('j') | KeyCode::Down => app.select_next(),
        KeyCode::Char('k') | KeyCode::Up => app.select_previous(),
        KeyCode::Char('g') | KeyCode::Home => app.select_first(),
        KeyCode::Char('G') | KeyCode::End => app.select_last(),

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
                } else {
                    app.expand_selected();
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
        KeyCode::Char('y') => {
            if app.tree_state.flat_items.get(app.tree_state.selected_index)
                .is_some_and(|i| i.node_type != NodeType::LoadMore) {
                app.copy_to_clipboard();
            }
        }
        KeyCode::Char('x') => {
            if app.tree_state.flat_items.get(app.tree_state.selected_index)
                .is_some_and(|i| i.node_type != NodeType::LoadMore) {
                app.cut_to_clipboard();
            }
        }
        KeyCode::Char('p') => app.paste_clipboard_async(event_tx.clone()),

        // File operations — open dialogs
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

        _ => {}
    }
}

fn handle_preview_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        // Enter edit mode
        KeyCode::Char('e') => {
            app.enter_edit_mode();
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
        // Toggle line wrap
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.preview_state.line_wrap = !app.preview_state.line_wrap;
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
fn handle_terminal_keys(app: &mut App, key: KeyEvent) {
    match key.code {
        // Esc returns focus to tree
        KeyCode::Esc => {
            app.focused_panel = FocusedPanel::Tree;
            return;
        }
        // Note: Tab is NOT intercepted here — it is forwarded to the PTY
        // for shell autocompletion (e.g. `cd <Tab>`).
        // Use Esc or Ctrl+T to leave the terminal panel.
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

    // Reset scroll offset on any input (auto-scroll to bottom)
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
            app.search_action_copy_path();
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
    let total = HelpOverlay::total_lines();
    match key.code {
        KeyCode::Char('?') | KeyCode::Esc => {
            app.mode = AppMode::Normal;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if app.help_state.scroll_offset < total.saturating_sub(1) {
                app.help_state.scroll_offset += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.help_state.scroll_offset = app.help_state.scroll_offset.saturating_sub(1);
        }
        KeyCode::Char('g') | KeyCode::Home => {
            app.help_state.scroll_offset = 0;
        }
        KeyCode::Char('G') | KeyCode::End => {
            app.help_state.scroll_offset = total.saturating_sub(1);
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
            let _ = app.save_editor_buffer();
            app.close_dialog();
            app.exit_edit_mode();
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

    fn setup_app() -> (TempDir, App) {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("alpha")).unwrap();
        fs::create_dir(dir.path().join("beta")).unwrap();
        File::create(dir.path().join("file_a.txt")).unwrap();
        File::create(dir.path().join(".hidden")).unwrap();
        let app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
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
    fn preview_ctrl_w_toggles_wrap() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        assert!(!app.preview_state.line_wrap);
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );
        assert!(app.preview_state.line_wrap);
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );
        assert!(!app.preview_state.line_wrap);
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
    fn search_arrow_navigates_results() {
        let (dir, mut app) = setup_app();
        std::fs::write(dir.path().join("file_a.txt"), "a").unwrap();
        std::fs::write(dir.path().join("file_b.rs"), "b").unwrap();
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

        let tx = make_event_tx();
        handle_mouse_event(&mut app, make_mouse_scroll_down(10, 5), &tx);
        assert_eq!(app.tree_state.selected_index, 1);
        handle_mouse_event(&mut app, make_mouse_scroll_up(10, 5), &tx);
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
}
