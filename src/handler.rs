use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::app::{App, AppMode, DialogKind, FocusedPanel};
use crate::components::help::HelpOverlay;
#[cfg(test)]
use crate::event::Event;
use crate::fs::operations;
use crate::fs::tree::NodeType;

/// Route literal paste to the active input context, never normal-mode commands.
pub fn handle_paste_event(app: &mut App, input: &str) {
    app.keymap.reset();
    if app.workspace.focus.overlay == AppMode::Normal
        && app.workspace_area.is_some()
        && !app.pane_available(app.workspace.focus.panel)
    {
        return;
    }
    if input.len() > 1024 * 1024 {
        app.set_status_message("Paste exceeds the 1 MiB limit".to_string());
        return;
    }
    if input.is_empty() {
        return;
    }
    let single_line = !input.contains(['\r', '\n']);
    match &app.workspace.focus.overlay {
        AppMode::CommandMenu => {
            if let Some(menu) = app.command_menu.as_mut() {
                menu.input(input);
            }
        }
        AppMode::Normal if app.workspace.focus.panel == FocusedPanel::Editor => {
            let max_bytes = app.config.max_editor_bytes();
            let max_lines = app.config.max_editor_lines();
            let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) else {
                return;
            };
            if editor.find_state.active {
                if !single_line {
                    app.set_status_message("Find input accepts a single line".to_string());
                    return;
                }
                let (field, cursor) = if editor.find_state.in_replace_field {
                    (
                        &mut editor.find_state.replacement,
                        &mut editor.find_state.replacement_cursor,
                    )
                } else {
                    (
                        &mut editor.find_state.query,
                        &mut editor.find_state.query_cursor,
                    )
                };
                field.insert_str(*cursor, input);
                *cursor += input.len();
                editor.update_find_matches();
                return;
            }
            let text = input.replace("\r\n", "\n");
            let current_bytes: usize =
                editor.buffer.iter().map(String::len).sum::<usize>() + editor.buffer.len() - 1;
            let selected = editor.selected_text();
            let added_lines = text.bytes().filter(|&byte| byte == b'\n').count();
            let removed_lines = selected.bytes().filter(|&byte| byte == b'\n').count();
            if (current_bytes - selected.len()).saturating_add(text.len()) as u64 > max_bytes
                || (editor.buffer.len() - removed_lines).saturating_add(added_lines) > max_lines
            {
                app.set_status_message(
                    "Paste exceeds configured editor size/line limits".to_string(),
                );
                return;
            }
            if let Err(error) = editor.insert_text(&text) {
                app.set_status_message(error.to_string());
            }
        }
        AppMode::Normal if app.workspace.focus.panel == FocusedPanel::Terminal => {
            if let Some(pty) = &app.terminal_state.pty {
                let bytes =
                    terminal_paste_bytes(input, app.terminal_state.emulator.bracketed_paste());
                if let Err(error) = pty.write(&bytes) {
                    app.set_status_message(format!("Terminal paste failed: {error}"));
                }
            } else {
                app.set_status_message("No running terminal for paste".to_string());
            }
        }
        AppMode::Dialog(
            DialogKind::CreateFile
            | DialogKind::CreateDirectory
            | DialogKind::Rename { .. }
            | DialogKind::EditorSaveAs { .. },
        ) if single_line => {
            app.dialog_state
                .input
                .insert_str(app.dialog_state.cursor_position, input);
            app.dialog_state.cursor_position += input.len();
        }
        AppMode::Search if single_line && app.document_list.is_none() => {
            for ch in input.chars() {
                app.search_input_char(ch);
            }
        }
        AppMode::Filter if single_line => {
            for ch in input.chars() {
                app.filter_input_char(ch);
            }
        }
        _ => {
            app.set_status_message("Paste is unavailable in this input context".to_string());
        }
    }
}

/// Handle a mouse event.
pub fn handle_mouse_event(app: &mut App, mouse: MouseEvent, event_tx: &crate::event::EventSender) {
    if !app.config.mouse_enabled() {
        app.splitter_drag = None;
        return;
    }
    if matches!(mouse.kind, MouseEventKind::Down(_)) {
        app.keymap.reset();
    }
    if app.workspace.focus.overlay != AppMode::Normal {
        app.splitter_drag = None;
    } else {
        use crate::app::SplitterDrag;
        let point = (mouse.column, mouse.row).into();
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            app.splitter_drag = if app.workspace_rects.explorer_split.contains(point) {
                Some(SplitterDrag::Explorer)
            } else if app.workspace_rects.terminal_split.contains(point) {
                Some(SplitterDrag::Terminal)
            } else {
                None
            };
            if app.splitter_drag.is_some() {
                app.scrollbar_dragging = false;
                app.preview_selection.end_drag();
                app.terminal_state.selection.end_drag();
                return;
            }
        } else if let Some(splitter) = app.splitter_drag {
            if matches!(
                mouse.kind,
                MouseEventKind::Drag(MouseButton::Left)
                    | MouseEventKind::Moved
                    | MouseEventKind::Up(MouseButton::Left)
            ) {
                if let Some(area) = app.workspace_area {
                    match splitter {
                        SplitterDrag::Explorer => {
                            app.workspace.layout.drag_explorer(area, mouse.column);
                        }
                        SplitterDrag::Terminal => {
                            app.workspace.layout.drag_terminal(area, mouse.row);
                        }
                    }
                    app.layout_changed();
                }
                if mouse.kind == MouseEventKind::Up(MouseButton::Left) {
                    app.splitter_drag = None;
                }
            }
            return;
        }
    }
    if app.workspace.focus.overlay == AppMode::CommandMenu {
        let command = app
            .command_menu
            .as_ref()
            .and_then(|menu| menu.hit(mouse.column, mouse.row));
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(command) = command {
                    if let Some(menu) = app.command_menu.as_mut() {
                        if let Some(index) = menu.filtered().iter().position(|m| m.id == command) {
                            menu.selected = index;
                        }
                    }
                    let _ = crate::commands::dispatch_command(app, command);
                } else if app
                    .command_menu
                    .as_ref()
                    .is_some_and(|menu| !menu.area.contains((mouse.column, mouse.row).into()))
                {
                    app.dismiss_command_menu();
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if let Some(menu) = app.command_menu.as_mut() {
                    menu.move_selection(if mouse.kind == MouseEventKind::ScrollUp {
                        -1
                    } else {
                        1
                    });
                }
            }
            _ => {}
        }
        return;
    }
    if app.workspace.focus.overlay == AppMode::LanguageFeatures {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = app
                    .language_features
                    .as_ref()
                    .and_then(|f| f.hit(mouse.column, mouse.row));
                if hit.is_some() {
                    if let (Some(features), Some(index)) = (app.language_features.as_mut(), hit) {
                        features.selected = index;
                    }
                    app.apply_language_selection();
                } else if app
                    .language_features
                    .as_ref()
                    .is_some_and(|f| !f.area.contains((mouse.column, mouse.row).into()))
                {
                    app.dismiss_language_features();
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if let Some(features) = app.language_features.as_mut() {
                    features.move_selection(if mouse.kind == MouseEventKind::ScrollUp {
                        -1
                    } else {
                        1
                    });
                }
            }
            _ => {}
        }
        return;
    }
    if app.workspace.focus.overlay == AppMode::Diagnostics {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = app
                    .diagnostics_panel
                    .as_ref()
                    .and_then(|panel| panel.hit(mouse.column, mouse.row));
                if hit.is_some() {
                    if let (Some(panel), Some(index)) = (app.diagnostics_panel.as_mut(), hit) {
                        panel.selected = index;
                    }
                    app.apply_diagnostic_selection();
                } else if app
                    .diagnostics_panel
                    .as_ref()
                    .is_some_and(|panel| !panel.area.contains((mouse.column, mouse.row).into()))
                {
                    app.dismiss_diagnostics();
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if let Some(panel) = app.diagnostics_panel.as_mut() {
                    panel.move_selection(if mouse.kind == MouseEventKind::ScrollUp {
                        -1
                    } else {
                        1
                    });
                }
            }
            _ => {}
        }
        return;
    }
    if app.workspace.focus.overlay == AppMode::Normal
        && app
            .command_entry_area
            .contains((mouse.column, mouse.row).into())
    {
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            let _ = crate::commands::dispatch_command(app, crate::commands::CommandId::Commands);
        }
        return;
    }
    // Only handle mouse in Normal mode for other panels
    if app.workspace.focus.overlay != AppMode::Normal {
        return;
    }

    let col = mouse.column;
    let row = mouse.row;
    if app.breadcrumbs.area.contains((col, row).into()) {
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            if let Some(path) = app.breadcrumbs.hit(col, row).map(std::path::Path::to_owned) {
                if !app.is_s3_mode() {
                    app.navigate_to_path(&path);
                    if app.pane_available(FocusedPanel::Tree) {
                        app.workspace.focus.panel = FocusedPanel::Tree;
                    }
                } else {
                    app.set_status_message("S3 breadcrumbs are read-only context".into());
                }
            }
        }
        return;
    }
    if app.document_tabs.area.contains((col, row).into()) {
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            if let Some(id) = app.document_tabs.hit(col, row) {
                app.activate_document(id);
            }
        }
        return;
    }
    if app.editor_visible() && is_in_rect(col, row, app.preview_content_area) {
        if matches!(mouse.kind, MouseEventKind::Down(_)) {
            app.workspace.focus.panel = FocusedPanel::Editor;
        }
        handle_editor_mouse(app, mouse);
        return;
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Determine which panel was clicked
            if is_in_rect(col, row, app.tree_content_area) {
                // Check if click is on the scrollbar column
                if let Some(sb_col) = app.scrollbar_column {
                    if col == sb_col {
                        // Scrollbar click-to-jump: map click row to scroll offset
                        app.workspace.focus.panel = FocusedPanel::Tree;
                        app.terminal_state.selection.clear();
                        app.preview_selection.clear();
                        app.scrollbar_dragging = true;
                        app.tree_viewport_locked = true;

                        let inner_y = row.saturating_sub(app.tree_content_area.y) as usize;
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
                app.workspace.focus.panel = FocusedPanel::Tree;
                // Unlock viewport so update_scroll works for clicked item
                app.tree_viewport_locked = false;

                // Map click to tree item index
                // Inner area: subtract border (1 top, 1 left)
                let inner_y = row.saturating_sub(app.tree_content_area.y);
                let clicked_index = app.tree_state.scroll_offset + inner_y as usize;

                if clicked_index < app.tree_state.flat_items.len() {
                    let already_selected = app.tree_state.selected_index == clicked_index;
                    app.tree_state.selected_index = clicked_index;
                    app.last_previewed_index = None; // Force preview update

                    if app.tree_state.flat_items[clicked_index].node_type == NodeType::File {
                        let path = app.tree_state.flat_items[clicked_index].path.clone();
                        let double = app.tree_last_click.as_ref().is_some_and(|(p, t)| {
                            *p == path && t.elapsed() < std::time::Duration::from_millis(500)
                        });
                        app.tree_last_click = if double {
                            None
                        } else {
                            Some((path.clone(), std::time::Instant::now()))
                        };
                        app.show_selected_preview();
                        app.update_preview();
                        if double {
                            app.open_document_path(&path, true);
                        } else {
                            app.workspace.focus.panel = FocusedPanel::Tree;
                        }
                    }

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
            } else if is_in_rect(col, row, app.preview_content_area) {
                // Clear any terminal selection when clicking elsewhere
                app.terminal_state.selection.clear();
                // Switch focus to preview
                app.workspace.focus.panel = FocusedPanel::Preview;

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
                                let text = crate::text::line_text(l);
                                crate::text::byte_to_display_col(
                                    &text,
                                    text.len(),
                                    crate::text::TAB_WIDTH,
                                )
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
            } else if is_in_rect(col, row, app.terminal_area) {
                // Switch focus to terminal and start/clear selection
                app.workspace.focus.panel = FocusedPanel::Terminal;
                app.preview_selection.clear();
                if let Some(coord) = mouse_to_terminal_coord(app, col, row, false) {
                    // Click sets anchor (clears any previous selection by overwriting)
                    app.terminal_state.selection.begin_drag(coord);
                }
            }
        }
        MouseEventKind::Down(MouseButton::Right) => {
            if is_in_rect(col, row, app.terminal_area) {
                app.workspace.focus.panel = FocusedPanel::Terminal;
                app.copy_terminal_selection(event_tx);
            } else if is_in_rect(col, row, app.preview_content_area) {
                app.workspace.focus.panel = FocusedPanel::Preview;
                app.copy_preview_selection(event_tx);
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            // Scrollbar drag-to-scroll
            if app.scrollbar_dragging {
                let inner_y = row.saturating_sub(app.tree_content_area.y) as usize;
                let visible_height = app.tree_visible_height;
                let total = app.tree_state.flat_items.len();
                let max_scroll = total.saturating_sub(visible_height);

                if visible_height > 1 && max_scroll > 0 {
                    let track_max = visible_height - 1;
                    let clamped_y = inner_y.min(track_max);
                    let new_offset = clamped_y * max_scroll / track_max;
                    app.tree_state.scroll_offset = new_offset.min(max_scroll);
                }
            } else if app.terminal_area.width > 0 && app.terminal_state.selection.dragging {
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
                let inner_y = row.saturating_sub(app.tree_content_area.y) as usize;
                let visible_height = app.tree_visible_height;
                let total = app.tree_state.flat_items.len();
                let max_scroll = total.saturating_sub(visible_height);

                if visible_height > 1 && max_scroll > 0 {
                    let track_max = visible_height - 1;
                    let clamped_y = inner_y.min(track_max);
                    let new_offset = clamped_y * max_scroll / track_max;
                    app.tree_state.scroll_offset = new_offset.min(max_scroll);
                }
            } else if app.terminal_area.width > 0 && app.terminal_state.selection.dragging {
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
            if is_in_rect(col, row, app.tree_content_area) {
                app.workspace.focus.panel = FocusedPanel::Tree;
                let n = app.config.scroll_lines();
                app.tree_scroll_up(n);
            } else if is_in_rect(col, row, app.preview_content_area) {
                app.workspace.focus.panel = FocusedPanel::Preview;
                app.preview_scroll_up();
            } else if is_in_rect(col, row, app.terminal_area) {
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
            if is_in_rect(col, row, app.tree_content_area) {
                app.workspace.focus.panel = FocusedPanel::Tree;
                let n = app.config.scroll_lines();
                app.tree_scroll_down(n);
            } else if is_in_rect(col, row, app.preview_content_area) {
                app.workspace.focus.panel = FocusedPanel::Preview;
                app.preview_scroll_down();
            } else if is_in_rect(col, row, app.terminal_area) {
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
    let inner_x = area.x;
    let inner_y = area.y;
    let inner_w = area.width;
    let inner_h = area.height;

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
    let area = app.preview_content_area;
    let inner_x = area.x;
    let inner_y = area.y;
    let inner_w = area.width;
    let inner_h = area.height;

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
    let row_count = app.preview_state.visual_row_count(inner_w as usize);
    let max_start = row_count.saturating_sub(visible_height);
    let start = app.preview_state.scroll_offset.min(max_start);
    let (abs_line, mapped) = app
        .preview_state
        .visual_row(start + local_row, inner_w as usize)
        .or_else(|| {
            app.preview_state
                .visual_row(row_count.saturating_sub(1), inner_w as usize)
        })?;
    Some(crate::terminal::TerminalCoord {
        line: abs_line,
        col: if app.preview_state.line_wrap {
            (mapped.start + local_col).min(mapped.end)
        } else {
            app.preview_state.horizontal_offset + local_col
        },
    })
}

/// Handle mouse events when in editor mode.
fn handle_editor_mouse(app: &mut App, mouse: MouseEvent) {
    let col = mouse.column;
    let row = mouse.row;

    // Only handle clicks within the preview/editor area
    if !is_in_rect(col, row, app.preview_content_area) {
        return;
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                let height = app.preview_content_area.height as usize;
                let code_height = height.saturating_sub(editor.find_bar_height(height));
                let code_width = app
                    .preview_content_area
                    .width
                    .saturating_sub(editor.gutter_width());
                let inner_y = app.preview_content_area.y;
                if code_width == 0
                    || code_height == 0
                    || row < inner_y
                    || row as usize >= inner_y as usize + code_height
                {
                    return;
                }
                let (target_line, target_col) =
                    mouse_to_editor_pos(editor, app.preview_content_area, col, row);
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
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                let (target_line, target_col) =
                    mouse_to_editor_pos(editor, app.preview_content_area, col, row);
                // Move cursor without clearing selection — anchor stays put
                editor.set_cursor_position_for_selection(target_line, target_col);
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            // If anchor == cursor after click-release (no drag), clear selection
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                if let Some(ref sel) = editor.selection {
                    if sel.anchor_line == editor.cursor_line && sel.anchor_col == editor.cursor_col
                    {
                        editor.selection = None;
                    }
                }
            }
        }
        MouseEventKind::ScrollUp => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.scroll_offset = editor.scroll_offset.saturating_sub(3);
                editor.clamp_viewport();
            }
        }
        MouseEventKind::ScrollDown => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                let max_scroll = editor
                    .visual_row_count()
                    .saturating_sub(editor.visible_height.max(1));
                editor.scroll_offset = (editor.scroll_offset + 3).min(max_scroll);
                editor.clamp_viewport();
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
    let inner_x = preview_area.x;
    let inner_y = preview_area.y;
    let gutter_w = editor.gutter_width();
    let code_x = inner_x + gutter_w;

    let inner_height = preview_area.height as usize;
    let code_height = inner_height.saturating_sub(editor.find_bar_height(inner_height));
    let click_row = (row.saturating_sub(inner_y) as usize).min(code_height.saturating_sub(1));
    let (target_line, mapped) = editor.visual_row(editor.scroll_offset + click_row);
    let width = preview_area.width.saturating_sub(gutter_w) as usize;
    let local_col = (col.saturating_sub(code_x) as usize).min(width.saturating_sub(1));
    let target_col = if editor.line_wrap {
        (mapped.start + local_col).min(mapped.end)
    } else {
        editor.horizontal_offset + local_col
    };
    let byte = crate::text::display_col_to_byte(
        &editor.buffer[target_line],
        target_col,
        crate::text::TAB_WIDTH,
    );
    (target_line, byte)
}

/// Check if a position (col, row) is inside a Rect.
fn is_in_rect(col: u16, row: u16, rect: ratatui::layout::Rect) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}

/// Handle a key event and dispatch to the appropriate app method.
pub fn handle_key_event(app: &mut App, key: KeyEvent, event_tx: &crate::event::EventSender) {
    // Ignore key release events to prevent duplicate actions from press/release pairs.
    if key.kind == KeyEventKind::Release {
        return;
    }
    use crate::keymap::{FocusContext, Resolution};
    let context = app.input_context();
    let target = (context, app.workspace.documents.active_id());
    if app.keymap_target.is_some_and(|old| old != target) {
        app.keymap.reset();
    }
    app.keymap_target = Some(target);
    let now_ms = app.keymap_epoch.elapsed().as_millis().min(u64::MAX as u128) as u64;
    match app.keymap.feed(context, key, now_ms) {
        Resolution::Consumed => return,
        Resolution::Command(command) => {
            // Preserve native preview-selection copy over the default Ctrl+C quit
            // route; an explicit rebind to a different command still resolves normally.
            if command == crate::commands::CommandId::Quit
                && context == FocusContext::Preview
                && key.code == KeyCode::Char('c')
                && key.modifiers.contains(KeyModifiers::CONTROL)
                && app.preview_selection.is_active()
            {
                app.copy_preview_selection(event_tx);
                return;
            }
            // The event adapter supplies the existing async route, not a new dispatch path.
            if command == crate::commands::CommandId::ToggleTerminal && app.event_tx.is_none() {
                app.event_tx = Some(event_tx.clone());
            }
            if let Err(error) = crate::commands::dispatch_command(app, command) {
                app.set_status_message(error);
            }
            return;
        }
        Resolution::Forward => {}
    }
    if app.workspace.focus.overlay == AppMode::Normal
        && app.workspace_area.is_some()
        && !app.pane_available(app.workspace.focus.panel)
    {
        return;
    }

    match &app.workspace.focus.overlay {
        AppMode::CommandMenu => handle_command_menu(app, key),
        AppMode::LanguageFeatures => handle_language_features(app, key),
        AppMode::Diagnostics => handle_diagnostics(app, key),
        AppMode::Normal => handle_normal_mode(app, key, event_tx),
        AppMode::Dialog(_) => handle_dialog_mode(app, key),
        AppMode::Search => handle_search_mode(app, key),
        AppMode::SearchAction => handle_search_action_mode(app, key, event_tx),
        AppMode::Filter => handle_filter_mode(app, key),
        AppMode::Help => handle_help_mode(app, key),
        AppMode::CopyOverlay => match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                // The consumer restores its own suspended capture after dispatch.
                // Never enqueue onto the queue this handler is itself consuming.
                app.dismiss_copy_overlay();
            }
            KeyCode::Up => app.copy_overlay_scroll.0 = app.copy_overlay_scroll.0.saturating_sub(1),
            KeyCode::Down => {
                app.copy_overlay_scroll.0 = app.copy_overlay_scroll.0.saturating_add(1)
            }
            KeyCode::Left => {
                app.copy_overlay_scroll.1 = app.copy_overlay_scroll.1.saturating_sub(1)
            }
            KeyCode::Right => {
                app.copy_overlay_scroll.1 = app.copy_overlay_scroll.1.saturating_add(1)
            }
            _ => {}
        },
    }
}

fn handle_command_menu(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.dismiss_command_menu(),
        KeyCode::Enter => {
            if let Some(id) = app.command_menu.as_ref().and_then(|m| m.selected_command()) {
                let _ = crate::commands::dispatch_command(app, id);
            }
        }
        _ => {
            let Some(menu) = app.command_menu.as_mut() else {
                return;
            };
            match key.code {
                KeyCode::Up => menu.move_selection(-1),
                KeyCode::Down => menu.move_selection(1),
                KeyCode::PageUp => menu.move_selection(-8),
                KeyCode::PageDown => menu.move_selection(8),
                KeyCode::Home => menu.move_selection(isize::MIN),
                KeyCode::End => menu.move_selection(isize::MAX),
                KeyCode::Backspace => menu.backspace(),
                KeyCode::Char(ch)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    menu.input(&ch.to_string())
                }
                _ => {}
            }
        }
    }
}

/// Keys while the language-feature overlay is up: navigation only — the
/// list is read-only. Enter applies a completion or jumps to a location.
fn handle_language_features(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.dismiss_language_features(),
        KeyCode::Enter => app.apply_language_selection(),
        _ => {
            let Some(features) = app.language_features.as_mut() else {
                return;
            };
            match key.code {
                KeyCode::Up => features.move_selection(-1),
                KeyCode::Down => features.move_selection(1),
                KeyCode::PageUp => features.move_selection(-8),
                KeyCode::PageDown => features.move_selection(8),
                KeyCode::Home => features.move_selection(isize::MIN),
                KeyCode::End => features.move_selection(isize::MAX),
                _ => {}
            }
        }
    }
}

/// Keys while the diagnostics panel is up: navigation only — the list is
/// read-only. Enter jumps to the selected diagnostic's position.
fn handle_diagnostics(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.dismiss_diagnostics(),
        KeyCode::Enter => app.apply_diagnostic_selection(),
        _ => {
            let Some(panel) = app.diagnostics_panel.as_mut() else {
                return;
            };
            match key.code {
                KeyCode::Up => panel.move_selection(-1),
                KeyCode::Down => panel.move_selection(1),
                KeyCode::PageUp => panel.move_selection(-8),
                KeyCode::PageDown => panel.move_selection(8),
                KeyCode::Home => panel.move_selection(isize::MIN),
                KeyCode::End => panel.move_selection(isize::MAX),
                _ => {}
            }
        }
    }
}

/// Handle keys when in Edit mode (editing a file in the preview panel).
fn handle_editor_keys(app: &mut App, key: KeyEvent) {
    // If find bar is active, handle find/replace keys first
    if app
        .workspace
        .documents
        .active()
        .map(|d| &d.editor)
        .is_some_and(|e| e.find_state.active)
    {
        handle_editor_find_keys(app, key);
        return;
    }

    match key.code {
        // Exit edit mode
        KeyCode::Esc => {
            let is_modified = app
                .workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .is_some_and(|e| e.modified);
            if is_modified {
                // Show save confirmation dialog
                app.set_overlay(AppMode::Dialog(DialogKind::FocusBackConfirm));
            } else {
                app.exit_edit_mode();
            }
        }

        // Undo/Redo
        KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.undo();
            }
        }
        KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.redo();
            }
        }

        // Select all (Ctrl+A)
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.select_all();
            }
        }

        // Find / Replace
        KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.open_find();
            }
        }
        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.open_find_replace();
            }
        }

        // Editor clipboard
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.copy_line();
            }
            app.copy_editor_text();
        }
        KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.cut_line();
            }
            app.copy_editor_text();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                let text = editor.clipboard_paste_text();
                let selected = editor.selected_text();
                let bytes = editor.buffer.iter().map(String::len).sum::<usize>()
                    + editor.buffer.len().saturating_sub(1);
                let lines = editor.buffer.len() - selected.bytes().filter(|&b| b == b'\n').count()
                    + text.bytes().filter(|&b| b == b'\n').count();
                if text.len() > 1024 * 1024
                    || bytes
                        .saturating_sub(selected.len())
                        .saturating_add(text.len()) as u64
                        > app.config.max_editor_bytes()
                    || lines > app.config.max_editor_lines()
                {
                    app.set_status_message(
                        "Paste exceeds configured editor size/line limits".into(),
                    );
                } else {
                    editor.paste();
                }
            }
        }

        // Selection-aware navigation (Shift+Arrow) — must match before plain navigation
        KeyCode::Home
            if key
                .modifiers
                .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT) =>
        {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.select_to_top();
            }
        }
        KeyCode::End
            if key
                .modifiers
                .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT) =>
        {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.select_to_bottom();
            }
        }
        KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.select_up();
            }
        }
        KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.select_down();
            }
        }
        KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.select_left();
            }
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.select_right();
            }
        }
        KeyCode::Home if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.select_home();
            }
        }
        KeyCode::End if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.select_end();
            }
        }
        KeyCode::PageUp if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.select_page_up();
            }
        }
        KeyCode::PageDown if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.select_page_down();
            }
        }

        // Navigation with Ctrl modifiers
        KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.move_to_top();
            }
        }
        KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.move_to_bottom();
            }
        }

        // Basic navigation
        KeyCode::Up => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.move_up();
            }
        }
        KeyCode::Down => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.move_down();
            }
        }
        KeyCode::Left => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.move_left();
            }
        }
        KeyCode::Right => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.move_right();
            }
        }
        KeyCode::Home => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.move_home();
            }
        }
        KeyCode::End => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.move_end();
            }
        }
        KeyCode::PageUp => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.page_up();
            }
        }
        KeyCode::PageDown => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.page_down();
            }
        }

        // Editing
        KeyCode::Enter => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.insert_newline();
                editor.ensure_cursor_visible();
            }
        }
        KeyCode::Backspace => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.delete_char_before();
                editor.ensure_cursor_visible();
            }
        }
        KeyCode::Delete => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.delete_char_at();
            }
        }
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.dedent();
            }
        }
        KeyCode::Tab => {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.insert_tab();
            }
        }

        // Character input
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) =>
        {
            if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
                editor.insert_char(c);
                editor.ensure_cursor_visible();
            }
        }

        _ => {}
    }
}

/// Handle keys when the find/replace bar is active in editor mode.
fn handle_editor_find_keys(app: &mut App, key: KeyEvent) {
    let editor = match app.workspace.documents.active_mut().map(|d| &mut d.editor) {
        Some(e) => e,
        None => return,
    };

    match key.code {
        KeyCode::Esc => {
            editor.close_find();
            app.close_dialog();
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
            let (field, cursor) = if editor.find_state.in_replace_field {
                (
                    &mut editor.find_state.replacement,
                    &mut editor.find_state.replacement_cursor,
                )
            } else {
                (
                    &mut editor.find_state.query,
                    &mut editor.find_state.query_cursor,
                )
            };
            if *cursor > 0 {
                let start = crate::text::previous_grapheme_boundary(field, *cursor);
                field.replace_range(start..*cursor, "");
                *cursor = start;
            }
            if !editor.find_state.in_replace_field {
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

fn handle_normal_mode(app: &mut App, key: KeyEvent, event_tx: &crate::event::EventSender) {
    // If terminal is focused, forward all other keys to the PTY
    if app.workspace.focus.panel == FocusedPanel::Terminal {
        handle_terminal_keys(app, key, event_tx);
        return;
    }

    // Reserved directional/terminal keys above also work from the editor,
    // including an active document-local find bar. Ordinary keys stay local.
    match app
        .workspace
        .focus
        .input_target(app.workspace.documents.active_id())
    {
        crate::workspace::focus::InputTarget::Editor(_) => {
            handle_editor_keys(app, key);
            return;
        }
        crate::workspace::focus::InputTarget::NoDocument => {
            if key.code == KeyCode::Tab {
                app.toggle_focus();
            }
            return;
        }
        _ => {}
    }

    // Global keys (work regardless of focus for tree/preview panels)
    match key.code {
        // Copy preview selection when preview is focused.
        // Supports Ctrl+Shift+C, Ctrl+C (when selection exists), Cmd+C, and Ctrl+Insert.
        KeyCode::Char('C')
            if app.workspace.focus.panel == FocusedPanel::Preview
                && (key.modifiers.contains(KeyModifiers::CONTROL)
                    || key.modifiers.contains(KeyModifiers::SUPER)) =>
        {
            app.copy_preview_selection(event_tx);
            return;
        }
        KeyCode::Char('c')
            if app.workspace.focus.panel == FocusedPanel::Preview
                && ((key.modifiers.contains(KeyModifiers::CONTROL)
                    && (key.modifiers.contains(KeyModifiers::SHIFT)
                        || app.preview_selection.is_active()))
                    || key.modifiers.contains(KeyModifiers::SUPER)) =>
        {
            app.copy_preview_selection(event_tx);
            return;
        }
        KeyCode::Insert
            if app.workspace.focus.panel == FocusedPanel::Preview
                && key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            app.copy_preview_selection(event_tx);
            return;
        }
        KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.undo();
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
            app.set_overlay(AppMode::Help);
            return;
        }
        _ => {}
    }

    // An unbound modified character is not its unmodified tree operation.
    // In particular, removed Alt+R must never turn into Rename.
    if matches!(key.code, KeyCode::Char(_))
        && key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return;
    }
    // Dispatch based on focused panel
    match app.workspace.focus.panel {
        FocusedPanel::Tree => handle_tree_keys(app, key, event_tx),
        FocusedPanel::Preview => handle_preview_keys(app, key, event_tx),
        FocusedPanel::Editor => handle_editor_keys(app, key),
        FocusedPanel::Terminal => {} // Already handled above
    }
}

fn handle_tree_keys(app: &mut App, key: KeyEvent, event_tx: &crate::event::EventSender) {
    // Any keyboard action in the tree unlocks the viewport so it follows selection
    app.tree_viewport_locked = false;

    // S3 mode: block write operations with user-friendly message
    if app.is_s3_mode() {
        match key.code {
            KeyCode::Char('a')
            | KeyCode::Char('A')
            | KeyCode::Char('d')
            | KeyCode::Char('r')
            | KeyCode::Char('p')
            | KeyCode::Char('x') => {
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

        KeyCode::Enter
            if app
                .tree_state
                .flat_items
                .get(app.tree_state.selected_index)
                .is_some_and(|item| item.node_type == NodeType::File) =>
        {
            let path = app.tree_state.flat_items[app.tree_state.selected_index]
                .path
                .clone();
            app.open_document_path(&path, true);
        }
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

        // S3 head preview toggle
        KeyCode::Char('H') => {
            if app.is_s3_mode() {
                if let Some(item) = app.tree_state.flat_items.get(app.tree_state.selected_index) {
                    if item.node_type == NodeType::File {
                        if app.s3_head_active {
                            // Toggle back to metadata view
                            app.s3_head_active = false;
                            app.s3_head_content = None;
                            app.s3_head_uri = None;
                            app.last_previewed_index = None;
                            app.update_preview();
                        } else {
                            app.spawn_s3_head(event_tx);
                        }
                    }
                }
            }
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

pub(crate) fn handle_preview_keys(
    app: &mut App,
    key: KeyEvent,
    event_tx: &crate::event::EventSender,
) {
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
                    if !app.spawn_async_dir_summary(path, event_tx) {
                        return;
                    }
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
                    app.set_status_message("Deep scan started...".to_string());
                }
            }
        }
        // Line-by-line scroll
        KeyCode::Right => app.preview_scroll_horizontal(true),
        KeyCode::Left => app.preview_scroll_horizontal(false),
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

        // S3 head preview toggle (same as tree panel H)
        KeyCode::Char('H') => {
            if app.is_s3_mode() {
                if app.s3_head_active {
                    app.s3_head_active = false;
                    app.s3_head_content = None;
                    app.s3_head_uri = None;
                    app.last_previewed_index = None;
                    app.update_preview();
                } else if let Some(item) =
                    app.tree_state.flat_items.get(app.tree_state.selected_index)
                {
                    if item.node_type == crate::fs::tree::NodeType::File {
                        app.spawn_s3_head(event_tx);
                    }
                }
            }
        }

        // Copy path to system clipboard (same as tree panel Y)
        KeyCode::Char('Y') => {
            app.copy_path_to_system_clipboard(event_tx);
        }

        _ => {}
    }
}

/// Handle keys when terminal panel is focused.
/// All non-reserved keys are forwarded to the PTY as raw bytes.
fn handle_terminal_keys(app: &mut App, key: KeyEvent, event_tx: &crate::event::EventSender) {
    match key.code {
        // Copy terminal selection to system clipboard.
        // Supports Ctrl+Shift+C, Ctrl+C (when selection exists), and Cmd+C on macOS terminals
        // that pass SUPER-modified keys through to the app.
        KeyCode::Char('C')
            if key
                .modifiers
                .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
                || key.modifiers.contains(KeyModifiers::SUPER) =>
        {
            app.copy_terminal_selection(event_tx);
            return;
        }
        KeyCode::Char('c')
            if (key.modifiers.contains(KeyModifiers::CONTROL)
                && key.modifiers.contains(KeyModifiers::SHIFT))
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

    // Convert KeyEvent to bytes and send to PTY. DEC mode 1 (DECCKM) from
    // the child selects application-cursor encoding for arrows/Home/End.
    let bytes = key_event_to_bytes(&key, app.terminal_state.emulator.application_cursor_keys());
    if !bytes.is_empty() {
        if let Some(ref pty) = app.terminal_state.pty {
            if let Err(error) = pty.write(&bytes) {
                app.set_status_message(format!("Terminal key not accepted: {error}"));
            }
        }
    }
}

/// Wrap pasted text in bracketed-paste markers when the child enabled
/// DEC mode 2004; otherwise the literal bytes go through unchanged.
fn terminal_paste_bytes(input: &str, bracketed_paste: bool) -> Vec<u8> {
    if !bracketed_paste {
        return input.as_bytes().to_vec();
    }
    let mut bytes = Vec::with_capacity(input.len() + 12);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(input.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes
}

/// Convert a crossterm KeyEvent into the byte sequence expected by a PTY.
/// `application_cursor_keys` is DEC mode 1 (DECCKM): when the child set it,
/// arrows and Home/End use SS3 (`\x1bO`) sequences instead of CSI (`\x1b[`).
fn key_event_to_bytes(key: &KeyEvent, application_cursor_keys: bool) -> Vec<u8> {
    let mut ordinary = *key;
    ordinary.modifiers.remove(KeyModifiers::ALT);
    let mut bytes = key_event_to_bytes_without_alt(&ordinary, application_cursor_keys);
    if key.modifiers.contains(KeyModifiers::ALT) && !bytes.is_empty() {
        bytes.insert(0, 0x1b);
    }
    bytes
}

fn key_event_to_bytes_without_alt(key: &KeyEvent, application_cursor_keys: bool) -> Vec<u8> {
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
        KeyCode::Up => application_or_normal(b"\x1bOA", b"\x1b[A", application_cursor_keys),
        KeyCode::Down => application_or_normal(b"\x1bOB", b"\x1b[B", application_cursor_keys),
        KeyCode::Right => application_or_normal(b"\x1bOC", b"\x1b[C", application_cursor_keys),
        KeyCode::Left => application_or_normal(b"\x1bOD", b"\x1b[D", application_cursor_keys),
        KeyCode::Home => application_or_normal(b"\x1bOH", b"\x1b[H", application_cursor_keys),
        KeyCode::End => application_or_normal(b"\x1bOF", b"\x1b[F", application_cursor_keys),
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

fn application_or_normal(
    application: &[u8],
    normal: &[u8],
    application_cursor_keys: bool,
) -> Vec<u8> {
    if application_cursor_keys {
        application.to_vec()
    } else {
        normal.to_vec()
    }
}

fn handle_search_mode(app: &mut App, key: KeyEvent) {
    if let Some(ids) = app.document_list.as_ref() {
        match key.code {
            KeyCode::Esc => {
                app.document_list = None;
                app.dismiss_overlay();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.document_list_index =
                    (app.document_list_index + 1).min(ids.len().saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.document_list_index = app.document_list_index.saturating_sub(1);
            }
            KeyCode::Enter => {
                let id = ids.get(app.document_list_index).copied();
                app.document_list = None;
                app.dismiss_overlay();
                if let Some(id) = id {
                    app.activate_document(id);
                }
            }
            _ => {}
        }
        return;
    }
    match key.code {
        KeyCode::Esc => app.close_search(),
        KeyCode::Tab => app.toggle_content_search_mode(),
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
            app.search_secondary_actions()
        }
        KeyCode::F(2) => app.search_secondary_actions(),
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

fn handle_search_action_mode(app: &mut App, key: KeyEvent, event_tx: &crate::event::EventSender) {
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
    let total = HelpOverlay::new(&theme, &app.help_state)
        .app(app)
        .total_lines_for_tab();

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
            app.set_overlay(AppMode::Normal);
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
            app.set_overlay(AppMode::Normal);
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
                app.help_state.settings_state = Some(SettingsState::from_app(app));
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
                    app.set_overlay(AppMode::Normal);
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
    let kind = match &app.workspace.focus.overlay {
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
            handle_save_confirm(app, key, false);
        }
        DialogKind::FocusBackConfirm => {
            handle_save_confirm(app, key, true);
        }
        DialogKind::DocumentDecision { id, .. } => match key.code {
            KeyCode::Char('s' | 'S' | 'y' | 'Y') => {
                let _ = app.save_editor_buffer();
            }
            KeyCode::Char('d' | 'D') => app.discard_lifecycle_document(*id),
            KeyCode::Esc | KeyCode::Char('c' | 'C') => app.close_dialog(),
            _ => {}
        },
        DialogKind::SaveConflict {
            exit_after_save,
            normalize,
            ..
        } => match key.code {
            KeyCode::Esc | KeyCode::Char('c' | 'C') => app.close_dialog(),
            KeyCode::Char('r' | 'R') => app.reload_editor_buffer(),
            KeyCode::Char('a' | 'A') => app.open_dialog(DialogKind::EditorSaveAs {
                exit_after_save: *exit_after_save,
                normalize: *normalize,
            }),
            KeyCode::Char('o' | 'O') => app.begin_editor_overwrite(*exit_after_save, *normalize),
            _ => {}
        },
        DialogKind::SaveOverwrite {
            exit_after_save,
            normalize,
            expected_revision,
        } => match key.code {
            KeyCode::Char('y' | 'Y') => {
                let _ = app.confirm_editor_overwrite(
                    expected_revision.as_ref(),
                    *exit_after_save,
                    *normalize,
                );
            }
            KeyCode::Esc | KeyCode::Char('n' | 'N' | 'c' | 'C') => app.close_dialog(),
            _ => {}
        },
        DialogKind::EditorSaveAs {
            exit_after_save,
            normalize,
        } => match key.code {
            KeyCode::Esc => {
                app.close_dialog();
                app.dialog_state = crate::app::DialogState::default();
            }
            KeyCode::Enter => {
                let input = app.dialog_state.input.clone();
                if !input.is_empty() {
                    let _ = app.save_editor_as(&input, *exit_after_save, *normalize);
                }
            }
            _ => handle_input_dialog(app, key, kind),
        },
        DialogKind::SaveSettings => {
            handle_save_settings_dialog(app, key);
        }
        DialogKind::RecoveryPrompt { .. } => match key.code {
            KeyCode::Char('r' | 'R' | 'y' | 'Y') => {
                let outcome = app.restore_recovery();
                app.set_status_message(outcome.unwrap_or_else(|error| error));
                // Re-offer the next remaining record, bounded by the record
                // count; closes the dialog when nothing is left.
                app.reoffer_recovery_prompt();
            }
            KeyCode::Char('d' | 'D') => {
                let outcome = app.discard_recovery();
                app.set_status_message(outcome.unwrap_or_else(|error| error));
                app.reoffer_recovery_prompt();
            }
            KeyCode::Esc | KeyCode::Char('n' | 'N' | 'c' | 'C') => app.close_dialog(),
            _ => {}
        },
        DialogKind::LspTrust { .. } => match key.code {
            // Approval binds this session to the exact (root, argv) shown —
            // anything else the project config might add stays untrusted.
            KeyCode::Char('y' | 'Y') | KeyCode::Enter => app.approve_lsp_trust(),
            KeyCode::Esc | KeyCode::Char('n' | 'N' | 'c' | 'C') => app.deny_lsp_trust(),
            _ => {}
        },
        DialogKind::LspStatus { .. } => match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | 'Q' | 'c' | 'C') => {
                app.close_dialog()
            }
            _ => {}
        },
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
        // Forward delete: move right then backspace
        KeyCode::Delete if app.dialog_state.cursor_position < app.dialog_state.input.len() => {
            app.dialog_move_cursor_right();
            app.dialog_delete_char();
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
                let changes = match app
                    .workspace
                    .documents
                    .preflight_rename(original, &new_path)
                {
                    Ok(changes) => changes,
                    Err(error) => {
                        app.set_status_message(format!("Rename refused: {error}"));
                        app.close_dialog();
                        return;
                    }
                };
                match operations::rename(original, &new_path) {
                    Ok(()) => {
                        // Old keys die: a republish under the new URI must
                        // not merge into entries for a moved-away path.
                        let old_uris: Vec<String> = changes
                            .iter()
                            .filter_map(|(id, _)| app.workspace.documents.get(*id))
                            .map(|d| crate::lsp::features::uri_for_path(d.path()))
                            .collect();
                        app.workspace.documents.commit_rename(changes);
                        for uri in old_uris {
                            app.diagnostics.remove_document(&uri);
                        }
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
                } else {
                    app.workspace.documents.mark_deleted_path(target);
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
/// Y/y = Save and return, N/n = Return retaining changes, Esc/C/c = Cancel.
fn handle_save_confirm(app: &mut App, key: KeyEvent, focus_back: bool) {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            let _ = app.save_editor_buffer();
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            app.close_dialog();
            if focus_back {
                app.exit_edit_mode();
            }
            app.set_status_message("Unsaved changes retained in workspace".to_string());
        }
        KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('C') => {
            // Cancel — return to edit mode
            app.close_dialog();
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
            app.set_overlay(AppMode::Help);
        }
        _ => {}
    }
}

const SETTINGS_CONFIG_LIMIT: usize = 1024 * 1024;

struct PreparedSettingsSave {
    config: crate::config::AppConfig,
    bytes: Vec<u8>,
    revision: Option<crate::fs::save::FileRevision>,
}

/// Capture exact bytes/revision together before parsing or merging. Never refresh
/// this baseline after the generated settings have been prepared.
fn prepare_settings_save(
    app: &App,
    path: &std::path::Path,
) -> Result<PreparedSettingsSave, String> {
    let settings = app
        .help_state
        .settings_state
        .as_ref()
        .ok_or("No settings state")?;
    let (source, revision) =
        match crate::fs::save::load_document_bounded(path, SETTINGS_CONFIG_LIMIT) {
            Ok((bytes, revision)) => (bytes, Some(revision)),
            Err(crate::fs::save::SaveError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                (Vec::new(), None)
            }
            Err(error) => return Err(error.to_string()),
        };
    let candidate = settings.merged_config(&app.config)?;
    let mut doc = std::str::from_utf8(&source)
        .map_err(|e| e.to_string())?
        .parse::<toml::Table>()
        .map_err(|e| e.to_string())?;
    let mut modified = settings.modified_table()?;
    if let Some(layout) = modified
        .get_mut("layout")
        .and_then(toml::Value::as_table_mut)
    {
        for (key, value) in [
            ("explorer_width", candidate.layout.explorer_width),
            ("terminal_height", candidate.layout.terminal_height),
        ] {
            if layout.contains_key(key) {
                layout.insert(
                    key.into(),
                    toml::Value::Integer(value.expect("validated layout").into()),
                );
            }
        }
    }
    for (section, values) in modified {
        let target = doc
            .entry(section)
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let target = target
            .as_table_mut()
            .ok_or("Existing config section is not a table")?;
        target.extend(
            values
                .as_table()
                .ok_or("Setting section is not a table")?
                .clone(),
        );
    }
    let persisted: crate::config::AppConfig = toml::Value::Table(doc.clone())
        .try_into()
        .map_err(|e| e.to_string())?;
    // A partial file may rely on a lower-source or explicit CLI profile.
    // Validate its bindings in the effective profile, not artificial Standard.
    let mut effective_persisted = candidate.keymap.clone().merge(&persisted.keymap);
    effective_persisted.profile = candidate.keymap.profile;
    effective_persisted.timeout_ms = candidate.keymap.timeout_ms;
    crate::keymap::Keymap::compile(&effective_persisted)?;
    let text = toml::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    if text.len() > SETTINGS_CONFIG_LIMIT {
        return Err("Config exceeds 1 MiB".into());
    }
    Ok(PreparedSettingsSave {
        config: candidate,
        bytes: text.into_bytes(),
        revision,
    })
}

fn persist_settings_save(
    path: &std::path::Path,
    prepared: &PreparedSettingsSave,
) -> Result<(), String> {
    if prepared.bytes.len() > SETTINGS_CONFIG_LIMIT {
        return Err("Config exceeds 1 MiB".into());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    crate::fs::save::save_document_bounded(
        path,
        &prepared.bytes,
        prepared.revision.as_ref(),
        SETTINGS_CONFIG_LIMIT,
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

fn finish_settings_save(
    app: &mut App,
    path: &std::path::Path,
    prepared: Result<PreparedSettingsSave, String>,
) {
    let result = prepared.and_then(|prepared| {
        persist_settings_save(path, &prepared)?;
        Ok(prepared.config)
    });
    match result {
        Ok(candidate) => {
            apply_settings_candidate(app, candidate);
            let entries = crate::components::settings::SettingsState::from_app(app).entries;
            if let Some(settings) = app.help_state.settings_state.as_mut() {
                settings.entries = entries;
            }
            app.set_status_message(format!("Settings saved and applied to {}", path.display()));
            app.set_overlay(AppMode::Help);
        }
        Err(error) => app.set_status_message(format!("Settings not saved or applied: {error}")),
    }
}

/// The production flow publishes only the exact captured preparation baseline.
fn save_settings_to_file(app: &mut App, path: &std::path::Path) {
    let prepared = prepare_settings_save(app, path);
    finish_settings_save(app, path, prepared);
}

/// Called only after validation (and successful persistence for the Save action).
fn apply_settings_candidate(app: &mut App, candidate: crate::config::AppConfig) {
    let old = app.config.clone();
    // Compile/swap before any other runtime mutation, preserving pending input on errors.
    if let Err(error) = app.apply_keymap_config(candidate.keymap.clone()) {
        app.set_status_message(format!("Settings not applied: {error}"));
        return;
    }
    let explicitly_modified = |section: &str, key: &str| {
        app.help_state
            .settings_state
            .as_ref()
            .is_some_and(|settings| {
                settings.entries.iter().any(|entry| {
                    entry.section == section && entry.key == key && entry.modified_value.is_some()
                })
            })
    };
    let wrap_modified = explicitly_modified("preview", "line_wrap");
    let recovery_modified = explicitly_modified("recovery", "enabled");
    let watcher_modified =
        explicitly_modified("watcher", "enabled") || explicitly_modified("watcher", "auto_refresh");
    let preview_modified = [
        "enabled",
        "default_view_mode",
        "head_lines",
        "tail_lines",
        "max_full_preview_bytes",
    ]
    .iter()
    .any(|key| explicitly_modified("preview", key));
    // Apply only explicitly edited fields; runtime controls are the saved authority.
    let layout_keys = [
        "explorer_width",
        "explorer_visible",
        "terminal_height",
        "terminal_visible",
    ];
    let modified_layout: Vec<_> = layout_keys
        .into_iter()
        .filter(|key| explicitly_modified("layout", key))
        .collect();
    let layout = candidate.layout.state();
    app.config = candidate;
    // Keep the live recovery policy in lock-step with the applied config by
    // routing through the same entry point the recovery commands use. A settings
    // edit to `[recovery] enabled` that only rewrote `app.config` would leave the
    // running context writing snapshots until restart.
    if recovery_modified || old.recovery.enabled != app.config.recovery.enabled {
        app.set_recovery_enabled(app.config.recovery_enabled());
    }
    if !modified_layout.is_empty() {
        app.workspace.layout.restore();
        for key in modified_layout {
            match key {
                "explorer_width" => app
                    .workspace
                    .layout
                    .set_explorer_width(layout.explorer_width()),
                "terminal_height" => app
                    .workspace
                    .layout
                    .set_terminal_height(layout.terminal_height()),
                "explorer_visible"
                    if app.workspace.layout.explorer_visible() != layout.explorer_visible() =>
                {
                    app.workspace.layout.toggle_explorer()
                }
                "terminal_visible"
                    if app.workspace.layout.terminal_visible() != layout.terminal_visible() =>
                {
                    app.workspace.layout.toggle_terminal()
                }
                _ => {}
            }
        }
    }
    app.layout_changed();
    if old.show_hidden() != app.config.show_hidden()
        || old.sort_by() != app.config.sort_by()
        || old.dirs_first() != app.config.dirs_first()
    {
        let selected = app
            .tree_state
            .flat_items
            .get(app.tree_state.selected_index)
            .map(|item| item.path.clone());
        app.tree_state.show_hidden = app.config.show_hidden();
        app.tree_state.sort_by = crate::fs::tree::SortBy::from_str(app.config.sort_by());
        app.tree_state.dirs_first = app.config.dirs_first();
        app.tree_state.sort_all_children();
        app.tree_state.flatten();
        if let Some(index) = selected.and_then(|p| {
            app.tree_state
                .flat_items
                .iter()
                .position(|item| item.path == p)
        }) {
            app.tree_state.selected_index = index;
        }
    }
    if watcher_modified
        || old.watcher.enabled != app.config.watcher.enabled
        || old.watcher.auto_refresh != app.config.watcher.auto_refresh
    {
        app.watcher_active =
            !app.is_s3_mode() && app.config.watcher_enabled() && app.config.watcher_auto_refresh();
    }
    app.terminal_state
        .set_scrollback_limit(app.config.terminal_scrollback());
    app.workspace
        .documents
        .set_limits(crate::workspace::documents::DocumentLimits {
            max_bytes: app.config.max_editor_bytes_usize(),
            max_lines: app.config.max_editor_lines(),
        });
    if wrap_modified || old.preview.line_wrap != app.config.preview.line_wrap {
        let wrap = app.config.preview.line_wrap.unwrap_or(false);
        let origin = app
            .help_state
            .origin
            .clone()
            .unwrap_or_else(|| crate::commands::CommandContext::capture(app));
        if let Some(editor) = origin
            .text_view_document(app)
            .and_then(|id| app.workspace.documents.get_mut(id))
            .map(|d| &mut d.editor)
        {
            if editor.line_wrap != wrap {
                editor.toggle_wrap();
            }
        } else if app.preview_state.line_wrap != wrap {
            app.preview_toggle_wrap();
        }
    }
    if old.theme.scheme != app.config.theme.scheme
        || old.preview.syntax_theme != app.config.preview.syntax_theme
    {
        app.theme_colors = crate::theme::resolve_theme(&app.config.theme);
        let name = app
            .config
            .syntax_theme_name(app.config.theme_scheme())
            .to_string();
        app.syntax_theme = crate::preview_content::load_theme(Some(&name));
        app.last_previewed_index = None;
    }
    if preview_modified
        || old.preview.enabled != app.config.preview.enabled
        || old.preview.default_view_mode != app.config.preview.default_view_mode
        || old.preview.head_lines != app.config.preview.head_lines
        || old.preview.tail_lines != app.config.preview.tail_lines
        || old.preview.max_full_preview_bytes != app.config.preview.max_full_preview_bytes
    {
        app.last_previewed_index = None;
        if !app.config.preview_enabled() {
            app.preview_state = crate::app::PreviewState::default();
            app.preview_selection.clear();
        } else {
            app.update_preview();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn adaptive_settings_persist_bounded_layout_and_apply_without_shell_start() {
        use crate::components::settings::SettingValueKind;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("settings.toml");
        std::fs::write(&destination, "[preview]\nenabled = false\n").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        task3_modify(
            &mut app,
            "layout",
            "explorer_width",
            SettingValueKind::UInt(999),
        );
        task3_modify(
            &mut app,
            "layout",
            "terminal_height",
            SettingValueKind::UInt(0),
        );
        task3_modify(
            &mut app,
            "layout",
            "terminal_visible",
            SettingValueKind::Bool(true),
        );
        save_settings_to_file(&mut app, &destination);
        let saved: crate::config::AppConfig =
            toml::from_str(&std::fs::read_to_string(&destination).unwrap()).unwrap();
        assert_eq!(saved.layout.explorer_width, Some(80));
        assert_eq!(saved.layout.terminal_height, Some(4));
        assert!(!saved.preview_enabled());
        assert_eq!(app.workspace.layout.explorer_width(), 80);
        assert_eq!(app.workspace.layout.terminal_height(), 4);
        assert!(app.workspace.layout.terminal_visible());
        assert!(app.terminal_state.pty.is_none());
    }
    fn install_editor(app: &mut App, editor: crate::editor::EditorState) {
        let temporary;
        let path = if editor.file_path.is_absolute() && editor.file_path.exists() {
            editor.file_path.clone()
        } else {
            temporary = tempfile::NamedTempFile::new().unwrap();
            std::fs::write(temporary.path(), editor.buffer.join("\n")).unwrap();
            temporary.path().to_path_buf()
        };
        let id = app
            .workspace
            .documents
            .open(&path, crate::workspace::documents::OpenDisposition::Pinned)
            .unwrap();
        app.workspace.documents.get_mut(id).unwrap().editor = editor;
    }
    use super::*;

    /// Apply modified settings from the settings state into the live app config.
    fn apply_settings_live(app: &mut App) {
        let candidate = match app
            .help_state
            .settings_state
            .as_ref()
            .map(|s| s.merged_config(&app.config))
        {
            Some(Ok(config)) => config,
            Some(Err(error)) => {
                app.set_status_message(format!("Settings not applied: {error}"));
                return;
            }
            None => return,
        };
        apply_settings_candidate(app, candidate);
    }

    /// P2-1 root red/green owner: an applied settings candidate that edits
    /// `[recovery] enabled` must take effect on the live recovery policy through
    /// the same path the commands use, so the settings surface and the command
    /// surface can never disagree.
    #[test]
    fn task3_applying_settings_disables_recovery_live_not_only_after_restart() {
        use crate::components::settings::SettingValueKind;
        use crate::recovery::RecoveryStore;
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = root.path().join("doc.txt");
        std::fs::write(&path, "body\n").unwrap();
        let mut app = App::new(root.path(), crate::config::AppConfig::default()).unwrap();
        let store = RecoveryStore::new(state.path());
        app.configure_recovery(store.clone());
        assert!(app.recovery_enabled());

        // A dirty buffer captures while recovery is enabled.
        assert!(app.open_document_path(&path, true));
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .insert_text("draft\n")
            .unwrap();
        let policy = app.recovery.as_ref().unwrap().policy;
        assert_eq!(
            app.snapshot_dirty_documents(std::time::Instant::now()),
            None
        );
        let captured = store.load_all(root.path(), &policy, std::time::SystemTime::now());
        assert_eq!(captured.len(), 1);
        let record_path = store.record_path(root.path(), &path, &captured[0].revision);

        // Toggle Settings -> Recovery off and apply the candidate.
        task3_modify(
            &mut app,
            "recovery",
            "enabled",
            SettingValueKind::Bool(false),
        );
        apply_settings_live(&mut app);

        // The live policy is disabled immediately, not only after restart: the
        // command surface agrees and a further edit writes no new snapshot.
        assert!(
            !app.recovery_enabled(),
            "live recovery policy must follow settings"
        );
        assert!(!app.recovery.as_ref().unwrap().policy.enabled);
        assert!(!app.config.recovery_enabled());
        let context = crate::commands::CommandContext::capture(&app);
        assert_eq!(
            crate::commands::unavailable_reason(
                &app,
                &context,
                crate::commands::CommandId::RecoveryRestore
            ),
            Some("Private recovery is disabled")
        );
        let before = std::fs::read_to_string(&record_path).unwrap();
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .insert_text("more\n")
            .unwrap();
        let next =
            std::time::Instant::now() + policy.min_interval + std::time::Duration::from_millis(1);
        assert_eq!(app.snapshot_dirty_documents(next), None);
        assert_eq!(std::fs::read_to_string(&record_path).unwrap(), before);

        // Re-enabling through settings restores the live command surface.
        task3_modify(
            &mut app,
            "recovery",
            "enabled",
            SettingValueKind::Bool(true),
        );
        apply_settings_live(&mut app);
        assert!(app.recovery_enabled());
        let reenabled = std::time::Instant::now() + std::time::Duration::from_secs(60);
        assert_eq!(app.snapshot_dirty_documents(reenabled), None);
        assert!(std::fs::read_to_string(&record_path)
            .unwrap()
            .contains("more"));
    }

    fn task3_modify(
        app: &mut App,
        section: &str,
        key: &str,
        value: crate::components::settings::SettingValueKind,
    ) {
        if app.help_state.settings_state.is_none() {
            app.help_state.settings_state =
                Some(crate::components::settings::SettingsState::from_app(app));
        }
        let entry = app
            .help_state
            .settings_state
            .as_mut()
            .unwrap()
            .entries
            .iter_mut()
            .find(|e| e.section == section && e.key == key)
            .unwrap();
        entry.modified_value = Some(value);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn task3_live_history_preserves_existing_shell_identity_and_liveness() {
        struct Cleanup(App);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                self.0.shutdown_terminal();
            }
        }
        async fn pid(app: &App, rx: &mut crate::event::EventReceiver, marker: &str) -> String {
            let command = format!("printf '\\n{marker}=%s\\n' $$\n");
            app.terminal_state
                .pty
                .as_ref()
                .unwrap()
                .write(command.as_bytes())
                .unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                let mut bytes = Vec::new();
                loop {
                    if let Event::TerminalOutput { data, .. } = rx.recv().await.expect("PTY output")
                    {
                        bytes.extend(data);
                    }
                    assert!(bytes.len() < 64 * 1024, "bounded fixture output");
                    let text = String::from_utf8_lossy(&bytes);
                    for line in text
                        .split_inclusive('\n')
                        .filter(|line| line.ends_with('\n'))
                    {
                        if let Some(value) = line.trim().strip_prefix(&format!("{marker}=")) {
                            if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
                                return value.to_string();
                            }
                        }
                    }
                }
            })
            .await
            .expect("bounded shell identity handshake")
        }
        let dir = tempfile::tempdir().unwrap();
        let mut guard = Cleanup(App::new(dir.path(), crate::config::AppConfig::default()).unwrap());
        let (tx, mut rx) = crate::event::event_channel(Default::default());
        guard.0.terminal_state.pty = Some(
            crate::terminal::pty::PtyProcess::spawn("/bin/sh", dir.path(), 24, 80, tx).unwrap(),
        );
        let before = pid(&guard.0, &mut rx, "TASK3_BEFORE").await;
        guard
            .0
            .terminal_state
            .emulator
            .process(&b"history\r\n".repeat(80));
        let cursor = guard.0.terminal_state.emulator.cursor_position();
        guard.0.set_overlay(AppMode::Help);
        task3_modify(
            &mut guard.0,
            "terminal",
            "scrollback_lines",
            crate::components::settings::SettingValueKind::UInt(0),
        );
        apply_settings_live(&mut guard.0);
        assert_eq!(guard.0.terminal_state.emulator.scrollback_len(), 0);
        assert_eq!(guard.0.terminal_state.emulator.cursor_position(), cursor);
        assert!(guard.0.terminal_state.pty.as_ref().unwrap().is_alive());
        assert_eq!(pid(&guard.0, &mut rx, "TASK3_AFTER").await, before);
    }

    fn round1_writer_fixture() -> (tempfile::TempDir, App, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.set_overlay(AppMode::Help);
        task3_modify(
            &mut app,
            "general",
            "show_hidden",
            crate::components::settings::SettingValueKind::Bool(true),
        );
        let path = dir.path().join("config.toml");
        std::fs::write(&path, b"[general]\nshow_hidden = false\n").unwrap();
        (dir, app, path)
    }

    fn round1_prime_pending(app: &mut App) {
        assert!(matches!(
            app.keymap.feed(
                crate::keymap::FocusContext::Tree,
                KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT),
                0
            ),
            crate::keymap::Resolution::Consumed
        ));
    }

    fn round1_assert_failure_retained(app: &mut App) {
        assert!(!app.config.show_hidden());
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
        assert_eq!(app.workspace.focus.overlay, AppMode::Help);
        assert_eq!(
            app.help_state
                .settings_state
                .as_ref()
                .unwrap()
                .modified_count(),
            1
        );
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("not saved or applied"));
        assert!(matches!(
            app.keymap.feed(
                crate::keymap::FocusContext::Tree,
                KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE),
                1
            ),
            crate::keymap::Resolution::Command(crate::commands::CommandId::Commands)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn round1_writer_refuses_dangling_and_ancestor_symlinks() {
        use std::os::unix::fs::symlink;
        for dangling in [false, true] {
            let (dir, mut app, path) = round1_writer_fixture();
            let before = std::fs::read(&path).unwrap();
            let destination;
            if dangling {
                destination = dir.path().join("dangling.toml");
                symlink(dir.path().join("missing.toml"), &destination).unwrap();
            } else {
                let alias = dir.path().join("alias");
                symlink(dir.path(), &alias).unwrap();
                destination = alias.join("config.toml");
            }
            round1_prime_pending(&mut app);
            save_settings_to_file(&mut app, &destination);
            round1_assert_failure_retained(&mut app);
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert!(!dir.path().join("missing.toml").exists());
            let link = if dangling {
                destination
            } else {
                dir.path().join("alias")
            };
            assert!(std::fs::symlink_metadata(link)
                .unwrap()
                .file_type()
                .is_symlink());
        }
    }

    #[cfg(unix)]
    #[test]
    fn round1_writer_publication_refuses_changed_leaf_or_ancestor_symlink() {
        use std::os::unix::fs::symlink;
        for ancestor in [false, true] {
            let (dir, mut app, path) = round1_writer_fixture();
            round1_prime_pending(&mut app);
            let destination;
            let external = b"[general]\nconfirm_delete = false\n";
            if ancestor {
                let parent = dir.path().join("parent");
                std::fs::create_dir(&parent).unwrap();
                destination = parent.join("config.toml");
                std::fs::rename(&path, &destination).unwrap();
            } else {
                destination = path;
            }
            let prepared = prepare_settings_save(&app, &destination).unwrap();
            let managed = dir.path().join("managed");
            if ancestor {
                std::fs::rename(destination.parent().unwrap(), &managed).unwrap();
                std::fs::write(managed.join("config.toml"), external).unwrap();
                symlink(&managed, destination.parent().unwrap()).unwrap();
            } else {
                std::fs::write(&managed, external).unwrap();
                std::fs::remove_file(&destination).unwrap();
                symlink(&managed, &destination).unwrap();
            }
            finish_settings_save(&mut app, &destination, Ok(prepared));
            assert_eq!(std::fs::read(&destination).unwrap(), external);
            round1_assert_failure_retained(&mut app);
            let link = if ancestor {
                destination.parent().unwrap().to_path_buf()
            } else {
                destination
            };
            assert!(std::fs::symlink_metadata(link)
                .unwrap()
                .file_type()
                .is_symlink());
        }
    }

    #[cfg(unix)]
    #[test]
    fn round1_writer_refuses_hardlinks_and_readonly_files() {
        use std::os::unix::fs::PermissionsExt;
        for hardlink in [false, true] {
            let (dir, mut app, path) = round1_writer_fixture();
            if hardlink {
                std::fs::hard_link(&path, dir.path().join("other")).unwrap();
            } else {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
            }
            let before = std::fs::read(&path).unwrap();
            round1_prime_pending(&mut app);
            save_settings_to_file(&mut app, &path);
            assert_eq!(std::fs::read(&path).unwrap(), before);
            round1_assert_failure_retained(&mut app);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn round1_writer_refuses_inherited_acl_and_preserves_original_mode() {
        use std::os::unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, PermissionsExt},
        };
        let (dir, mut app, path) = round1_writer_fixture();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let mut acl = 2u32.to_le_bytes().to_vec();
        for (tag, permission, id) in [
            (1u16, 7u16, u32::MAX),
            (2, 4, 42424),
            (4, 5, u32::MAX),
            (16, 5, u32::MAX),
            (32, 0, u32::MAX),
        ] {
            acl.extend(tag.to_le_bytes());
            acl.extend(permission.to_le_bytes());
            acl.extend(id.to_le_bytes());
        }
        let name = std::ffi::CString::new(dir.path().as_os_str().as_bytes()).unwrap();
        // SAFETY: Valid C name and serialized ACL buffer belong to this fixture.
        assert_eq!(
            unsafe {
                libc::setxattr(
                    name.as_ptr(),
                    c"system.posix_acl_default".as_ptr(),
                    acl.as_ptr().cast(),
                    acl.len(),
                    0,
                )
            },
            0
        );
        let before = std::fs::read(&path).unwrap();
        round1_prime_pending(&mut app);
        save_settings_to_file(&mut app, &path);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o7777, 0o640);
        round1_assert_failure_retained(&mut app);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn round1_writer_faults_retain_dirty_document_and_pending_state() {
        for stage in [
            crate::fs::save::Stage::Write,
            crate::fs::save::Stage::Permissions,
            crate::fs::save::Stage::Validate,
            crate::fs::save::Stage::Replace,
        ] {
            let (dir, mut app, path) = round1_writer_fixture();
            app.dismiss_overlay();
            let document = dir.path().join("dirty.txt");
            std::fs::write(&document, "retained").unwrap();
            app.open_document_path(&document, true);
            let id = app.workspace.documents.active_id();
            let editor = &mut app.workspace.documents.active_mut().unwrap().editor;
            editor.insert_text("dirty").unwrap();
            editor.find_state.query = "remember".into();
            app.workspace.focus.panel = FocusedPanel::Tree;
            app.set_overlay(AppMode::Help);
            round1_prime_pending(&mut app);
            let before = std::fs::read(&path).unwrap();
            if cfg!(any(target_os = "linux", target_os = "macos")) {
                crate::fs::save::inject_failure(stage);
            } else {
                // Replacement fails before the injected stages on unsupported
                // platforms. Do not leak an unconsumed fault into another test.
                let revision = crate::fs::save::load_document_bounded(&path, SETTINGS_CONFIG_LIMIT)
                    .unwrap()
                    .1;
                assert!(matches!(
                    crate::fs::save::save_document_bounded(
                        &path,
                        b"unchanged",
                        Some(&revision),
                        SETTINGS_CONFIG_LIMIT
                    ),
                    Err(crate::fs::save::SaveError::UnsupportedReplacement)
                ));
            }
            save_settings_to_file(&mut app, &path);
            assert_eq!(std::fs::read(&path).unwrap(), before);
            round1_assert_failure_retained(&mut app);
            assert_eq!(app.workspace.documents.active_id(), id);
            let doc = app.workspace.documents.active().unwrap();
            assert_eq!(doc.text(), "dirtyretained");
            assert!(doc.editor.modified);
            assert_eq!(doc.editor.find_state.query, "remember");
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        }
    }

    #[test]
    fn round1_writer_source_and_generated_output_remain_bounded() {
        for source_too_large in [false, true] {
            let (_dir, mut app, path) = round1_writer_fixture();
            if source_too_large {
                std::fs::write(&path, vec![b'x'; SETTINGS_CONFIG_LIMIT + 1]).unwrap();
            } else {
                app.help_state
                    .settings_state
                    .as_mut()
                    .unwrap()
                    .entries
                    .iter_mut()
                    .find(|e| e.key == "show_hidden")
                    .unwrap()
                    .modified_value = None;
                task3_modify(
                    &mut app,
                    "preview",
                    "syntax_theme",
                    crate::components::settings::SettingValueKind::Str(
                        "x".repeat(SETTINGS_CONFIG_LIMIT),
                    ),
                );
            }
            let before = std::fs::read(&path).unwrap();
            round1_prime_pending(&mut app);
            save_settings_to_file(&mut app, &path);
            assert_eq!(std::fs::read(&path).unwrap(), before);
            round1_assert_failure_retained(&mut app);
        }
    }

    #[cfg(unix)]
    #[test]
    fn round1_writer_new_private_config_is_restrictive() {
        use std::os::unix::fs::MetadataExt;
        let (_dir, mut app, path) = round1_writer_fixture();
        std::fs::remove_file(&path).unwrap();
        save_settings_to_file(&mut app, &path);
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert!(app.config.show_hidden());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn round1_writer_refuses_unpreservable_owner() {
        use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
        // SAFETY: geteuid has no pointer arguments or side effects.
        if unsafe { libc::geteuid() } != 0 {
            return;
        }
        let (_dir, mut app, path) = round1_writer_fixture();
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: Valid C pathname points to this test's isolated temporary file.
        assert_eq!(unsafe { libc::chown(name.as_ptr(), 65534, 65534) }, 0);
        let before = std::fs::read(&path).unwrap();
        round1_prime_pending(&mut app);
        save_settings_to_file(&mut app, &path);
        assert_eq!(std::fs::metadata(&path).unwrap().uid(), 65534);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        round1_assert_failure_retained(&mut app);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn round1_writer_refuses_and_preserves_extended_attributes() {
        use std::os::unix::ffi::OsStrExt;
        let (_dir, mut app, path) = round1_writer_fixture();
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: C strings and attribute buffer remain valid for this syscall.
        assert_eq!(
            unsafe {
                libc::setxattr(
                    name.as_ptr(),
                    c"user.fm_task3".as_ptr(),
                    b"keep".as_ptr().cast(),
                    4,
                    0,
                )
            },
            0
        );
        let before = std::fs::read(&path).unwrap();
        round1_prime_pending(&mut app);
        save_settings_to_file(&mut app, &path);
        let mut value = [0u8; 4];
        // SAFETY: The output buffer has the declared capacity and valid C names.
        assert_eq!(
            unsafe {
                libc::getxattr(
                    name.as_ptr(),
                    c"user.fm_task3".as_ptr(),
                    value.as_mut_ptr().cast(),
                    value.len(),
                )
            },
            4
        );
        assert_eq!(&value, b"keep");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        round1_assert_failure_retained(&mut app);
    }

    #[test]
    fn round1_writer_captured_snapshot_refuses_modified_deleted_replaced_or_grown_target() {
        for change in ["modified", "deleted", "replaced", "grown"] {
            let (dir, mut app, path) = round1_writer_fixture();
            round1_prime_pending(&mut app);
            let prepared = prepare_settings_save(&app, &path).unwrap();
            assert!(prepared.revision.is_some());
            let external = match change {
                "modified" => {
                    let bytes = b"[general]\nshow_hidden = true \n".to_vec();
                    std::fs::write(&path, &bytes).unwrap();
                    Some(bytes)
                }
                "deleted" => {
                    std::fs::remove_file(&path).unwrap();
                    None
                }
                "replaced" => {
                    let bytes = std::fs::read(&path).unwrap();
                    let other = dir.path().join("replacement");
                    std::fs::write(&other, &bytes).unwrap();
                    std::fs::rename(other, &path).unwrap();
                    Some(bytes)
                }
                _ => {
                    let bytes = vec![b'x'; SETTINGS_CONFIG_LIMIT + 1];
                    std::fs::write(&path, &bytes).unwrap();
                    Some(bytes)
                }
            };
            finish_settings_save(&mut app, &path, Ok(prepared));
            assert_eq!(std::fs::read(&path).ok(), external, "{change}");
            round1_assert_failure_retained(&mut app);
            assert_eq!(
                std::fs::read_dir(dir.path()).unwrap().count(),
                usize::from(external.is_some())
            );
        }
    }

    #[test]
    fn round1_writer_exclusive_creation_preserves_collision_and_live_state() {
        let (dir, mut app, path) = round1_writer_fixture();
        std::fs::remove_file(&path).unwrap();
        round1_prime_pending(&mut app);
        let prepared = prepare_settings_save(&app, &path).unwrap();
        assert!(prepared.revision.is_none());
        let external = b"[general]\nconfirm_delete = false\n";
        std::fs::write(&path, external).unwrap();
        finish_settings_save(&mut app, &path, Ok(prepared));
        assert_eq!(std::fs::read(&path).unwrap(), external);
        round1_assert_failure_retained(&mut app);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn round1_writer_preserves_private_existing_permissions() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let (_dir, mut app, path) = round1_writer_fixture();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let before = std::fs::metadata(&path).unwrap();
        save_settings_to_file(&mut app, &path);
        let after = std::fs::metadata(&path).unwrap();
        assert_eq!(after.mode() & 0o7777, 0o600);
        assert_eq!((after.uid(), after.gid()), (before.uid(), before.gid()));
        assert!(app.config.show_hidden());
    }

    #[cfg(unix)]
    #[test]
    fn round1_writer_preserves_leaf_symlink_and_refuses_live_apply() {
        use std::os::unix::fs::symlink;
        let (dir, mut app, path) = round1_writer_fixture();
        let managed = dir.path().join("managed.toml");
        std::fs::rename(&path, &managed).unwrap();
        symlink(&managed, &path).unwrap();
        let before = std::fs::read(&managed).unwrap();
        save_settings_to_file(&mut app, &path);
        assert!(std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(&managed).unwrap(), before);
        assert!(!app.config.show_hidden());
        assert!(
            app.help_state
                .settings_state
                .as_ref()
                .unwrap()
                .modified_count()
                > 0
        );
    }

    #[test]
    fn task3_partial_save_validates_with_effective_lower_or_cli_profile() {
        use crate::keymap::{BindingOverride, FocusContext, KeymapProfile};
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::AppConfig::default();
        config.keymap.profile = Some(KeymapProfile::Web);
        config.keymap.bindings = Some(vec![BindingOverride {
            command: "document.save".into(),
            context: FocusContext::Editor,
            keys: vec!["Ctrl+T".into()],
        }]);
        let mut app = App::new(dir.path(), config).unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[[keymap.bindings]]\ncommand = \"document.save\"\ncontext = \"editor\"\nkeys = [\"Ctrl+T\"]\n").unwrap();
        task3_modify(
            &mut app,
            "general",
            "show_hidden",
            crate::components::settings::SettingValueKind::Bool(true),
        );
        save_settings_to_file(&mut app, &path);
        assert!(app.config.show_hidden());
        assert_eq!(app.config.keymap.profile, Some(KeymapProfile::Web));
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("show_hidden = true"));
    }

    #[test]
    fn task3_explicit_wrap_setting_applies_when_global_default_already_matches() {
        use crate::components::settings::SettingValueKind;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "abcdefghijk").unwrap();
        let mut config = crate::config::AppConfig::default();
        config.preview.line_wrap = Some(true);
        let mut app = App::new(dir.path(), config).unwrap();
        app.open_document_path(&path, true);
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .toggle_wrap();
        app.set_overlay(AppMode::Help);
        task3_modify(
            &mut app,
            "preview",
            "line_wrap",
            SettingValueKind::Bool(true),
        );
        apply_settings_live(&mut app);
        assert!(app.workspace.documents.active().unwrap().editor.line_wrap);
    }

    #[test]
    fn task3_explicit_auto_refresh_setting_applies_when_startup_default_matches() {
        use crate::components::settings::SettingValueKind;
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::AppConfig::default();
        config.watcher.enabled = Some(true);
        config.watcher.auto_refresh = Some(true);
        let mut app = App::new(dir.path(), config).unwrap();
        app.watcher_active = false;
        app.set_overlay(AppMode::Help);
        task3_modify(
            &mut app,
            "watcher",
            "auto_refresh",
            SettingValueKind::Bool(true),
        );
        apply_settings_live(&mut app);
        assert!(app.watcher_active);
    }

    #[test]
    fn task3_live_preview_modes_and_partial_settings_have_actual_effects() {
        use crate::components::settings::SettingValueKind;
        let (dir, mut app) = setup_app();
        std::fs::write(
            dir.path().join("file_a.txt"),
            "TOPUNIQUE\nMID\nBOTTOMUNIQUE\n",
        )
        .unwrap();
        app.tree_state.selected_index = app
            .tree_state
            .flat_items
            .iter()
            .position(|n| n.name == "file_a.txt")
            .unwrap();
        app.update_preview();
        let id = app.workspace.documents.active_id();
        app.set_overlay(AppMode::Help);
        task3_modify(&mut app, "preview", "head_lines", SettingValueKind::UInt(1));
        task3_modify(&mut app, "preview", "tail_lines", SettingValueKind::UInt(1));
        task3_modify(
            &mut app,
            "preview",
            "default_view_mode",
            SettingValueKind::Enum("head_only".into(), vec![]),
        );
        task3_modify(
            &mut app,
            "general",
            "show_hidden",
            SettingValueKind::Bool(true),
        );
        let selected = app.tree_state.flat_items[app.tree_state.selected_index]
            .path
            .clone();
        apply_settings_live(&mut app);
        assert_eq!(app.preview_state.view_mode, crate::app::ViewMode::HeadOnly);
        let text = app
            .preview_state
            .content_lines
            .iter()
            .map(crate::text::line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("TOPUNIQUE") && !text.contains("BOTTOMUNIQUE"));
        assert_eq!(
            app.tree_state.flat_items[app.tree_state.selected_index].path,
            selected
        );
        assert!(app.tree_state.show_hidden);
        assert_eq!(app.workspace.documents.active_id(), id);
        task3_modify(
            &mut app,
            "preview",
            "default_view_mode",
            SettingValueKind::Enum("tail_only".into(), vec![]),
        );
        apply_settings_live(&mut app);
        let text = app
            .preview_state
            .content_lines
            .iter()
            .map(crate::text::line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("TOPUNIQUE") && text.contains("BOTTOMUNIQUE"));
        task3_modify(
            &mut app,
            "preview",
            "enabled",
            SettingValueKind::Bool(false),
        );
        apply_settings_live(&mut app);
        assert!(app.preview_state.current_path.is_none());
        task3_modify(&mut app, "preview", "enabled", SettingValueKind::Bool(true));
        apply_settings_live(&mut app);
        assert_eq!(app.preview_state.view_mode, crate::app::ViewMode::TailOnly);
        assert!(app.preview_state.current_path.is_some());
    }

    #[test]
    fn task3_live_origin_wrap_preview_disable_limits_theme_and_watcher_retention() {
        use crate::components::settings::SettingValueKind;
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "abcdefghijk").unwrap();
        std::fs::write(&b, "independent long text").unwrap();
        let mut config = crate::config::AppConfig::default();
        config.watcher.enabled = Some(true);
        config.watcher.auto_refresh = Some(true);
        let mut app = App::new(dir.path(), config).unwrap();
        app.watcher_active = false; // Deliberate manual preference, independent of startup config.
        app.open_document_path(&a, true);
        let aid = app.workspace.documents.active_id().unwrap();
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .update_viewport(4, 3);
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .insert_text("dirty")
            .unwrap();
        app.open_document_path(&b, true);
        let bid = app.workspace.documents.active_id().unwrap();
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .horizontal_offset = 6;
        app.activate_document(aid);
        app.set_overlay(AppMode::Help);
        app.workspace.documents.activate(bid).unwrap();
        task3_modify(
            &mut app,
            "preview",
            "line_wrap",
            SettingValueKind::Bool(true),
        );
        task3_modify(
            &mut app,
            "preview",
            "enabled",
            SettingValueKind::Bool(false),
        );
        task3_modify(
            &mut app,
            "general",
            "max_editor_bytes",
            SettingValueKind::UInt(1),
        );
        task3_modify(
            &mut app,
            "general",
            "max_editor_lines",
            SettingValueKind::UInt(1),
        );
        task3_modify(
            &mut app,
            "theme",
            "scheme",
            SettingValueKind::Enum("light".into(), vec![]),
        );
        task3_modify(
            &mut app,
            "preview",
            "syntax_theme",
            SettingValueKind::Str(" ".into()),
        );
        apply_settings_live(&mut app);
        assert!(app.workspace.documents.get(aid).unwrap().editor.line_wrap);
        let editor_b = &app.workspace.documents.get(bid).unwrap().editor;
        assert!(!editor_b.line_wrap);
        assert_eq!(editor_b.horizontal_offset, 6);
        assert!(app.workspace.documents.get(aid).unwrap().editor.modified);
        assert_eq!(app.workspace.documents.len(), 2);
        assert!(!app.watcher_active);
        assert_eq!(app.config.theme_scheme(), "light");
        assert!(app.config.preview.syntax_theme.is_none());
        assert!(app.preview_state.current_path.is_none());
        app.dismiss_overlay();
        assert_eq!(app.workspace.documents.active_id(), Some(aid));
        assert!(app.editor_visible());
        let (tx, _rx) = crate::event::event_channel(Default::default());
        handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            &tx,
        );
        assert!(app
            .workspace
            .documents
            .get(aid)
            .unwrap()
            .text()
            .starts_with("dirtyx"));
        let c = dir.path().join("c.txt");
        std::fs::write(&c, "exceeds new limit").unwrap();
        assert!(!app.open_document_path(&c, true));
        assert_eq!(app.workspace.documents.len(), 2);
        task3_modify(
            &mut app,
            "watcher",
            "enabled",
            SettingValueKind::Bool(false),
        );
        apply_settings_live(&mut app);
        assert!(!app.toggle_watcher());
    }

    #[test]
    fn task3_live_selected_preview_wrap_does_not_touch_retained_editor() {
        use crate::components::settings::SettingValueKind;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "retained").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.open_document_path(&path, true);
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        app.show_selected_preview();
        app.set_overlay(AppMode::Help);
        task3_modify(
            &mut app,
            "preview",
            "line_wrap",
            SettingValueKind::Bool(true),
        );
        apply_settings_live(&mut app);
        assert!(app.preview_state.line_wrap);
        assert!(!app.workspace.documents.active().unwrap().editor.line_wrap);
        assert_eq!(
            app.right_panel_presentation,
            crate::app::RightPanelPresentation::SelectedPreview
        );
    }

    #[test]
    fn task3_profile_conflict_rolls_back_keymap_config_pending_and_root_focus() {
        use crate::components::settings::SettingValueKind;
        use crate::keymap::{
            BindingOverride, FocusContext, KeymapConfig, KeymapProfile, Resolution,
        };
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::AppConfig {
            keymap: KeymapConfig {
                profile: Some(KeymapProfile::Web),
                bindings: Some(vec![BindingOverride {
                    command: "document.save".into(),
                    context: FocusContext::Editor,
                    keys: vec!["Ctrl+T".into()],
                }]),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut app = App::new(dir.path(), config).unwrap();
        app.set_overlay(AppMode::Help);
        let focus = app.workspace.focus.panel;
        assert!(matches!(
            app.keymap.feed(
                FocusContext::Tree,
                KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT),
                0
            ),
            Resolution::Consumed
        ));
        task3_modify(
            &mut app,
            "keymap",
            "profile",
            SettingValueKind::Enum("standard".into(), vec![]),
        );
        let path = dir.path().join("config.toml");
        save_settings_to_file(&mut app, &path);
        assert!(!path.exists());
        assert_eq!(app.config.keymap.profile, Some(KeymapProfile::Web));
        assert_eq!(app.workspace.focus.panel, focus);
        assert_eq!(app.workspace.focus.overlay, AppMode::Help);
        assert!(matches!(
            app.keymap.feed(
                FocusContext::Tree,
                KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE),
                1
            ),
            Resolution::Command(crate::commands::CommandId::Commands)
        ));
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("not saved or applied"));
    }

    #[test]
    fn task3_persistence_failure_retains_edits_and_live_state() {
        use crate::components::settings::SettingValueKind;
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.set_overlay(AppMode::Help);
        task3_modify(
            &mut app,
            "general",
            "show_hidden",
            SettingValueKind::Bool(true),
        );
        let path = dir.path().join("directory");
        std::fs::create_dir(&path).unwrap();
        save_settings_to_file(&mut app, &path);
        assert!(!app.config.show_hidden());
        assert!(
            app.help_state
                .settings_state
                .as_ref()
                .unwrap()
                .modified_count()
                > 0
        );
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("not saved or applied"));
        assert!(path.is_dir());
    }

    #[test]
    fn task3_successful_partial_save_retains_binding_overrides_and_focus() {
        use crate::components::settings::SettingValueKind;
        use crate::keymap::{BindingOverride, FocusContext};
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::AppConfig::default();
        config.keymap.bindings = Some(vec![BindingOverride {
            command: "document.save".into(),
            context: FocusContext::Editor,
            keys: vec!["F9".into()],
        }]);
        config.terminal.enabled = Some(false);
        let mut app = App::new(dir.path(), config).unwrap();
        app.set_overlay(AppMode::Help);
        task3_modify(
            &mut app,
            "keymap",
            "profile",
            SettingValueKind::Enum("web".into(), vec![]),
        );
        task3_modify(
            &mut app,
            "terminal",
            "scrollback_lines",
            SettingValueKind::UInt(2),
        );
        let path = dir.path().join("nested/config.toml");
        save_settings_to_file(&mut app, &path);
        assert_eq!(
            app.keymap
                .binding_labels(crate::commands::CommandId::Save, FocusContext::Editor),
            vec!["F9"]
        );
        assert_eq!(app.terminal_state.emulator.scrollback_limit(), 2);
        assert!(!app.workspace.layout.terminal_visible() && app.terminal_state.pty.is_none());
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
        assert!(!app.config.terminal_enabled());
        assert_eq!(
            app.help_state
                .settings_state
                .as_ref()
                .unwrap()
                .modified_count(),
            0
        );
        let saved: crate::config::AppConfig =
            toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            saved.keymap.profile,
            Some(crate::keymap::KeymapProfile::Web)
        );
    }

    #[test]
    fn task3_failed_profile_save_retains_live_profile_and_file() {
        use crate::components::settings::SettingValueKind;
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[general]\nshow_hidden = false\n").unwrap();
        let before = std::fs::read(&path).unwrap();
        task3_modify(
            &mut app,
            "keymap",
            "profile",
            SettingValueKind::Enum("invalid".into(), vec![]),
        );
        save_settings_to_file(&mut app, &path);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(app.config.keymap.profile.is_none());
        assert!(
            app.help_state
                .settings_state
                .as_ref()
                .unwrap()
                .modified_count()
                > 0
        );
        assert!(app.status_message.as_ref().unwrap().0.contains("not saved"));
    }

    #[test]
    fn task3_invalid_existing_config_not_silently_replaced() {
        use crate::components::settings::SettingValueKind;
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "malformed [").unwrap();
        task3_modify(
            &mut app,
            "general",
            "show_hidden",
            SettingValueKind::Bool(true),
        );
        save_settings_to_file(&mut app, &path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "malformed [");
        assert!(!app.config.show_hidden());
    }

    #[test]
    fn task3_live_history_setting_shrinks_and_clamps_selection() {
        use crate::components::settings::SettingValueKind;
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.terminal_state.emulator.process(&b"line\r\n".repeat(80));
        app.terminal_state.scroll_offset = 50;
        app.terminal_state
            .selection
            .set_anchor(crate::terminal::TerminalCoord { line: 0, col: 0 });
        task3_modify(
            &mut app,
            "terminal",
            "scrollback_lines",
            SettingValueKind::UInt(2),
        );
        apply_settings_live(&mut app);
        assert_eq!(app.terminal_state.emulator.scrollback_len(), 2);
        assert_eq!(app.terminal_state.scroll_offset, 2);
        assert!(!app.terminal_state.selection.is_active());
        assert!(app.terminal_state.pty.is_none());
    }

    #[test]
    fn task3_live_wrap_changes_only_origin_document_geometry() {
        use crate::components::settings::SettingValueKind;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "abcdefghijk").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.open_document_path(&path, true);
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .update_viewport(4, 3);
        app.set_overlay(AppMode::Help);
        task3_modify(
            &mut app,
            "preview",
            "line_wrap",
            SettingValueKind::Bool(true),
        );
        apply_settings_live(&mut app);
        let editor = &app.workspace.documents.active().unwrap().editor;
        assert!(editor.line_wrap);
        assert_eq!(editor.visual_row_count(), 3);
    }

    #[test]
    fn keymap_raw_terminal_escape_and_ctrl_c_leave_focus_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        for key in [
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            handle_key(&mut app, key);
            assert_eq!(app.workspace.focus.panel, FocusedPanel::Terminal);
            assert!(!app.should_quit);
        }
    }

    #[test]
    fn keymap_removed_save_and_quick_open_are_not_legacy_active() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "alpha").unwrap();
        let cfg: crate::config::AppConfig = toml::from_str(
            r#"
[[keymap.bindings]]
command = "document.save"
context = "editor"
keys = []
[[keymap.bindings]]
command = "navigation.quick_open"
context = "tree"
keys = []
"#,
        )
        .unwrap();
        let mut app = App::new(dir.path(), cfg).unwrap();
        app.open_document_path(&path, true);
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .insert_text("dirty")
            .unwrap();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha");
        app.workspace.focus.panel = FocusedPanel::Tree;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
    }

    #[test]
    fn keymap_live_apply_transactional_and_resets_prefix() {
        use crate::keymap::{FocusContext, KeymapConfig, KeymapProfile, Resolution};
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), Default::default()).unwrap();
        assert_eq!(
            app.keymap.feed(
                FocusContext::Editor,
                KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT),
                0
            ),
            Resolution::Consumed
        );
        let invalid = KeymapConfig {
            timeout_ms: Some(0),
            ..Default::default()
        };
        assert!(app.apply_keymap_config(invalid).is_err());
        assert_eq!(
            app.keymap.feed(
                FocusContext::Editor,
                KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
                1
            ),
            Resolution::Command(crate::commands::CommandId::Save)
        );
        app.keymap.feed(
            FocusContext::Editor,
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT),
            2,
        );
        app.apply_keymap_config(KeymapConfig {
            profile: Some(KeymapProfile::Web),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(app.config.keymap.profile, Some(KeymapProfile::Web));
        assert_eq!(
            app.keymap.feed(
                FocusContext::Tree,
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
                3
            ),
            Resolution::Consumed
        );
        assert_eq!(
            app.keymap.feed(
                FocusContext::Editor,
                KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
                4
            ),
            Resolution::Forward
        );
    }
    #[test]
    fn keymap_unbound_modified_tree_key_never_becomes_plain_rename() {
        let (_dir, mut app) = setup_app();
        app.config.keymap = toml::from_str::<crate::config::AppConfig>(
            r#"
[[keymap.bindings]]
command = "document.reveal"
context = "tree"
keys = []
"#,
        )
        .unwrap()
        .keymap;
        app.keymap = crate::keymap::Keymap::compile(&app.config.keymap).unwrap();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('r'), KeyModifiers::ALT),
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
    }
    #[test]
    fn keymap_focus_roundtrip_resets_pending_prefix() {
        let (_dir, mut app) = setup_app();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('g'), KeyModifiers::ALT),
        );
        app.focus_right();
        app.focus_left();
        handle_key(&mut app, make_key(KeyCode::Char('o')));
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
    }
    #[test]
    fn keymap_f8_menu_toggle_returns_terminal_without_nested_frames() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        handle_key(&mut app, make_key(KeyCode::F(8)));
        assert_eq!(app.workspace.focus.overlay, AppMode::CommandMenu);
        handle_key(&mut app, make_key(KeyCode::F(8)));
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Terminal);
    }
    #[test]
    fn keymap_unreserved_shell_alt_chords_preserve_escape_encoding() {
        assert_eq!(
            key_event_to_bytes(
                &make_key_with_modifiers(KeyCode::Char('b'), KeyModifiers::ALT),
                false,
            ),
            b"\x1bb"
        );
        assert_eq!(
            key_event_to_bytes(
                &make_key_with_modifiers(
                    KeyCode::Char('a'),
                    KeyModifiers::ALT | KeyModifiers::CONTROL
                ),
                false,
            ),
            b"\x1b\x01"
        );
    }
    #[test]
    fn keymap_decckm_selects_ss3_cursor_sequences() {
        for (code, normal, application) in [
            (KeyCode::Up, b"\x1b[A" as &[u8], b"\x1bOA" as &[u8]),
            (KeyCode::Down, b"\x1b[B" as &[u8], b"\x1bOB" as &[u8]),
            (KeyCode::Right, b"\x1b[C" as &[u8], b"\x1bOC" as &[u8]),
            (KeyCode::Left, b"\x1b[D" as &[u8], b"\x1bOD" as &[u8]),
            (KeyCode::Home, b"\x1b[H" as &[u8], b"\x1bOH" as &[u8]),
            (KeyCode::End, b"\x1b[F" as &[u8], b"\x1bOF" as &[u8]),
        ] {
            assert_eq!(key_event_to_bytes(&make_key(code), false), normal);
            assert_eq!(key_event_to_bytes(&make_key(code), true), application);
            // Alt prefix composes with either encoding.
            let mut alt = vec![0x1b];
            alt.extend_from_slice(application);
            assert_eq!(
                key_event_to_bytes(&make_key_with_modifiers(code, KeyModifiers::ALT), true),
                alt
            );
        }
        // Non-cursor keys ignore the mode entirely.
        assert_eq!(key_event_to_bytes(&make_key(KeyCode::Tab), true), b"\t");
        assert_eq!(
            key_event_to_bytes(&make_key(KeyCode::Esc), true),
            vec![0x1b]
        );
    }
    #[test]
    fn keymap_bracketed_paste_wraps_only_when_child_enabled_it() {
        assert_eq!(terminal_paste_bytes("a\nb", false), b"a\nb");
        assert_eq!(
            terminal_paste_bytes("a\nb", true),
            b"\x1b[200~a\nb\x1b[201~"
        );
        // Multiline control characters stay literal inside the markers.
        assert_eq!(
            terminal_paste_bytes("x\x1b[200~injected", true),
            b"\x1b[200~x\x1b[200~injected\x1b[201~"
        );
    }
    #[test]
    fn keymap_paste_modal_and_document_changes_cancel_without_tree_suffix() {
        let (dir, mut app) = setup_app();
        let path = dir.path().join("owned.txt");
        std::fs::write(&path, "alpha").unwrap();
        app.open_document_path(&path, true);
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('g'), KeyModifiers::ALT),
        );
        handle_paste_event(&mut app, "literal");
        app.workspace.focus.panel = FocusedPanel::Tree;
        handle_key(&mut app, make_key(KeyCode::Char('d')));
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha");
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('g'), KeyModifiers::ALT),
        );
        app.open_dialog(DialogKind::CreateFile);
        app.close_dialog();
        handle_key(&mut app, make_key(KeyCode::Char('d')));
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('g'), KeyModifiers::ALT),
        );
        app.activate_document(app.workspace.documents.active_id().unwrap());
        handle_key(&mut app, make_key(KeyCode::Char('s')));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha");
        assert!(app.workspace.documents.active().unwrap().editor.modified);
    }
    #[test]
    fn keymap_terminal_menu_save_close_captures_dirty_owned_target() {
        let dir = tempfile::tempdir().unwrap();
        let a_path = dir.path().join("a.txt");
        let b_path = dir.path().join("b.txt");
        std::fs::write(&a_path, "alpha").unwrap();
        std::fs::write(&b_path, "beta").unwrap();
        let cfg = crate::config::AppConfig {
            keymap: crate::keymap::KeymapConfig {
                profile: Some(crate::keymap::KeymapProfile::Web),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut app = App::new(dir.path(), cfg).unwrap();
        app.open_document_path(&a_path, true);
        let a = app.workspace.documents.active_id().unwrap();
        app.workspace
            .documents
            .get_mut(a)
            .unwrap()
            .editor
            .insert_text("dirty")
            .unwrap();
        app.open_document_path(&b_path, true);
        let b = app.workspace.documents.active_id().unwrap();
        app.workspace
            .documents
            .get_mut(b)
            .unwrap()
            .editor
            .insert_text("other")
            .unwrap();
        app.activate_document(a);
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('g'), KeyModifiers::ALT),
        );
        handle_key(&mut app, make_key(KeyCode::Char('m')));
        app.workspace.documents.activate(b).unwrap();
        handle_paste_event(&mut app, "document.save");
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert_eq!(std::fs::read_to_string(&a_path).unwrap(), "dirtyalpha");
        assert_eq!(std::fs::read_to_string(&b_path).unwrap(), "beta");
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Terminal);
        app.workspace
            .documents
            .get_mut(a)
            .unwrap()
            .editor
            .insert_text("again")
            .unwrap();
        handle_key(&mut app, make_key(KeyCode::F(8)));
        app.workspace.documents.activate(b).unwrap();
        handle_paste_event(&mut app, "document.close");
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert_ne!(app.workspace.focus.overlay, AppMode::Normal);
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert!(app.workspace.documents.get(a).unwrap().editor.modified);
        assert!(app.workspace.documents.get(b).unwrap().editor.modified);
        assert_eq!(app.workspace.documents.active_id(), Some(a));
    }
    #[test]
    fn keymap_disabled_and_save_conflict_safety_cannot_be_bypassed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "alpha").unwrap();
        let mut app = App::new(dir.path(), Default::default()).unwrap();
        app.open_document_path(&path, true);
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .insert_text("dirty")
            .unwrap();
        std::fs::write(&path, "external").unwrap();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('g'), KeyModifiers::ALT),
        );
        handle_key(&mut app, make_key(KeyCode::Char('s')));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
        assert_ne!(app.workspace.focus.overlay, AppMode::Normal);
        app.close_dialog();
        let keymap: crate::config::AppConfig = toml::from_str(
            r#"
[[keymap.bindings]]
command = "recovery.restore"
context = "editor"
keys = ["F9"]
"#,
        )
        .unwrap();
        app.apply_keymap_config(keymap.keymap).unwrap();
        handle_key(&mut app, make_key(KeyCode::F(9)));
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("not implemented"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn keymap_exact_shell_bytes_reach_isolated_raw_cat_in_both_profiles() {
        use std::os::unix::fs::PermissionsExt;
        struct RunningApp(App);
        impl Drop for RunningApp {
            fn drop(&mut self) {
                self.0.shutdown_terminal();
            }
        }
        for profile in [
            crate::keymap::KeymapProfile::Standard,
            crate::keymap::KeymapProfile::Web,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let runner = dir.path().join("raw-cat");
            std::fs::write(
                &runner,
                "#!/bin/sh\nstty raw -echo\nprintf 'CAT_READY'\nexec /bin/cat\n",
            )
            .unwrap();
            std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o700)).unwrap();
            let cfg = crate::config::AppConfig {
                keymap: crate::keymap::KeymapConfig {
                    profile: Some(profile),
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut running = RunningApp(App::new(dir.path(), cfg).unwrap());
            let app = &mut running.0;
            let (tx, mut rx) = crate::event::event_channel(Default::default());
            app.terminal_state.pty = Some(
                crate::terminal::pty::PtyProcess::spawn(
                    runner.to_str().unwrap(),
                    dir.path(),
                    24,
                    80,
                    tx,
                )
                .unwrap(),
            );
            if !app.workspace.layout.terminal_visible() {
                app.workspace.layout.toggle_terminal();
            }
            app.workspace.focus.panel = FocusedPanel::Terminal;
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                let mut ready = vec![];
                while let Some(event) = rx.recv().await {
                    let bytes = match event {
                        Event::TerminalOutput { data, .. } => data,
                        Event::TerminalInputComplete {
                            outcome: crate::terminal::pty::InputOutcome::Written,
                            ..
                        } => continue,
                        _ => break,
                    };
                    ready.extend(bytes);
                    assert!(ready.len() < 65536);
                    if ready.windows(9).any(|b| b == b"CAT_READY") {
                        return;
                    }
                }
                panic!("raw cat exited before ready");
            })
            .await
            .unwrap();
            for key in [
                make_key(KeyCode::Char('q')),
                make_key(KeyCode::Tab),
                make_key(KeyCode::Esc),
                make_key_with_modifiers(KeyCode::Char('c'), KeyModifiers::CONTROL),
                make_key_with_modifiers(KeyCode::Char('a'), KeyModifiers::CONTROL),
                make_key_with_modifiers(KeyCode::Char('e'), KeyModifiers::CONTROL),
                make_key_with_modifiers(KeyCode::Char('u'), KeyModifiers::CONTROL),
                make_key_with_modifiers(KeyCode::Char('k'), KeyModifiers::CONTROL),
                make_key_with_modifiers(KeyCode::Char('w'), KeyModifiers::CONTROL),
            ] {
                handle_key(app, key);
            }
            let expected = b"q\t\x1b\x03\x01\x05\x15\x0b\x17";
            let output = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                let mut output = vec![];
                while let Some(event) = rx.recv().await {
                    let bytes = match event {
                        Event::TerminalOutput { data, .. } => data,
                        Event::TerminalInputComplete {
                            outcome: crate::terminal::pty::InputOutcome::Written,
                            ..
                        } => continue,
                        _ => break,
                    };
                    output.extend(bytes);
                    if output.len() >= expected.len() {
                        return output;
                    }
                }
                output
            })
            .await
            .unwrap();
            assert_eq!(output, expected);
            assert_eq!(app.workspace.focus.panel, FocusedPanel::Terminal);
            assert!(!app.should_quit);
        }
    }
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

    fn make_event_tx() -> crate::event::EventSender {
        let (tx, _rx) = crate::event::event_channel(Default::default());
        tx
    }

    #[test]
    fn transport_copy_dismiss_is_consumer_owned_not_a_self_enqueued_completion() {
        let (_dir, mut app) = setup_app();
        app.config.general.mouse = Some(true);
        let origin = app.workspace.focus.panel;
        let (tx, mut rx) = crate::event::event_channel(Default::default());
        let mut output = Vec::new();
        app.show_copyable_text("literal".into(), &mut output, false);
        handle_key_event(&mut app, make_key(KeyCode::Esc), &tx);
        assert!(
            rx.try_recv().is_err(),
            "consumer dismissal must not send to its own queue"
        );
        assert_eq!(app.workspace.focus.panel, origin);
        app.restore_copy_mouse_capture(&mut output);
        assert!(output
            .windows(b"\x1b[?1000h".len())
            .any(|w| w == b"\x1b[?1000h"));
    }

    #[test]
    fn transport_stdin_full_and_closed_keys_paste_report_zero_admission_without_blocking() {
        let (dir, mut app) = setup_app();
        let (tx, mut rx) = crate::event::event_channel(crate::event::TransportLimits {
            slots: 1,
            ..Default::default()
        });
        tx.blocking_send(Event::Paste("occupied".into())).unwrap();
        let pty =
            crate::terminal::pty::PtyProcess::spawn("/bin/sh", dir.path(), 24, 80, tx.clone())
                .unwrap();
        for _ in 0..64 {
            pty.write(b"").unwrap();
        }
        app.terminal_state.pty = Some(pty);
        app.workspace.focus.panel = FocusedPanel::Terminal;
        handle_terminal_keys(&mut app, make_key(KeyCode::Char('x')), &tx);
        let key_status = app
            .status_message
            .as_ref()
            .map(|(message, _)| message.clone());
        handle_paste_event(&mut app, "whole literal paste");
        let paste_status = app
            .status_message
            .as_ref()
            .map(|(message, _)| message.clone());
        app.open_terminal_at_selected(&tx);
        let command_status = app
            .status_message
            .as_ref()
            .map(|(message, _)| message.clone());
        app.terminal_state.pty.as_ref().unwrap().shutdown();
        handle_terminal_keys(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &tx,
        );
        let closed_status = app
            .status_message
            .as_ref()
            .map(|(message, _)| message.clone());
        app.shutdown_terminal();
        rx.close();
        assert!(
            key_status
                .unwrap()
                .contains("queue full: no bytes accepted"),
            "key Full must not be silently discarded"
        );
        assert!(paste_status
            .unwrap()
            .contains("queue full: no bytes accepted"));
        assert!(
            command_status
                .unwrap()
                .contains("queue full: no bytes accepted"),
            "commands must report admission reason"
        );
        assert!(closed_status.unwrap().contains("closed: no bytes accepted"));
    }

    #[test]
    fn transport_full_consumer_queue_cannot_block_copy_capture_restoration() {
        let (_dir, mut app) = setup_app();
        app.config.general.mouse = Some(true);
        let (tx, mut rx) = crate::event::event_channel(crate::event::TransportLimits {
            slots: 1,
            ..Default::default()
        });
        tx.blocking_send(Event::Paste("already queued".into()))
            .unwrap();
        let mut output = Vec::new();
        app.show_copyable_text("copy".into(), &mut output, false);
        handle_key_event(&mut app, make_key(KeyCode::Esc), &tx);
        app.restore_copy_mouse_capture(&mut output);
        assert!(matches!(rx.try_recv(), Ok(Event::Paste(text)) if text == "already queued"));
        assert!(rx.try_recv().is_err());
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
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
        app.config.watcher.enabled = Some(true);
        app.watcher_active = true;
        (dir, app)
    }

    fn lifecycle_documents(
        app: &mut App,
        dir: &TempDir,
        count: usize,
    ) -> Vec<crate::workspace::documents::DocumentId> {
        (0..count)
            .map(|n| {
                let path = dir.path().join(format!("doc-{n}"));
                fs::write(&path, "original").unwrap();
                let id = app
                    .workspace
                    .documents
                    .open(&path, crate::workspace::documents::OpenDisposition::Pinned)
                    .unwrap();
                app.workspace
                    .documents
                    .get_mut(id)
                    .unwrap()
                    .editor
                    .insert_text("dirty")
                    .unwrap();
                id
            })
            .collect()
    }

    #[test]
    fn document_lifecycle_quit_captured_ids_save_discard_cancel() {
        let (dir, mut app) = setup_app();
        let ids = lifecycle_documents(&mut app, &dir, 3);
        app.quit();
        app.workspace.documents.activate(ids[2]).unwrap();
        handle_key(&mut app, make_key(KeyCode::Char('s')));
        assert!(!app.workspace.documents.get(ids[0]).unwrap().editor.modified);
        assert!(app.workspace.documents.get(ids[2]).unwrap().editor.modified);
        handle_key(&mut app, make_key(KeyCode::Char('d')));
        assert!(app.workspace.documents.get(ids[1]).is_none());
        handle_key(&mut app, make_key(KeyCode::Char('c')));
        assert!(!app.should_quit);
        assert!(app.workspace.documents.get(ids[2]).unwrap().editor.modified);
        app.quit();
        handle_key(&mut app, make_key(KeyCode::Char('s')));
        assert!(app.should_quit);
        assert_eq!(
            fs::read_to_string(dir.path().join("doc-1")).unwrap(),
            "original"
        );
    }

    #[test]
    fn document_lifecycle_quit_middle_failure_halts_without_discarding_remaining() {
        let (dir, mut app) = setup_app();
        let ids = lifecycle_documents(&mut app, &dir, 3);
        fs::write(dir.path().join("doc-1"), "external").unwrap();
        app.quit();
        handle_key(&mut app, make_key(KeyCode::Char('s')));
        handle_key(&mut app, make_key(KeyCode::Char('s')));
        assert!(!app.should_quit);
        assert!(!app.workspace.documents.get(ids[0]).unwrap().editor.modified);
        assert!(app.workspace.documents.get(ids[1]).unwrap().editor.modified);
        assert!(app.workspace.documents.get(ids[2]).unwrap().editor.modified);
        assert_eq!(
            fs::read_to_string(dir.path().join("doc-1")).unwrap(),
            "external"
        );
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert!(app.workspace.documents.get(ids[2]).is_some());
    }

    #[test]
    fn document_lifecycle_quit_failure_recovery_does_not_resume_or_acknowledge_other_docs() {
        let (dir, mut app) = setup_app();
        let ids = lifecycle_documents(&mut app, &dir, 3);
        app.workspace
            .documents
            .mark_external_change(ids[2])
            .unwrap();
        fs::write(dir.path().join("doc-1"), "external").unwrap();
        app.quit();
        handle_key(&mut app, make_key(KeyCode::Char('s')));
        handle_key(&mut app, make_key(KeyCode::Char('s')));
        app.workspace.documents.activate(ids[2]).unwrap();
        handle_key(&mut app, make_key(KeyCode::Char('o')));
        handle_key(&mut app, make_key(KeyCode::Char('y')));
        assert!(!app.should_quit);
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert!(!app.workspace.documents.get(ids[1]).unwrap().editor.modified);
        assert!(!app
            .workspace
            .documents
            .get(ids[1])
            .unwrap()
            .has_external_change());
        assert!(app.workspace.documents.get(ids[2]).unwrap().editor.modified);
        assert!(app
            .workspace
            .documents
            .get(ids[2])
            .unwrap()
            .has_external_change());
        assert_eq!(
            fs::read_to_string(dir.path().join("doc-2")).unwrap(),
            "original"
        );
    }

    #[test]
    fn document_lifecycle_close_cancel_failure_discard_and_clean_adjacent() {
        let (dir, mut app) = setup_app();
        let ids = lifecycle_documents(&mut app, &dir, 3);
        app.workspace.documents.activate(ids[1]).unwrap();
        let close = make_key_with_modifiers(KeyCode::Char('q'), KeyModifiers::ALT);
        handle_key(&mut app, close);
        app.workspace.documents.activate(ids[2]).unwrap();
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert!(app.workspace.documents.get(ids[1]).unwrap().editor.modified);
        app.workspace.documents.activate(ids[1]).unwrap();
        handle_key(&mut app, close);
        fs::remove_file(dir.path().join("doc-1")).unwrap();
        handle_key(&mut app, make_key(KeyCode::Char('s')));
        assert!(app.workspace.documents.get(ids[1]).is_some());
        assert!(!dir.path().join("doc-1").exists());
        handle_key(&mut app, make_key(KeyCode::Esc));
        handle_key(&mut app, close);
        handle_key(&mut app, make_key(KeyCode::Char('d')));
        assert!(app.workspace.documents.get(ids[1]).is_none());
        assert_eq!(app.workspace.documents.active_id(), Some(ids[2]));
        app.workspace
            .documents
            .get_mut(ids[2])
            .unwrap()
            .editor
            .save()
            .unwrap();
        handle_key(&mut app, close);
        assert_eq!(app.workspace.documents.active_id(), Some(ids[0]));
        assert!(!app.should_quit);
    }

    #[test]
    fn document_lifecycle_save_as_rekeys_and_refuses_owned_missing_target() {
        let (dir, mut app) = setup_app();
        let ids = lifecycle_documents(&mut app, &dir, 2);
        fs::remove_file(dir.path().join("doc-0")).unwrap();
        assert!(app.save_editor_as("doc-0", false, false).is_err());
        assert!(!dir.path().join("doc-0").exists());
        assert!(app.workspace.documents.get(ids[1]).unwrap().editor.modified);
        app.save_editor_as("new-name", false, false).unwrap();
        let d = app.workspace.documents.get(ids[1]).unwrap();
        assert_eq!(d.path(), dir.path().join("new-name"));
        assert_eq!(d.title(), "new-name");
        assert_eq!(d.editor.file_path, dir.path().join("new-name"));
        assert_eq!(
            app.workspace
                .documents
                .open(
                    &dir.path().join("./new-name"),
                    crate::workspace::documents::OpenDisposition::Pinned
                )
                .unwrap(),
            ids[1]
        );
        assert!(app.workspace.documents.get(ids[0]).is_some());
    }

    #[test]
    fn document_lifecycle_owned_file_rename_and_undo_preserve_history_and_save_target() {
        let (dir, mut app) = setup_app();
        let id = lifecycle_documents(&mut app, &dir, 1)[0];
        let original = dir.path().join("doc-0");
        let revision = app
            .workspace
            .documents
            .get(id)
            .unwrap()
            .editor
            .content_revision();
        let history = app
            .workspace
            .documents
            .get(id)
            .unwrap()
            .editor
            .undo_stack
            .len();
        execute_input_operation(
            &mut app,
            &DialogKind::Rename {
                original: original.clone(),
            },
            "renamed",
        );
        let d = app.workspace.documents.get(id).unwrap();
        assert_eq!(d.title(), "renamed");
        assert_eq!(d.editor.content_revision(), revision);
        assert_eq!(d.editor.undo_stack.len(), history);
        assert!(d.editor.modified);
        assert!(!d.has_external_change());
        app.save_editor_buffer().unwrap();
        assert!(!original.exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("renamed")).unwrap(),
            "dirtyoriginal"
        );
        app.workspace
            .documents
            .get_mut(id)
            .unwrap()
            .editor
            .insert_char('!');
        app.undo();
        assert_eq!(app.workspace.documents.get(id).unwrap().path(), original);
        app.save_editor_buffer().unwrap();
        assert!(!dir.path().join("renamed").exists());
        assert_eq!(
            app.workspace
                .documents
                .open(
                    &original,
                    crate::workspace::documents::OpenDisposition::Pinned
                )
                .unwrap(),
            id
        );
    }

    #[test]
    fn document_lifecycle_folder_rename_undo_keeps_existing_conflict_and_all_buffers() {
        let (dir, mut app) = setup_app();
        let folder = dir.path().join("alpha");
        let mut ids = Vec::new();
        for name in ["a", "b"] {
            let path = folder.join(name);
            fs::write(&path, "old").unwrap();
            let id = app
                .workspace
                .documents
                .open(&path, crate::workspace::documents::OpenDisposition::Pinned)
                .unwrap();
            app.workspace
                .documents
                .get_mut(id)
                .unwrap()
                .editor
                .insert_char('x');
            ids.push(id);
        }
        let baseline = app
            .workspace
            .documents
            .get(ids[0])
            .unwrap()
            .editor
            .source_revision
            .clone();
        app.workspace
            .documents
            .mark_external_change(ids[0])
            .unwrap();
        execute_input_operation(
            &mut app,
            &DialogKind::Rename {
                original: folder.clone(),
            },
            "moved",
        );
        assert_eq!(
            app.workspace.documents.get(ids[0]).unwrap().path(),
            dir.path().join("moved/a")
        );
        assert_eq!(
            app.workspace
                .documents
                .get(ids[0])
                .unwrap()
                .editor
                .source_revision,
            baseline
        );
        app.workspace.documents.activate(ids[0]).unwrap();
        assert!(app.save_editor_buffer().is_err());
        handle_key(&mut app, make_key(KeyCode::Esc));
        app.undo();
        for (id, name) in ids.iter().zip(["a", "b"]) {
            assert_eq!(
                app.workspace.documents.get(*id).unwrap().path(),
                folder.join(name)
            );
            assert!(app.workspace.documents.get(*id).unwrap().editor.modified);
        }
        assert!(app
            .workspace
            .documents
            .get(ids[0])
            .unwrap()
            .has_external_change());
        assert_eq!(
            app.workspace
                .documents
                .get(ids[0])
                .unwrap()
                .editor
                .source_revision,
            baseline
        );
        app.workspace.documents.activate(ids[1]).unwrap();
        app.save_editor_buffer().unwrap();
        app.workspace.documents.activate(ids[0]).unwrap();
        assert!(app.save_editor_buffer().is_err());
        app.reload_editor_buffer();
        assert!(!app
            .workspace
            .documents
            .get(ids[0])
            .unwrap()
            .has_external_change());
        assert_eq!(app.workspace.documents.get(ids[0]).unwrap().text(), "old");
    }

    #[test]
    fn document_lifecycle_delete_folder_retains_buffers_until_explicit_resolution() {
        let (dir, mut app) = setup_app();
        let folder = dir.path().join("alpha");
        let mut ids = Vec::new();
        for name in ["a", "b"] {
            let path = folder.join(name);
            fs::write(&path, "old").unwrap();
            let id = app
                .workspace
                .documents
                .open(&path, crate::workspace::documents::OpenDisposition::Pinned)
                .unwrap();
            app.workspace
                .documents
                .get_mut(id)
                .unwrap()
                .editor
                .insert_char('x');
            ids.push(id);
        }
        handle_delete_confirm(&mut app, make_key(KeyCode::Char('n')), vec![folder.clone()]);
        assert!(folder.exists());
        handle_delete_confirm(&mut app, make_key(KeyCode::Char('y')), vec![folder.clone()]);
        for id in ids {
            assert_eq!(
                app.workspace.documents.get(id).unwrap().disk_change(),
                crate::workspace::documents::DiskChange::Deleted
            );
            assert_eq!(app.workspace.documents.get(id).unwrap().text(), "xold");
            app.workspace.documents.activate(id).unwrap();
            assert!(app.save_editor_buffer().is_err());
            assert!(!folder.exists());
            handle_key(&mut app, make_key(KeyCode::Esc));
        }
        app.save_editor_as("../rescued", false, false).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("rescued")).unwrap(),
            "xold"
        );
        assert!(!folder.exists());
    }

    #[test]
    fn document_lifecycle_esc_keep_is_focus_back_not_discard_or_close() {
        let (dir, mut app) = setup_app();
        let id = lifecycle_documents(&mut app, &dir, 1)[0];
        app.workspace.focus.panel = FocusedPanel::Editor;
        handle_key(&mut app, make_key(KeyCode::Char('q')));
        assert_eq!(
            app.workspace.documents.get(id).unwrap().text(),
            "dirtyqoriginal"
        );
        assert!(!app.should_quit);
        handle_key(&mut app, make_key(KeyCode::Esc));
        handle_key(&mut app, make_key(KeyCode::Char('n')));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
        assert!(app.workspace.documents.get(id).unwrap().editor.modified);
        assert_eq!(
            fs::read_to_string(dir.path().join("doc-0")).unwrap(),
            "original"
        );
        app.workspace.focus.panel = FocusedPanel::Editor;
        handle_key(&mut app, make_key(KeyCode::Esc));
        handle_key(&mut app, make_key(KeyCode::Char('y')));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
        assert!(app.workspace.documents.get(id).is_some());
        assert!(!app.workspace.documents.get(id).unwrap().editor.modified);
        assert!(!app.should_quit);
    }

    #[test]
    fn document_lifecycle_external_failure_is_sticky_until_explicit_overwrite() {
        let (dir, mut app) = setup_app();
        let id = lifecycle_documents(&mut app, &dir, 1)[0];
        let path = dir.path().join("doc-0");
        let baseline = app
            .workspace
            .documents
            .get(id)
            .unwrap()
            .editor
            .source_revision
            .clone();
        fs::write(&path, "replacement").unwrap();
        assert!(app.save_editor_buffer().is_err());
        assert!(app
            .workspace
            .documents
            .get(id)
            .unwrap()
            .has_external_change());
        assert_eq!(
            app.workspace
                .documents
                .get(id)
                .unwrap()
                .editor
                .source_revision,
            baseline
        );
        fs::write(&path, "original").unwrap();
        assert!(app.save_editor_buffer().is_err());
        app.begin_editor_overwrite(false, false);
        let expected = match &app.workspace.focus.overlay {
            AppMode::Dialog(DialogKind::SaveOverwrite {
                expected_revision, ..
            }) => expected_revision.clone(),
            _ => panic!("expected explicit overwrite"),
        };
        app.confirm_editor_overwrite(expected.as_ref(), false, false)
            .unwrap();
        assert!(!app
            .workspace
            .documents
            .get(id)
            .unwrap()
            .has_external_change());
        assert_eq!(fs::read_to_string(&path).unwrap(), "dirtyoriginal");
    }

    #[test]
    fn document_lifecycle_external_folder_delete_marks_owned_children_without_polling_getters() {
        let (dir, mut app) = setup_app();
        let path = dir.path().join("alpha/a");
        fs::write(&path, "held").unwrap();
        let id = app
            .workspace
            .documents
            .open(&path, crate::workspace::documents::OpenDisposition::Preview)
            .unwrap();
        fs::remove_dir_all(dir.path().join("alpha")).unwrap();
        assert!(!app
            .workspace
            .documents
            .get(id)
            .unwrap()
            .has_external_change());
        app.handle_fs_change(vec![dir.path().join("alpha")]);
        assert_eq!(
            app.workspace.documents.get(id).unwrap().disk_change(),
            crate::workspace::documents::DiskChange::Deleted
        );
        assert_eq!(app.workspace.documents.get(id).unwrap().text(), "held");
        assert!(app.save_editor_buffer().is_err());
        assert!(!path.exists());
        app.reload_editor_buffer();
        assert_eq!(app.workspace.documents.get(id).unwrap().text(), "held");
        assert!(app
            .workspace
            .documents
            .get(id)
            .unwrap()
            .has_external_change());
    }

    #[test]
    fn document_lifecycle_quit_all_save_all_discard_and_cancel_multiple_dirty() {
        for decision in ['s', 'd', 'c'] {
            let (dir, mut app) = setup_app();
            let ids = lifecycle_documents(&mut app, &dir, 3);
            app.workspace.focus.panel = FocusedPanel::Preview;
            handle_key(&mut app, make_key(KeyCode::Char('q')));
            handle_paste_event(&mut app, "d");
            assert!(app.workspace.documents.get(ids[0]).is_some());
            handle_key(&mut app, make_key(KeyCode::Char(decision)));
            if decision == 'c' {
                assert!(!app.should_quit);
                for id in ids {
                    assert!(app.workspace.documents.get(id).unwrap().editor.modified);
                }
            } else {
                handle_key(&mut app, make_key(KeyCode::Char(decision)));
                assert!(!app.should_quit);
                handle_key(&mut app, make_key(KeyCode::Char(decision)));
                assert!(app.should_quit);
                if decision == 's' {
                    for id in ids {
                        assert!(!app.workspace.documents.get(id).unwrap().editor.modified);
                    }
                } else {
                    assert!(app.workspace.documents.is_empty());
                }
            }
        }
    }

    #[test]
    fn document_lifecycle_close_save_original_id_and_editor_copy_are_not_quit() {
        let (dir, mut app) = setup_app();
        let ids = lifecycle_documents(&mut app, &dir, 2);
        app.workspace.documents.activate(ids[0]).unwrap();
        app.workspace.focus.panel = FocusedPanel::Editor;
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert_eq!(
            app.workspace
                .documents
                .get(ids[0])
                .unwrap()
                .editor
                .clipboard_text(),
            "dirtyoriginal\n"
        );
        assert!(!app.should_quit);
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('q'), KeyModifiers::ALT),
        );
        app.workspace.documents.activate(ids[1]).unwrap();
        handle_key(&mut app, make_key(KeyCode::Char('s')));
        assert!(app.workspace.documents.get(ids[0]).is_none());
        assert_eq!(app.workspace.documents.active_id(), Some(ids[1]));
        assert!(app.workspace.documents.get(ids[1]).unwrap().editor.modified);
        assert_eq!(
            fs::read_to_string(dir.path().join("doc-0")).unwrap(),
            "dirtyoriginal"
        );
        assert!(!app.should_quit);
    }

    #[test]
    fn document_lifecycle_owned_missing_destination_blocks_rename_and_undo_before_disk() {
        let (dir, mut app) = setup_app();
        let ids = lifecycle_documents(&mut app, &dir, 2);
        fs::remove_file(dir.path().join("doc-1")).unwrap();
        execute_input_operation(
            &mut app,
            &DialogKind::Rename {
                original: dir.path().join("doc-0"),
            },
            "doc-1",
        );
        assert!(dir.path().join("doc-0").exists());
        assert!(!dir.path().join("doc-1").exists());
        assert_eq!(
            app.workspace.documents.get(ids[0]).unwrap().title(),
            "doc-0"
        );
        assert!(app.workspace.documents.get(ids[1]).unwrap().editor.modified);
        execute_input_operation(
            &mut app,
            &DialogKind::Rename {
                original: dir.path().join("doc-0"),
            },
            "renamed",
        );
        fs::write(dir.path().join("doc-0"), "other").unwrap();
        let other = app
            .workspace
            .documents
            .open(
                &dir.path().join("doc-0"),
                crate::workspace::documents::OpenDisposition::Pinned,
            )
            .unwrap();
        fs::remove_file(dir.path().join("doc-0")).unwrap();
        app.undo();
        assert!(dir.path().join("renamed").exists());
        assert!(!dir.path().join("doc-0").exists());
        assert!(app.last_undo.is_some());
        assert!(app.workspace.documents.get(other).is_some());
        assert_eq!(
            app.workspace.documents.get(ids[0]).unwrap().title(),
            "renamed"
        );
    }

    #[test]
    fn document_lifecycle_folder_rename_refuses_missing_owned_destination_root() {
        let (dir, mut app) = setup_app();
        let target = dir.path().join("destination");
        fs::write(&target, "owned").unwrap();
        let owned = app
            .workspace
            .documents
            .open(
                &target,
                crate::workspace::documents::OpenDisposition::Pinned,
            )
            .unwrap();
        fs::remove_file(&target).unwrap();
        fs::write(dir.path().join("alpha/a"), "source").unwrap();
        let source = app
            .workspace
            .documents
            .open(
                &dir.path().join("alpha/a"),
                crate::workspace::documents::OpenDisposition::Pinned,
            )
            .unwrap();
        execute_input_operation(
            &mut app,
            &DialogKind::Rename {
                original: dir.path().join("alpha"),
            },
            "destination",
        );
        assert!(dir.path().join("alpha/a").exists());
        assert!(!target.exists());
        assert_eq!(app.workspace.documents.get(owned).unwrap().text(), "owned");
        assert_eq!(
            app.workspace.documents.get(source).unwrap().path(),
            dir.path().join("alpha/a")
        );
    }

    #[test]
    fn document_lifecycle_failed_and_cancelled_save_as_keep_original_identity() {
        let (dir, mut app) = setup_app();
        let id = lifecycle_documents(&mut app, &dir, 1)[0];
        let original = dir.path().join("doc-0");
        assert!(app.save_editor_as("doc-0", false, false).is_err());
        assert!(app.save_editor_as("missing/new", false, false).is_err());
        assert_eq!(app.workspace.documents.get(id).unwrap().path(), original);
        assert_eq!(
            app.workspace.documents.get(id).unwrap().editor.file_path,
            original
        );
        app.open_dialog(DialogKind::EditorSaveAs {
            exit_after_save: false,
            normalize: false,
        });
        handle_paste_event(&mut app, "cancelled");
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert!(!dir.path().join("cancelled").exists());
        assert_eq!(app.workspace.documents.get(id).unwrap().path(), original);
        assert!(app.workspace.documents.get(id).unwrap().editor.modified);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn document_lifecycle_shell_q_and_control_c_reach_real_pty_not_quit() {
        struct RunningApp(App);
        impl Drop for RunningApp {
            fn drop(&mut self) {
                self.0.shutdown_terminal();
            }
        }
        let (dir, app) = setup_app();
        let mut running = RunningApp(app);
        let app = &mut running.0;
        let (tx, mut rx) = crate::event::event_channel(Default::default());
        app.terminal_state.pty = Some(
            crate::terminal::pty::PtyProcess::spawn("/bin/cat", dir.path(), 24, 80, tx).unwrap(),
        );
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        handle_key(app, make_key(KeyCode::Char('q')));
        handle_key(app, make_key(KeyCode::Enter));
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut output = Vec::new();
            while let Some(event) = rx.recv().await {
                let bytes = match event {
                    Event::TerminalOutput { data, .. } => data,
                    Event::TerminalInputComplete {
                        outcome: crate::terminal::pty::InputOutcome::Written,
                        ..
                    } => continue,
                    _ => break,
                };
                output.extend(bytes);
                assert!(output.len() < 65536);
                if output.contains(&b'q') {
                    break;
                }
            }
            output
        })
        .await
        .unwrap();
        assert!(first.contains(&b'q'));
        handle_key(
            app,
            make_key_with_modifiers(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        let interrupted = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut output = Vec::new();
            while let Some(event) = rx.recv().await {
                let bytes = match event {
                    Event::TerminalOutput { data, .. } => data,
                    Event::TerminalInputComplete {
                        outcome: crate::terminal::pty::InputOutcome::Written,
                        ..
                    } => continue,
                    _ => break,
                };
                output.extend(bytes);
                assert!(output.len() < 65536);
                if output.windows(2).any(|b| b == b"^C") {
                    break;
                }
            }
            output
        })
        .await
        .unwrap();
        assert!(interrupted.windows(2).any(|b| b == b"^C"));
        assert!(!app.should_quit);
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
    }

    #[test]
    fn clipboard_editor_keys_inline_paste_and_configured_limits() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.overlay = AppMode::Normal;
        app.workspace.focus.panel = FocusedPanel::Editor;
        let mut editor = crate::editor::EditorState::new("betaomega", "text.txt".into());
        editor.selection = Some(crate::editor::Selection::new(0, 0));
        editor.set_cursor_position_for_selection(0, 4);
        install_editor(&mut app, editor);
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(app.clipboard.is_empty());
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("internally"));
        app.workspace
            .documents
            .active_mut()
            .map(|d| &mut d.editor)
            .unwrap()
            .set_cursor_position(0, 4);
        app.config.general.max_editor_bytes = Some(10);
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('v'), KeyModifiers::CONTROL),
        );
        assert_eq!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .buffer[0],
            "betaomega"
        );
        app.config.general.max_editor_bytes = None;
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('v'), KeyModifiers::CONTROL),
        );
        assert_eq!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .buffer[0],
            "betabetaomega"
        );
        app.workspace
            .documents
            .active_mut()
            .map(|d| &mut d.editor)
            .unwrap()
            .undo();
        assert_eq!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .buffer[0],
            "betaomega"
        );
    }

    #[test]
    fn bracketed_paste_in_editor_is_literal_and_one_undo() {
        let (_dir, mut app) = setup_app();
        install_editor(
            &mut app,
            crate::editor::EditorState::new("", "config.yaml".into()),
        );
        app.workspace.focus.overlay = AppMode::Normal;
        app.workspace.focus.panel = FocusedPanel::Editor;

        handle_paste_event(&mut app, "q\n\x1b[A\n  key: value\n");

        assert_eq!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .buffer
                .join("\n"),
            "q\n\x1b[A\n  key: value\n"
        );
        assert!(!app.should_quit);
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        app.workspace
            .documents
            .active_mut()
            .map(|d| &mut d.editor)
            .unwrap()
            .undo();
        assert_eq!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .buffer
                .join("\n"),
            ""
        );
    }

    #[test]
    fn keymap_editor_find_is_raw_until_explicit_menu_focus_action() {
        let (_dir, mut app) = setup_app();
        install_editor(
            &mut app,
            crate::editor::EditorState::new("alpha", "a.txt".into()),
        );
        app.workspace.focus.panel = FocusedPanel::Editor;
        let a = app.workspace.documents.active_id().unwrap();
        app.workspace
            .documents
            .get_mut(a)
            .unwrap()
            .editor
            .open_find();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Left, KeyModifiers::CONTROL),
        );
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        handle_paste_event(&mut app, "literal q");
        assert_eq!(
            app.workspace
                .documents
                .get(a)
                .unwrap()
                .editor
                .find_state
                .query,
            "literal q"
        );
        handle_key(&mut app, make_key(KeyCode::F(8)));
        assert_eq!(app.workspace.focus.overlay, AppMode::CommandMenu);
        crate::commands::dispatch_command(&mut app, crate::commands::CommandId::FocusTree).unwrap();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
        assert_eq!(app.workspace.documents.get(a).unwrap().text(), "alpha");
        assert!(
            app.workspace
                .documents
                .get(a)
                .unwrap()
                .editor
                .find_state
                .active
        );
    }

    #[test]
    fn stage2b_editor_tab_preserves_literal_indentation() {
        let (_dir, mut app) = setup_app();
        install_editor(
            &mut app,
            crate::editor::EditorState::new("", "a.txt".into()),
        );
        app.workspace.focus.panel = FocusedPanel::Editor;
        handle_key(&mut app, make_key(KeyCode::Tab));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert!(!app.editor().unwrap().buffer[0].is_empty());
    }

    #[test]
    fn stage2b_edit_tree_b_terminal_reactivate_a_retains_history_and_view() {
        let (dir, mut app) = setup_app();
        let a_path = dir.path().join("a.txt");
        let b_path = dir.path().join("b.txt");
        fs::write(&a_path, "alpha\nsecond").unwrap();
        fs::write(&b_path, "beta").unwrap();
        app.tree_state.reload_dir(dir.path());
        let a = app
            .workspace
            .documents
            .open(
                &a_path,
                crate::workspace::documents::OpenDisposition::Pinned,
            )
            .unwrap();
        app.workspace.focus.panel = FocusedPanel::Editor;
        handle_paste_event(&mut app, "unsaved");
        let e = &mut app.workspace.documents.get_mut(a).unwrap().editor;
        e.scroll_offset = 1;
        e.horizontal_offset = 4;
        let history = e.undo_stack.len();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Left, KeyModifiers::CONTROL),
        );
        app.tree_state.selected_index = app
            .tree_state
            .flat_items
            .iter()
            .position(|i| i.path == b_path)
            .unwrap();
        app.preview_state.current_path = Some(b_path.clone());
        assert_eq!(app.workspace.documents.active_id(), Some(a));
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.focus_down();
        handle_paste_event(&mut app, "q\nterminal text");
        let b = app
            .workspace
            .documents
            .open(
                &b_path,
                crate::workspace::documents::OpenDisposition::Pinned,
            )
            .unwrap();
        app.workspace.documents.activate(a).unwrap();
        app.focus_right();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert_eq!(app.workspace.documents.active_id(), Some(a));
        let e = &app.workspace.documents.get(a).unwrap().editor;
        assert_eq!(e.buffer[0], "unsavedalpha");
        assert_eq!(e.undo_stack.len(), history);
        assert_eq!((e.scroll_offset, e.horizontal_offset), (1, 4));
        assert_eq!(app.workspace.documents.get(b).unwrap().text(), "beta");
        assert!(!app.should_quit);
    }

    #[test]
    fn stage2b_mouse_switches_panels_without_editor_capturing_other_areas() {
        let (_dir, mut app) = setup_app();
        install_editor(
            &mut app,
            crate::editor::EditorState::new("alpha", "a.txt".into()),
        );
        app.workspace.focus.panel = FocusedPanel::Editor;
        app.tree_area = ratatui::layout::Rect::new(0, 0, 20, 10);
        app.tree_content_area = ratatui::widgets::Block::bordered().inner(app.tree_area);
        app.preview_area = ratatui::layout::Rect::new(20, 0, 30, 10);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
        app.terminal_area = ratatui::layout::Rect::new(1, 11, 48, 8);
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        let a = app.workspace.documents.active_id().unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        for (column, row, expected) in [
            (2, 2, FocusedPanel::Tree),
            (2, 12, FocusedPanel::Terminal),
            (28, 2, FocusedPanel::Editor),
        ] {
            handle_mouse_event(
                &mut app,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                },
                &tx,
            );
            assert_eq!(app.workspace.focus.panel, expected);
            assert_eq!(app.workspace.documents.get(a).unwrap().text(), "alpha");
        }
        app.open_dialog(DialogKind::SaveConfirm);
        handle_mouse_event(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 2,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
            &tx,
        );
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert_eq!(app.workspace.focus.overlay_document(), Some(a));
    }

    #[test]
    fn stage2b_empty_editor_and_optional_terminal_never_route_tree_commands() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Editor;
        handle_key(&mut app, make_key(KeyCode::Char('q')));
        handle_key(&mut app, make_key(KeyCode::Char('a')));
        handle_paste_event(&mut app, "q\nliteral");
        assert!(!app.should_quit);
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert!(app.editor().is_none());
        handle_key(&mut app, make_key(KeyCode::Tab));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        handle_key(&mut app, make_key(KeyCode::Char('a')));
        handle_paste_event(&mut app, "q\nliteral");
        assert!(!app.should_quit);
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("No running terminal"));
    }

    #[test]
    fn bracketed_paste_routes_find_and_dialog_text_without_commands() {
        let (_dir, mut app) = setup_app();
        install_editor(
            &mut app,
            crate::editor::EditorState::new("needle", "text.txt".into()),
        );
        app.workspace.focus.overlay = AppMode::Normal;
        app.workspace.focus.panel = FocusedPanel::Editor;
        app.workspace
            .documents
            .active_mut()
            .map(|d| &mut d.editor)
            .unwrap()
            .open_find();

        handle_paste_event(&mut app, "needle");

        let editor = app.workspace.documents.active().map(|d| &d.editor).unwrap();
        assert_eq!(editor.find_state.query, "needle");
        assert_eq!(editor.find_state.matches, vec![(0, 0)]);
        assert_eq!(editor.buffer[0], "needle");
        assert!(!editor.modified);
        app.open_dialog(DialogKind::CreateFile);
        handle_paste_event(&mut app, "测试q.yaml");
        assert_eq!(app.dialog_state.input, "测试q.yaml");
        assert!(matches!(
            app.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::CreateFile)
        ));
        assert!(!app.should_quit);
    }

    #[test]
    fn bracketed_paste_does_not_accept_modal_confirmation_or_normal_commands() {
        let (_dir, mut app) = setup_app();
        handle_paste_event(&mut app, "q");
        assert!(!app.should_quit);
        app.workspace.focus.overlay = AppMode::Dialog(DialogKind::SaveConfirm);
        handle_paste_event(&mut app, "y");
        assert!(matches!(
            app.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::SaveConfirm)
        ));
        assert!(!app.should_quit);
    }

    #[test]
    fn bracketed_paste_respects_editor_size_limits_before_selection_replacement() {
        let (_dir, mut app) = setup_app();
        app.config.general.max_editor_bytes = Some(8);
        install_editor(
            &mut app,
            crate::editor::EditorState::new("keep", "text.txt".into()),
        );
        app.workspace.focus.overlay = AppMode::Normal;
        app.workspace.focus.panel = FocusedPanel::Editor;
        app.workspace
            .documents
            .active_mut()
            .map(|d| &mut d.editor)
            .unwrap()
            .select_all();

        handle_paste_event(&mut app, "123456789");

        assert_eq!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .selected_text(),
            "keep"
        );
        assert!(
            !app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .modified
        );
        assert!(app.status_message.as_ref().unwrap().0.contains("limit"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bracketed_paste_terminal_route_reaches_pty_without_editor_commands() {
        struct RunningApp(App);
        impl Drop for RunningApp {
            fn drop(&mut self) {
                self.0.shutdown_terminal();
            }
        }
        let (dir, app) = setup_app();
        let mut running = RunningApp(app);
        let app = &mut running.0;
        let (tx, mut rx) = crate::event::event_channel(Default::default());
        app.terminal_state.pty = Some(
            crate::terminal::pty::PtyProcess::spawn("/bin/cat", dir.path(), 24, 80, tx).unwrap(),
        );
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        app.workspace.focus.overlay = AppMode::Normal;

        handle_paste_event(app, "q\n  key: value\n");

        let output = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut output = Vec::new();
            while let Some(event) = rx.recv().await {
                let bytes = match event {
                    Event::TerminalOutput { data, .. } => data,
                    Event::TerminalInputComplete {
                        outcome: crate::terminal::pty::InputOutcome::Written,
                        ..
                    } => continue,
                    _ => break,
                };
                output.extend(bytes);
                assert!(output.len() < 65536);
                if output.windows(10).any(|bytes| bytes == b"key: value") {
                    break;
                }
            }
            output
        })
        .await
        .unwrap();
        assert!(output.windows(10).any(|bytes| bytes == b"key: value"));
        assert!(!app.should_quit);
        assert!(app.workspace.documents.active().is_none());
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
    }

    // === Normal mode tests (existing) ===

    #[test]
    fn unicode_find_fields_backspace_whole_grapheme() {
        for replace in [false, true] {
            let (_dir, mut app) = setup_app();
            install_editor(
                &mut app,
                crate::editor::EditorState::new("abc", "unused".into()),
            );
            app.workspace.focus.overlay = AppMode::Normal;
            app.workspace.focus.panel = FocusedPanel::Editor;
            {
                let e = app
                    .workspace
                    .documents
                    .active_mut()
                    .map(|d| &mut d.editor)
                    .unwrap();
                e.open_find_replace();
                e.find_state.in_replace_field = replace;
            }
            for ch in "e\u{301}👩‍💻".chars() {
                handle_key(&mut app, make_key(KeyCode::Char(ch)));
            }
            handle_key(&mut app, make_key(KeyCode::Backspace));
            let e = app.workspace.documents.active().map(|d| &d.editor).unwrap();
            assert_eq!(
                if replace {
                    &e.find_state.replacement
                } else {
                    &e.find_state.query
                },
                "e\u{301}"
            );
            handle_key(&mut app, make_key(KeyCode::Backspace));
            let e = app.workspace.documents.active().map(|d| &d.editor).unwrap();
            assert_eq!(
                if replace {
                    &e.find_state.replacement
                } else {
                    &e.find_state.query
                },
                ""
            );
        }
    }

    #[test]
    fn unicode_mouse_maps_cells_to_bytes() {
        let e = crate::editor::EditorState::new("\t中e\u{301}🙂", "unused".into());
        let area = ratatui::layout::Rect::new(0, 0, 40, 10);
        for (cell, byte) in [
            (0, 0),
            (3, 0),
            (4, 1),
            (5, 1),
            (6, 4),
            (7, 7),
            (8, 7),
            (9, 11),
        ] {
            assert_eq!(
                mouse_to_editor_pos(
                    &e,
                    ratatui::widgets::Block::bordered().inner(area),
                    4 + cell,
                    1
                ),
                (0, byte)
            );
        }
    }

    #[test]
    fn double_click_unicode_preview_selects_display_width() {
        let (_dir, mut app) = setup_app_with_preview();
        app.preview_state.content_lines = vec![ratatui::text::Line::from("\t中e\u{301}👩‍💻")];
        let tx = make_event_tx();
        handle_mouse_event(&mut app, make_mouse_down_left(25, 1), &tx);
        handle_mouse_event(&mut app, make_mouse_up_left(25, 1), &tx);
        handle_mouse_event(&mut app, make_mouse_down_left(25, 1), &tx);
        assert_eq!(app.preview_selection.normalized().unwrap().1.col, 9);
    }

    #[test]
    fn wrapped_split_glyph_arrow_and_page_keys_progress_with_selection() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.overlay = AppMode::Normal;
        app.workspace.focus.panel = FocusedPanel::Editor;
        for (content, width, glyph) in [("\tX", 2, "\t"), ("中X", 1, "中")] {
            for (down, up) in [
                (KeyCode::Down, KeyCode::Up),
                (KeyCode::PageDown, KeyCode::PageUp),
            ] {
                for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
                    let mut editor = crate::editor::EditorState::new(content, "test.txt".into());
                    editor.update_viewport(width, 1);
                    editor.toggle_wrap();
                    install_editor(&mut app, editor);
                    handle_key(&mut app, make_key_with_modifiers(down, modifiers));
                    let editor = app.workspace.documents.active().map(|d| &d.editor).unwrap();
                    assert_eq!(editor.cursor_col, glyph.len());
                    assert_eq!(editor.cursor_visual_row(), 2);
                    if modifiers == KeyModifiers::SHIFT {
                        assert_eq!(editor.selected_text(), glyph);
                    }
                    handle_key(&mut app, make_key_with_modifiers(up, modifiers));
                    let editor = app.workspace.documents.active().map(|d| &d.editor).unwrap();
                    assert_eq!(editor.cursor_col, 0);
                    assert_eq!(editor.cursor_visual_row(), 0);
                    assert!(editor.selected_text().is_empty());
                }
            }
        }
    }

    #[test]
    fn delete_key_same_byte_reflow_is_followed_on_render_viewport_update() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.overlay = AppMode::Normal;
        app.workspace.focus.panel = FocusedPanel::Editor;
        let mut editor =
            crate::editor::EditorState::new("abc中dddd\nmore\nmore", "test.txt".into());
        editor.update_viewport(4, 1);
        editor.toggle_wrap();
        editor.set_cursor_position(0, 3);
        install_editor(&mut app, editor);
        handle_key(&mut app, make_key(KeyCode::Delete));
        let editor = app
            .workspace
            .documents
            .active_mut()
            .map(|d| &mut d.editor)
            .unwrap();
        assert_eq!(editor.cursor_col, 3);
        assert_eq!(editor.buffer[0], "abcdddd");
        editor.update_viewport(4, 1);
        assert_eq!(editor.scroll_offset, editor.cursor_visual_row());
    }

    #[test]
    fn mouse_editor_maps_horizontal_and_wrapped_byte_positions() {
        let mut editor = crate::editor::EditorState::new("abc中\tZ", "test.txt".into());
        let area = ratatui::layout::Rect::new(10, 5, 9, 5);
        editor.visible_width = 4;
        editor.visible_height = 3;
        editor.horizontal_offset = 4;
        assert_eq!(
            mouse_to_editor_pos(
                &editor,
                ratatui::widgets::Block::bordered().inner(area),
                14,
                6
            ),
            (0, 3)
        );
        assert_eq!(
            mouse_to_editor_pos(
                &editor,
                ratatui::widgets::Block::bordered().inner(area),
                16,
                6
            ),
            (0, 6)
        );
        editor.line_wrap = true;
        editor.horizontal_offset = 0;
        assert_eq!(
            mouse_to_editor_pos(
                &editor,
                ratatui::widgets::Block::bordered().inner(area),
                14,
                7
            ),
            (0, 3)
        );
        assert_eq!(
            mouse_to_editor_pos(
                &editor,
                ratatui::widgets::Block::bordered().inner(area),
                16,
                7
            ),
            (0, 6)
        );
        assert_eq!(
            mouse_to_editor_pos(
                &editor,
                ratatui::widgets::Block::bordered().inner(area),
                15,
                8
            ),
            (0, 7)
        );
        editor.open_find_replace();
        assert_eq!(
            mouse_to_editor_pos(
                &editor,
                ratatui::widgets::Block::bordered().inner(area),
                14,
                8
            ),
            (0, 0)
        );
    }

    #[test]
    fn mouse_preview_maps_wrapped_and_horizontal_display_cells() {
        let (_dir, mut app) = setup_app();
        app.preview_area = ratatui::layout::Rect::new(10, 5, 6, 5);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
        app.preview_state.content_lines = vec![ratatui::text::Line::from("abc中\tZ")];
        app.preview_state.line_wrap = true;
        for (x, y, col) in [(11, 6, 0), (11, 7, 3), (13, 7, 5), (12, 8, 8)] {
            let coord = mouse_to_preview_coord(&app, x, y, false).unwrap();
            assert_eq!(coord, crate::terminal::TerminalCoord { line: 0, col });
        }
        let a = mouse_to_preview_coord(&app, 13, 7, false).unwrap();
        let b = mouse_to_preview_coord(&app, 12, 8, false).unwrap();
        assert_eq!(
            crate::text::display_slice("abc中\tZ", a.col, b.col + 1),
            "   Z"
        );
        app.preview_state.line_wrap = false;
        app.preview_state.horizontal_offset = 5;
        assert_eq!(mouse_to_preview_coord(&app, 11, 6, false).unwrap().col, 5);
        assert!(mouse_to_preview_coord(&app, 10, 6, false).is_none());
        app.preview_area.width = 0;
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
        assert!(mouse_to_preview_coord(&app, 11, 6, true).is_none());
    }

    #[test]
    fn mouse_preview_blank_rows_clamp_to_last_logical_line() {
        let (_dir, app) = setup_app_with_preview();
        let coord = mouse_to_preview_coord(&app, 25, 8, true).unwrap();
        assert_eq!(coord.line, 2);
    }

    #[test]
    fn mouse_editor_find_bar_and_zero_code_area_do_not_move_cursor() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.overlay = AppMode::Normal;
        app.workspace.focus.panel = FocusedPanel::Editor;
        app.preview_area = ratatui::layout::Rect::new(0, 0, 10, 5);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
        let mut editor = crate::editor::EditorState::new("abc\nxyz", "test.txt".into());
        editor.set_cursor_position(0, 2);
        editor.open_find_replace();
        install_editor(&mut app, editor);
        handle_editor_mouse(&mut app, make_mouse_down_left(5, 3));
        assert_eq!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .cursor_position(),
            crate::text::TextPosition { line: 0, byte: 2 }
        );
        assert!(app
            .workspace
            .documents
            .active()
            .map(|d| &d.editor)
            .unwrap()
            .selection
            .is_none());
        app.preview_area.height = 3;
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
        handle_editor_mouse(&mut app, make_mouse_down_left(5, 1));
        assert_eq!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .cursor_col,
            2
        );
    }

    #[test]
    fn viewport_navigation_keys_are_reachable_and_preserve_legacy_editing() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.preview_area = ratatui::layout::Rect::new(0, 0, 6, 4);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
        app.preview_state.content_lines = vec![ratatui::text::Line::from("abcdefghij")];
        handle_key(&mut app, make_key(KeyCode::Right));
        assert_eq!(app.preview_state.horizontal_offset, 4);
        handle_key(&mut app, make_key(KeyCode::Left));
        assert_eq!(app.preview_state.horizontal_offset, 0);
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('w'), KeyModifiers::ALT),
        );
        assert!(app.preview_state.line_wrap);
        app.workspace.focus.overlay = AppMode::Normal;
        app.workspace.focus.panel = FocusedPanel::Editor;
        install_editor(
            &mut app,
            crate::editor::EditorState::new("abc", "test.txt".into()),
        );
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('w'), KeyModifiers::ALT),
        );
        assert!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .line_wrap
        );
        handle_key(&mut app, make_key(KeyCode::Char('w')));
        assert_eq!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .buffer[0],
            "wabc"
        );
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('f'), KeyModifiers::CONTROL),
        );
        assert!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .find_state
                .active
        );
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('w'), KeyModifiers::ALT),
        );
        assert!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .line_wrap
        );
    }

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

    #[tokio::test]
    async fn key_enter_expands_directory() {
        let (_dir, mut app) = setup_app();
        handle_key(&mut app, make_key(KeyCode::Char('j')));
        assert_eq!(app.tree_state.flat_items[1].name, "alpha");
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert!(app.tree_state.flat_items[1].is_expanded);
        app.shutdown_background().await;
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

    #[tokio::test]
    async fn key_backspace_collapses_directory() {
        let (_dir, mut app) = setup_app();
        handle_key(&mut app, make_key(KeyCode::Char('j')));
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert!(app.tree_state.flat_items[1].is_expanded);
        handle_key(&mut app, make_key(KeyCode::Backspace));
        assert!(!app.tree_state.flat_items[1].is_expanded);
        app.shutdown_background().await;
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
        assert!(matches!(
            app.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::CreateFile)
        ));
    }

    #[test]
    fn key_shift_a_opens_create_dir_dialog() {
        let (_dir, mut app) = setup_app();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('A'), KeyModifiers::SHIFT),
        );
        assert!(matches!(
            app.workspace.focus.overlay,
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
            app.workspace.focus.overlay,
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
            app.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::DeleteConfirm { .. })
        ));
    }

    #[test]
    fn key_d_on_root_is_noop() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 0; // root
        handle_key(&mut app, make_key(KeyCode::Char('d')));
        assert!(matches!(app.workspace.focus.overlay, AppMode::Normal));
    }

    // === Dialog input tests ===

    #[test]
    fn dialog_esc_closes() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert!(matches!(app.workspace.focus.overlay, AppMode::Normal));
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

        let mut editor = crate::editor::EditorState::from_file(&file).unwrap();
        editor.select_all();
        for ch in "new".chars() {
            editor.insert_char(ch);
        }
        install_editor(&mut app, editor);
        app.workspace.focus.overlay = AppMode::Dialog(DialogKind::SaveConfirm);

        handle_key(&mut app, make_key(KeyCode::Char('y')));

        assert_eq!(
            app.workspace.focus.overlay,
            AppMode::Normal,
            "{:?}",
            app.status_message
        );
        assert!(app.workspace.documents.active().is_some());
        assert_eq!(fs::read_to_string(file).unwrap(), "new");
    }

    #[test]
    fn save_confirm_yes_stays_in_dialog_on_save_error() {
        let (dir, mut app) = setup_app();
        let invalid_path = dir.path().join("missing").join("editable.txt");

        let mut editor = crate::editor::EditorState::new("new", invalid_path);
        editor.modified = true;
        install_editor(&mut app, editor);
        app.workspace.focus.overlay = AppMode::Dialog(DialogKind::SaveConfirm);

        handle_key(&mut app, make_key(KeyCode::Char('y')));

        assert!(matches!(
            app.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::SaveConfirm)
        ));
        assert!(app.workspace.documents.active().is_some());
        assert!(app.status_message.is_some());
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(msg.contains("Save failed"));
        assert!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .modified
        );
        assert!(!app.should_quit);
    }

    #[test]
    fn save_conflict_cancel_retains_dirty_buffer_and_external_bytes() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("conflict.txt");
        fs::write(&file, "original").unwrap();
        let mut editor = crate::editor::EditorState::from_file(&file).unwrap();
        editor.insert_char('x');
        install_editor(&mut app, editor);
        app.workspace.focus.overlay = AppMode::Normal;
        app.workspace.focus.panel = FocusedPanel::Editor;
        fs::write(&file, "external").unwrap();

        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('s'), KeyModifiers::CONTROL),
        );
        assert!(matches!(app.workspace.focus.overlay, AppMode::Dialog(_)));
        handle_key(&mut app, make_key(KeyCode::Esc));

        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert_eq!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .buffer[0],
            "xoriginal"
        );
        assert!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .modified
        );
        assert!(!app.should_quit);
        assert_eq!(fs::read(&file).unwrap(), b"external");
    }

    #[test]
    fn save_conflict_reload_explicitly_loads_external_version() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("conflict.txt");
        fs::write(&file, "original").unwrap();
        let mut editor = crate::editor::EditorState::from_file(&file).unwrap();
        editor.insert_char('x');
        install_editor(&mut app, editor);
        app.workspace.focus.overlay = AppMode::Normal;
        app.workspace.focus.panel = FocusedPanel::Editor;
        fs::write(&file, "external").unwrap();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('s'), KeyModifiers::CONTROL),
        );

        handle_key(&mut app, make_key(KeyCode::Char('r')));

        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert_eq!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .buffer[0],
            "external"
        );
        assert!(
            !app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .modified
        );
        assert!(!app.should_quit);
        assert_eq!(fs::read(&file).unwrap(), b"external");
    }

    #[test]
    fn save_conflict_failed_reload_retains_dirty_buffer() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("deleted.txt");
        fs::write(&file, "original").unwrap();
        let mut editor = crate::editor::EditorState::from_file(&file).unwrap();
        editor.insert_char('x');
        install_editor(&mut app, editor);
        app.workspace.focus.overlay = AppMode::Normal;
        app.workspace.focus.panel = FocusedPanel::Editor;
        fs::remove_file(&file).unwrap();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('s'), KeyModifiers::CONTROL),
        );

        handle_key(&mut app, make_key(KeyCode::Char('r')));

        assert!(matches!(app.workspace.focus.overlay, AppMode::Dialog(_)));
        assert_eq!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .buffer[0],
            "xoriginal"
        );
        assert!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .modified
        );
        assert!(!app.should_quit);
        assert!(!file.exists());
        assert!(app.status_message.as_ref().unwrap().0.contains("Reload"));
    }

    fn start_save_conflict(app: &mut App, path: &std::path::Path) {
        fs::write(path, "original").unwrap();
        let mut editor = crate::editor::EditorState::from_file(path).unwrap();
        editor.insert_char('x');
        install_editor(app, editor);
        app.workspace.focus.overlay = AppMode::Normal;
        app.workspace.focus.panel = FocusedPanel::Editor;
        fs::write(path, "external").unwrap();
        handle_key(
            app,
            make_key_with_modifiers(KeyCode::Char('s'), KeyModifiers::CONTROL),
        );
        assert!(matches!(app.workspace.focus.overlay, AppMode::Dialog(_)));
    }

    #[test]
    fn save_conflict_save_as_preserves_external_file() {
        let (dir, mut app) = setup_app();
        let original = dir.path().join("original.txt");
        let copy = dir.path().join("copy.txt");
        start_save_conflict(&mut app, &original);
        handle_key(&mut app, make_key(KeyCode::Char('a')));
        app.dialog_state.input = "copy.txt".to_string();
        app.dialog_state.cursor_position = app.dialog_state.input.len();

        handle_key(&mut app, make_key(KeyCode::Enter));

        assert_eq!(fs::read(&original).unwrap(), b"external");
        assert_eq!(fs::read(&copy).unwrap(), b"xoriginal");
        assert_eq!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .file_path,
            copy
        );
        assert!(
            !app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .modified
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert!(!app.should_quit);
    }

    #[test]
    fn save_conflict_save_as_collision_retains_dirty_buffer() {
        let (dir, mut app) = setup_app();
        let original = dir.path().join("original.txt");
        let copy = dir.path().join("copy.txt");
        fs::write(&copy, "keep").unwrap();
        start_save_conflict(&mut app, &original);
        handle_key(&mut app, make_key(KeyCode::Char('a')));
        app.dialog_state.input = "copy.txt".to_string();
        app.dialog_state.cursor_position = app.dialog_state.input.len();

        handle_key(&mut app, make_key(KeyCode::Enter));

        assert_eq!(fs::read(&original).unwrap(), b"external");
        assert_eq!(fs::read(&copy).unwrap(), b"keep");
        assert_eq!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .file_path,
            original
        );
        assert!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .modified
        );
        assert!(!app.should_quit);
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
    }

    #[test]
    fn save_conflict_overwrite_requires_second_explicit_confirmation() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("original.txt");
        start_save_conflict(&mut app, &file);

        handle_key(&mut app, make_key(KeyCode::Char('o')));
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert_eq!(fs::read(&file).unwrap(), b"external");
        assert!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .modified
        );
        handle_key(&mut app, make_key(KeyCode::Char('y')));

        assert_eq!(fs::read(&file).unwrap(), b"xoriginal");
        assert!(
            !app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .modified
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert!(!app.should_quit);
    }

    #[test]
    fn save_conflict_overwrite_cancel_retains_both_versions() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("original.txt");
        start_save_conflict(&mut app, &file);
        handle_key(&mut app, make_key(KeyCode::Char('o')));

        handle_key(&mut app, make_key(KeyCode::Esc));

        assert_eq!(fs::read(&file).unwrap(), b"external");
        assert_eq!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .buffer[0],
            "xoriginal"
        );
        assert!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .modified
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert!(!app.should_quit);
    }

    #[test]
    fn save_conflict_overwrite_refuses_changes_during_confirmation() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("original.txt");
        start_save_conflict(&mut app, &file);
        handle_key(&mut app, make_key(KeyCode::Char('o')));
        fs::write(&file, "newer external").unwrap();

        handle_key(&mut app, make_key(KeyCode::Char('y')));

        assert_eq!(fs::read(&file).unwrap(), b"newer external");
        assert!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .modified
        );
        assert!(!app.should_quit);
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("Save failed"));
    }

    #[test]
    fn save_confirm_conflict_cancel_does_not_close_dirty_editor() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("original.txt");
        start_save_conflict(&mut app, &file);
        app.workspace.focus.overlay = AppMode::Dialog(DialogKind::FocusBackConfirm);

        handle_key(&mut app, make_key(KeyCode::Char('y')));
        assert!(matches!(
            app.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::SaveConflict {
                exit_after_save: true,
                ..
            })
        ));
        handle_key(&mut app, make_key(KeyCode::Esc));

        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .modified
        );
        assert_eq!(fs::read(&file).unwrap(), b"external");
        assert!(!app.should_quit);
    }

    #[test]
    fn mixed_endings_save_requires_explicit_normalization_confirmation() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("mixed.txt");
        fs::write(&file, b"a\r\nb\n").unwrap();
        let mut editor = crate::editor::EditorState::from_file(&file).unwrap();
        editor.insert_char('x');
        install_editor(&mut app, editor);
        app.workspace.focus.overlay = AppMode::Normal;
        app.workspace.focus.panel = FocusedPanel::Editor;
        let save_key = make_key_with_modifiers(KeyCode::Char('s'), KeyModifiers::CONTROL);

        handle_key(&mut app, save_key);
        assert!(matches!(
            app.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::SaveConflict {
                normalize: true,
                ..
            })
        ));
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert_eq!(fs::read(&file).unwrap(), b"a\r\nb\n");
        assert!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .modified
        );
        handle_key(&mut app, save_key);
        handle_key(&mut app, make_key(KeyCode::Char('o')));
        assert_eq!(fs::read(&file).unwrap(), b"a\r\nb\n");

        handle_key(&mut app, make_key(KeyCode::Char('y')));

        assert_eq!(fs::read(&file).unwrap(), b"xa\nb\n");
        assert!(
            !app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .modified
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
    }

    #[cfg(unix)]
    #[test]
    fn confirmed_overwrite_io_failure_retains_dirty_buffer_and_loaded_revision() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("original.txt");
        start_save_conflict(&mut app, &file);
        let loaded = app
            .workspace
            .documents
            .active()
            .map(|d| &d.editor)
            .unwrap()
            .source_revision
            .clone();
        handle_key(&mut app, make_key(KeyCode::Char('o')));
        crate::fs::save::inject_failure(crate::fs::save::Stage::Replace);

        handle_key(&mut app, make_key(KeyCode::Char('y')));

        assert!(matches!(
            app.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::SaveOverwrite { .. })
        ));
        assert_eq!(fs::read(&file).unwrap(), b"external");
        assert!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .modified
        );
        assert_eq!(
            app.workspace
                .documents
                .active()
                .map(|d| &d.editor)
                .unwrap()
                .source_revision,
            loaded
        );
        assert!(!app.should_quit);
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
    }

    #[test]
    fn editor_find_query_handles_multibyte_input_and_backspace() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("unicode_find.txt");
        fs::write(&file, "abc").unwrap();

        install_editor(&mut app, crate::editor::EditorState::new("abc", file));
        app.workspace.focus.overlay = AppMode::Normal;
        app.workspace.focus.panel = FocusedPanel::Editor;
        app.workspace
            .documents
            .active_mut()
            .map(|d| &mut d.editor)
            .unwrap()
            .open_find();

        handle_key(&mut app, make_key(KeyCode::Char('é')));
        {
            let editor = app.workspace.documents.active().map(|d| &d.editor).unwrap();
            assert_eq!(editor.find_state.query, "é");
            assert_eq!(editor.find_state.query_cursor, 'é'.len_utf8());
        }

        handle_key(&mut app, make_key(KeyCode::Backspace));
        let editor = app.workspace.documents.active().map(|d| &d.editor).unwrap();
        assert_eq!(editor.find_state.query, "");
        assert_eq!(editor.find_state.query_cursor, 0);
    }

    #[test]
    fn editor_find_replace_field_handles_multibyte_input_and_backspace() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("unicode_replace.txt");
        fs::write(&file, "abc").unwrap();

        install_editor(&mut app, crate::editor::EditorState::new("abc", file));
        app.workspace.focus.overlay = AppMode::Normal;
        app.workspace.focus.panel = FocusedPanel::Editor;
        {
            let editor = app
                .workspace
                .documents
                .active_mut()
                .map(|d| &mut d.editor)
                .unwrap();
            editor.open_find_replace();
            editor.find_state.in_replace_field = true;
        }

        handle_key(&mut app, make_key(KeyCode::Char('한')));
        {
            let editor = app.workspace.documents.active().map(|d| &d.editor).unwrap();
            assert_eq!(editor.find_state.replacement, "한");
            assert_eq!(editor.find_state.replacement_cursor, '한'.len_utf8());
        }

        handle_key(&mut app, make_key(KeyCode::Backspace));
        let editor = app.workspace.documents.active().map(|d| &d.editor).unwrap();
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
        assert!(matches!(app.workspace.focus.overlay, AppMode::Normal));
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
        assert!(matches!(app.workspace.focus.overlay, AppMode::Normal));
    }

    #[test]
    fn delete_cancel_preserves_file() {
        let (dir, mut app) = setup_app();
        app.tree_state.selected_index = 3;
        handle_key(&mut app, make_key(KeyCode::Char('d')));
        handle_key(&mut app, make_key(KeyCode::Char('n')));
        assert!(dir.path().join("file_a.txt").exists());
        assert!(matches!(app.workspace.focus.overlay, AppMode::Normal));
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
        assert!(matches!(app.workspace.focus.overlay, AppMode::Normal));
    }

    #[test]
    fn error_dialog_dismiss_on_esc() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::Error {
            message: "test error".to_string(),
        });
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert!(matches!(app.workspace.focus.overlay, AppMode::Normal));
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
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
        handle_key(&mut app, make_key(KeyCode::Tab));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
        handle_key(&mut app, make_key(KeyCode::Tab));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
    }

    #[test]
    fn q_quits_from_preview_focus() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Preview;
        handle_key(&mut app, make_key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_quits_from_preview_focus() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Preview;
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(app.should_quit);
    }

    #[tokio::test]
    async fn ctrl_c_with_selection_in_preview_triggers_copy() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Preview;
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
        app.shutdown_background().await;
    }

    #[test]
    fn preview_j_scrolls_down() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.preview_state.total_lines = 100;
        handle_key(&mut app, make_key(KeyCode::Char('j')));
        assert_eq!(app.preview_state.scroll_offset, 1);
    }

    #[test]
    fn preview_k_scrolls_up() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.preview_state.total_lines = 100;
        app.preview_state.scroll_offset = 5;
        handle_key(&mut app, make_key(KeyCode::Char('k')));
        assert_eq!(app.preview_state.scroll_offset, 4);
    }

    #[test]
    fn preview_g_jumps_top() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.preview_state.total_lines = 100;
        app.preview_state.scroll_offset = 50;
        handle_key(&mut app, make_key(KeyCode::Char('g')));
        assert_eq!(app.preview_state.scroll_offset, 0);
    }

    #[test]
    fn preview_shift_g_jumps_bottom() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.preview_state.total_lines = 100;
        handle_key(&mut app, make_key(KeyCode::Char('G')));
        assert_eq!(app.preview_state.scroll_offset, 99);
    }

    #[test]
    fn preview_j_does_not_navigate_tree() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Preview;
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
        let (tx, _rx) = crate::event::event_channel(Default::default());
        // Copy file_a.txt (index 3)
        app.tree_state.selected_index = 3;
        app.copy_to_clipboard();
        // Navigate to beta dir (index 2) and paste
        app.tree_state.selected_index = 2;
        app.expand_selected();
        app.paste_clipboard_async(tx);
        // Wait for completion
        loop {
            let delivery = app.next_background().await.unwrap();
            let done = matches!(delivery.target, crate::app_jobs::Target::Paste(..))
                && !matches!(
                    delivery.result,
                    Ok(crate::app_jobs::NativeOutput::OperationProgress(_))
                );
            app.apply_background(delivery);
            if done {
                break;
            }
        }
        app.shutdown_background().await;
        assert!(dir.path().join("beta").join("file_a.txt").exists());
        // Original still exists
        assert!(dir.path().join("file_a.txt").exists());
    }

    #[tokio::test]
    async fn paste_cut_moves_file() {
        let (dir, mut app) = setup_app();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        // Cut file_a.txt (index 3)
        app.tree_state.selected_index = 3;
        app.cut_to_clipboard();
        // Navigate to beta dir
        app.tree_state.selected_index = 2;
        app.expand_selected();
        app.paste_clipboard_async(tx);
        loop {
            let delivery = app.next_background().await.unwrap();
            let done = matches!(delivery.target, crate::app_jobs::Target::Paste(..))
                && !matches!(
                    delivery.result,
                    Ok(crate::app_jobs::NativeOutput::OperationProgress(_))
                );
            app.apply_background(delivery);
            if done {
                break;
            }
        }
        app.shutdown_background().await;
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
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.tree_state.selected_index = 3;
        app.copy_to_clipboard();
        // Paste into beta
        app.tree_state.selected_index = 2;
        app.expand_selected();
        app.paste_clipboard_async(tx);
        loop {
            let delivery = app.next_background().await.unwrap();
            let done = matches!(delivery.target, crate::app_jobs::Target::Paste(..))
                && !matches!(
                    delivery.result,
                    Ok(crate::app_jobs::NativeOutput::OperationProgress(_))
                );
            app.apply_background(delivery);
            if done {
                break;
            }
        }
        app.shutdown_background().await;
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
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.tree_state.selected_index = 3;
        app.copy_to_clipboard();
        app.tree_state.selected_index = 2;
        app.expand_selected();
        app.paste_clipboard_async(tx);
        loop {
            let delivery = app.next_background().await.unwrap();
            let done = matches!(delivery.target, crate::app_jobs::Target::Paste(..))
                && !matches!(
                    delivery.result,
                    Ok(crate::app_jobs::NativeOutput::OperationProgress(_))
                );
            app.apply_background(delivery);
            if done {
                break;
            }
        }
        app.shutdown_background().await;
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

    #[test]
    fn task3_single_double_click_preview_replacement_and_edit_pin() {
        let (dir, mut app) = setup_app();
        let a = dir.path().join("file_a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, "alpha").unwrap();
        fs::write(&b, "beta").unwrap();
        app.tree_state.reload_dir(dir.path());
        app.tree_area = ratatui::layout::Rect::new(0, 0, 40, 20);
        app.tree_content_area = ratatui::widgets::Block::bordered().inner(app.tree_area);
        let (tx, _rx) = crate::event::event_channel(Default::default());
        let click = |app: &mut App, path: &std::path::Path| {
            let index = app
                .tree_state
                .flat_items
                .iter()
                .position(|i| i.path == path)
                .unwrap();
            handle_mouse_event(
                app,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: 2,
                    row: index as u16 + 1,
                    modifiers: KeyModifiers::NONE,
                },
                &tx,
            );
        };
        click(&mut app, &a);
        let first = app.workspace.documents.active_id().unwrap();
        assert!(!app.workspace.documents.get(first).unwrap().is_pinned());
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
        assert!(!app.editor_visible());
        click(&mut app, &b);
        let second = app.workspace.documents.active_id().unwrap();
        assert_ne!(first, second);
        assert!(app.workspace.documents.get(first).is_none());
        app.workspace.focus.panel = FocusedPanel::Preview;
        // Preview's edit command activates the temporary buffer without prematurely pinning.
        handle_key(&mut app, make_key(KeyCode::Char('e')));
        assert!(!app.workspace.documents.get(second).unwrap().is_pinned());
        handle_key(&mut app, make_key(KeyCode::Char('X')));
        assert!(app.workspace.documents.get(second).unwrap().is_pinned());
        click(&mut app, &a);
        click(&mut app, &a);
        let retained = app.workspace.documents.active().unwrap();
        assert_eq!(retained.path(), a.canonicalize().unwrap());
        assert!(retained.is_pinned());
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert!(app.workspace.documents.get(second).unwrap().editor.modified);
    }

    #[test]
    fn task3_keyboard_document_commands_and_modal_origin() {
        let (dir, mut app) = setup_app();
        let a = dir.path().join("file_a.txt");
        fs::write(&a, "alpha").unwrap();
        let b = dir.path().join("b.txt");
        fs::write(&b, "beta").unwrap();
        app.open_document_path(&a, true);
        let first = app.workspace.documents.active_id().unwrap();
        handle_key(&mut app, make_key(KeyCode::Char('X')));
        app.open_document_path(&b, false);
        let second = app.workspace.documents.active_id().unwrap();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('p'), KeyModifiers::ALT),
        );
        assert!(app.workspace.documents.get(second).unwrap().is_pinned());
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('b'), KeyModifiers::ALT),
        );
        assert_eq!(app.workspace.documents.active_id(), Some(first));
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('n'), KeyModifiers::ALT),
        );
        assert_eq!(app.workspace.documents.active_id(), Some(second));
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('o'), KeyModifiers::ALT),
        );
        let query = app.search_state.query.clone();
        handle_paste_event(&mut app, "not-a-document-command");
        assert_eq!(app.search_state.query, query);
        assert_eq!(app.workspace.focus.overlay, AppMode::Search);
        handle_key(&mut app, make_key(KeyCode::Up));
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert_eq!(app.workspace.documents.active_id(), Some(first));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('r'), KeyModifiers::ALT),
        );
        assert_eq!(
            app.tree_state.flat_items[app.tree_state.selected_index].path,
            a
        );
        assert!(app.workspace.documents.get(first).unwrap().editor.modified);
        app.workspace.focus.panel = FocusedPanel::Editor;
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('o'), KeyModifiers::ALT),
        );
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert_eq!(app.workspace.documents.active_id(), Some(first));
        app.open_dialog(DialogKind::SaveConfirm);
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('n'), KeyModifiers::ALT),
        );
        assert_eq!(app.workspace.documents.active_id(), Some(first));
    }

    #[tokio::test]
    async fn task3_enter_directory_and_quick_open_directory_secondary_actions() {
        let (dir, mut app) = setup_app();
        let path = dir.path().join("alpha");
        app.navigate_to_path(&path);
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert!(app.tree_state.flat_items[app.tree_state.selected_index].is_expanded);
        assert!(app.workspace.documents.is_empty());
        app.open_search();
        for c in "alpha".chars() {
            app.search_input_char(c);
        }
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
        assert_eq!(
            app.tree_state.flat_items[app.tree_state.selected_index].path,
            path
        );
        app.open_search();
        for c in "file_a".chars() {
            app.search_input_char(c);
        }
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Enter, KeyModifiers::ALT),
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::SearchAction);
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert_eq!(app.workspace.focus.overlay, AppMode::Search);
        handle_key(&mut app, make_key(KeyCode::F(2)));
        assert_eq!(app.workspace.focus.overlay, AppMode::SearchAction);
        handle_key(&mut app, make_key(KeyCode::Char('p')));
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
        app.shutdown_background().await;
    }

    #[test]
    fn task3_direct_open_fallbacks_and_cancel_keep_dirty_owner() {
        let (dir, mut app) = setup_app();
        let a = dir.path().join("file_a.txt");
        fs::write(&a, "alpha").unwrap();
        fs::write(dir.path().join("binary.bin"), [0, 1, 2]).unwrap();
        fs::write(
            dir.path().join("book.ipynb"),
            r#"{"cells":[],"metadata":{},"nbformat":4,"nbformat_minor":0}"#,
        )
        .unwrap();
        app.tree_state.reload_dir(dir.path());
        app.open_document_path(&a, true);
        let owner = app.workspace.documents.active_id().unwrap();
        handle_key(&mut app, make_key(KeyCode::Char('X')));
        for query in ["binary.bin", "book.ipynb"] {
            app.workspace.focus.panel = FocusedPanel::Tree;
            app.open_search();
            for c in query.chars() {
                app.search_input_char(c);
            }
            handle_key(&mut app, make_key(KeyCode::Enter));
            assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
            assert_eq!(app.preview_state.current_path, Some(dir.path().join(query)));
            assert_eq!(app.workspace.documents.len(), 1);
            assert_eq!(app.workspace.documents.active_id(), Some(owner));
            assert!(app.workspace.documents.get(owner).unwrap().editor.modified);
        }
        app.activate_document(owner);
        app.open_search();
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert_eq!(app.workspace.documents.active_id(), Some(owner));
        let alias = dir.path().join("./file_a.txt");
        assert!(app.open_document_path(&alias, true));
        assert_eq!(app.workspace.documents.active_id(), Some(owner));
        assert_eq!(app.workspace.documents.len(), 1);
        assert_eq!(app.workspace.documents.get(owner).unwrap().text(), "Xalpha");
    }

    #[test]
    fn task3_tree_enter_opens_pinned_text_and_retains_dirty_history() {
        let (dir, mut app) = setup_app();
        let a = dir.path().join("file_a.txt");
        app.navigate_to_path(&a);
        handle_key(&mut app, make_key(KeyCode::Enter));
        let id = app
            .workspace
            .documents
            .active_id()
            .expect("Enter opens text directly");
        assert!(app.workspace.documents.get(id).unwrap().is_pinned());
        handle_key(&mut app, make_key(KeyCode::Char('X')));
        let text = app.workspace.documents.get(id).unwrap().text();
        app.workspace.focus.panel = FocusedPanel::Tree;
        std::fs::write(dir.path().join("file_b.rs"), "b").unwrap();
        app.tree_state.reload_dir(dir.path());
        app.navigate_to_path(&dir.path().join("file_b.rs"));
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert_ne!(app.workspace.documents.active_id(), Some(id));
        assert_eq!(app.workspace.documents.get(id).unwrap().text(), text);
        assert!(app.workspace.documents.get(id).unwrap().editor.modified);
    }

    #[test]
    fn task3_quick_open_enter_opens_without_action_menu() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        for c in "file_a".chars() {
            app.search_input_char(c);
        }
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert!(app.workspace.documents.active().unwrap().is_pinned());
    }

    // === Search (Ctrl+P) handler tests ===

    #[test]
    fn ctrl_p_opens_search() {
        let (_dir, mut app) = setup_app();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::Search);
    }

    #[test]
    fn search_esc_closes() {
        let (_dir, mut app) = setup_app();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
    }

    #[test]
    fn task3_search_tab_toggles_content_mode_and_routes_input() {
        let (_dir, mut app) = setup_app();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        handle_key(&mut app, make_key(KeyCode::Tab));
        assert!(app.content_search_active);
        assert_eq!(app.workspace.focus.overlay, AppMode::Search);
        handle_key(&mut app, make_key(KeyCode::Char('n')));
        handle_key(&mut app, make_key(KeyCode::Char('e')));
        assert_eq!(app.content_search.query, "ne");
        handle_key(&mut app, make_key(KeyCode::Backspace));
        assert_eq!(app.content_search.query, "n");
        handle_key(&mut app, make_key(KeyCode::Tab));
        assert!(!app.content_search_active);
        assert_eq!(app.search_state.query, "");
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
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
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert!(app.workspace.documents.active().unwrap().is_pinned());
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
    }

    #[tokio::test]
    async fn search_action_y_copy_path_is_non_blocking() {
        let (_dir, mut app) = setup_app();

        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        for c in "file".chars() {
            handle_key(&mut app, make_key(KeyCode::Char(c)));
        }
        assert!(!app.search_state.results.is_empty());

        handle_key(&mut app, make_key(KeyCode::F(2)));
        assert_eq!(app.workspace.focus.overlay, AppMode::SearchAction);

        handle_key(&mut app, make_key(KeyCode::Char('y')));
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);

        let (msg, _) = app
            .status_message
            .as_ref()
            .expect("status message should exist");
        assert!(msg.contains("Copying selection"));
        app.shutdown_background().await;
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
        assert_eq!(app.workspace.focus.overlay, AppMode::Filter);
    }

    #[test]
    fn filter_esc_clears() {
        let (_dir, mut app) = setup_app();
        handle_key(&mut app, make_key(KeyCode::Char('/')));
        handle_key(&mut app, make_key(KeyCode::Char('f')));
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert!(!app.tree_state.is_filtering);
    }

    #[test]
    fn filter_enter_accepts() {
        let (_dir, mut app) = setup_app();
        handle_key(&mut app, make_key(KeyCode::Char('/')));
        handle_key(&mut app, make_key(KeyCode::Char('f')));
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
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
        assert_eq!(app.workspace.focus.overlay, AppMode::Search);

        // Type query
        for c in "deep".chars() {
            handle_key(&mut app, make_key(KeyCode::Char(c)));
        }
        assert!(!app.search_state.results.is_empty());

        // Primary confirmation opens directly; secondary menu is explicitly F2.
        handle_key(&mut app, make_key(KeyCode::Enter));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);

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
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
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
        app.workspace.focus.panel = crate::app::FocusedPanel::Preview;

        // Ctrl+P should work from preview panel (global key)
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::Search);
        handle_key(&mut app, make_key(KeyCode::Esc));

        // / should work from preview panel (global key)
        handle_key(&mut app, make_key(KeyCode::Char('/')));
        assert_eq!(app.workspace.focus.overlay, AppMode::Filter);
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
        app.workspace.focus.panel = FocusedPanel::Preview;
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
        app.workspace.focus.panel = FocusedPanel::Preview;
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
        assert_eq!(app.workspace.focus.overlay, AppMode::Help);
    }

    #[test]
    fn question_mark_toggles_help() {
        let (_dir, mut app) = setup_app();
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('?'), KeyModifiers::SHIFT),
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::Help);
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('?'), KeyModifiers::SHIFT),
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
    }

    #[test]
    fn esc_closes_help() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.overlay = AppMode::Help;
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
    }

    #[test]
    fn help_scroll_down_and_up() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.overlay = AppMode::Help;
        handle_key(&mut app, make_key(KeyCode::Char('j')));
        assert_eq!(app.help_state.scroll_offset, 1);
        handle_key(&mut app, make_key(KeyCode::Char('k')));
        assert_eq!(app.help_state.scroll_offset, 0);
    }

    #[test]
    fn help_keys_do_not_navigate_tree() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.overlay = AppMode::Help;
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
        app.tree_content_area = ratatui::widgets::Block::bordered().inner(app.tree_area);
        app.preview_area = ratatui::layout::Rect::new(40, 0, 60, 20);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
        assert_eq!(app.tree_state.selected_index, 0);

        // Click on row 2 (inner row 1 = index 1, accounting for top border)
        let tx = make_event_tx();
        handle_mouse_event(&mut app, make_mouse_click(10, 2), &tx);
        assert_eq!(app.tree_state.selected_index, 1);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
    }

    #[test]
    fn language_features_overlay_keys_navigate_apply_and_dismiss() {
        use crate::components::language_features::{FeatureView, LanguageFeatures};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.rs");
        std::fs::write(&path, "x").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        let tx = make_event_tx();
        // Manually install an overlay: a small completion list.
        let item = crate::lsp::features::CompletionEntry {
            label: "a".into(),
            detail: None,
            kind: None,
            documentation: None,
            edit: crate::lsp::features::CompletionEdit::Insert { text: "a".into() },
            additional_edits: vec![],
            snippet: false,
            has_command: false,
            deprecated: false,
        };
        app.open_document_path(&path, true);
        let doc = app.workspace.documents.active_id().unwrap();
        app.workspace
            .focus
            .open_overlay(crate::app::AppMode::LanguageFeatures, Some(doc))
            .unwrap();
        app.language_features = Some(LanguageFeatures::new(
            doc,
            "file:///f".into(),
            0,
            FeatureView::Completion {
                items: vec![item.clone()],
            },
        ));

        // Arrows + page keys move selection (list len 1 → stays 0, covered).
        for code in [
            KeyCode::Down,
            KeyCode::Up,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::Char('j'),
        ] {
            handle_key_event(&mut app, make_key(code), &tx);
        }
        assert!(app.language_features.is_some());
        // Keys with the overlay mode open but no surface state → no-op.
        app.language_features = None;
        handle_key_event(&mut app, make_key(KeyCode::Down), &tx);
        // Esc dismisses the open overlay.
        app.language_features = Some(LanguageFeatures::new(
            doc,
            "file:///f".into(),
            0,
            FeatureView::Completion {
                items: vec![item.clone()],
            },
        ));
        handle_key_event(&mut app, make_key(KeyCode::Esc), &tx);
        assert!(app.language_features.is_none());
        // Reopen: Enter applies the (empty-range) completion — overlay closes.
        app.workspace
            .focus
            .open_overlay(crate::app::AppMode::LanguageFeatures, Some(doc))
            .unwrap();
        app.language_features = Some(LanguageFeatures::new(
            doc,
            "file:///f".into(),
            0,
            FeatureView::Completion {
                items: vec![item.clone()],
            },
        ));
        handle_key_event(&mut app, make_key(KeyCode::Enter), &tx);
        assert!(app.language_features.is_none());
        assert_eq!(app.workspace.focus.overlay, crate::app::AppMode::Normal);
    }

    #[test]
    fn language_features_overlay_mouse_click_scroll_and_dismiss() {
        use crate::components::language_features::{FeatureView, LanguageFeatures};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.rs");
        std::fs::write(&path, "x").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        let tx = make_event_tx();
        app.open_document_path(&path, true);
        let doc = app.workspace.documents.active_id().unwrap();
        app.workspace
            .focus
            .open_overlay(crate::app::AppMode::LanguageFeatures, Some(doc))
            .unwrap();
        let loc = crate::lsp::features::LocationEntry {
            uri: "untitled:u1".into(),
            start_line: 0,
            start_character: 0,
            end_line: 0,
            end_character: 0,
        };
        let mut features = LanguageFeatures::new(
            doc,
            "file:///f".into(),
            0,
            FeatureView::Locations {
                title: "References".into(),
                items: vec![loc.clone(), loc],
            },
        );
        // Geometry the mouse handler consumes.
        features.area = ratatui::layout::Rect::new(10, 4, 40, 14);
        features.rows = vec![
            ratatui::layout::Rect::new(10, 5, 40, 1),
            ratatui::layout::Rect::new(10, 6, 40, 1),
        ];
        app.language_features = Some(features);

        // Click row 2 → selection moves and applies (untitled URI →
        // navigate refuses visibly, overlay dismissed).
        handle_mouse_event(&mut app, make_mouse_click(15, 6), &tx);
        assert!(app.language_features.is_none());
        let note = app
            .status_message
            .as_ref()
            .map(|(m, _)| m.clone())
            .unwrap_or_default();
        assert!(note.contains("unsupported URI scheme"), "{note}");

        // Reopen: scroll + click-outside dismiss.
        app.workspace
            .focus
            .open_overlay(crate::app::AppMode::LanguageFeatures, Some(doc))
            .unwrap();
        let mut features = LanguageFeatures::new(
            doc,
            "file:///f".into(),
            0,
            FeatureView::Text {
                title: "Hover".into(),
                lines: vec!["a".into(), "b".into()],
            },
        );
        features.area = ratatui::layout::Rect::new(10, 4, 40, 14);
        features.rows = vec![ratatui::layout::Rect::new(10, 5, 40, 1)];
        app.language_features = Some(features);
        handle_mouse_event(&mut app, make_mouse_scroll_down(15, 5), &tx);
        handle_mouse_event(&mut app, make_mouse_scroll_up(15, 5), &tx);
        assert_eq!(app.language_features.as_ref().unwrap().selected, 0);
        // Click outside the surface dismisses the overlay.
        handle_mouse_event(&mut app, make_mouse_click(60, 40), &tx);
        assert!(app.language_features.is_none());
        // Scroll/other kinds with no overlay state installed → no-op arms.
        app.workspace
            .focus
            .open_overlay(crate::app::AppMode::LanguageFeatures, Some(doc))
            .unwrap();
        handle_mouse_event(&mut app, make_mouse_scroll_down(15, 5), &tx);
        handle_mouse_event(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: 15,
                row: 5,
                modifiers: KeyModifiers::NONE,
            },
            &tx,
        );
        app.dismiss_language_features();
    }

    #[test]
    fn diagnostics_overlay_keys_mouse_and_dismiss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.rs");
        std::fs::write(&path, "let x = 1\nlet y = 2\n").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        let tx = make_event_tx();
        app.open_document_path(&path, true);
        let uri = crate::lsp::features::uri_for_path(&path);
        app.diagnostics.apply(
            crate::diagnostics::Publish {
                language: "rust".into(),
                generation: 0,
                uri: uri.clone(),
                version: None,
                diagnostics: vec![
                    crate::diagnostics::Diagnostic {
                        start_line: 0,
                        start_character: 0,
                        end_line: 0,
                        end_character: 1,
                        severity: crate::diagnostics::Severity::Error,
                        code: None,
                        source: None,
                        message: "boom".into(),
                    },
                    crate::diagnostics::Diagnostic {
                        start_line: 1,
                        start_character: 0,
                        end_line: 1,
                        end_character: 1,
                        severity: crate::diagnostics::Severity::Warning,
                        code: None,
                        source: None,
                        message: "careful".into(),
                    },
                ],
            },
            None,
        );
        app.toggle_diagnostics_panel().unwrap();
        assert_eq!(
            app.workspace.focus.overlay,
            crate::app::AppMode::Diagnostics
        );

        // Every movement key routes through the panel; a non-movement key
        // is swallowed by the read-only list.
        for code in [
            KeyCode::Down,
            KeyCode::Up,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::Char('j'),
        ] {
            handle_key_event(&mut app, make_key(code), &tx);
        }
        assert_eq!(
            app.workspace.focus.overlay,
            crate::app::AppMode::Diagnostics
        );
        // Overlay open but the surface was dropped → keys no-op.
        app.diagnostics_panel = None;
        handle_key_event(&mut app, make_key(KeyCode::Down), &tx);
        assert_eq!(
            app.workspace.focus.overlay,
            crate::app::AppMode::Diagnostics
        );

        // Rebuild the surface; Enter applies the selected row's position.
        let mut panel = crate::components::diagnostics::DiagnosticsPanel::default();
        panel.rebuild(&app.diagnostics);
        app.diagnostics_panel = Some(panel);
        handle_key_event(&mut app, make_key(KeyCode::Enter), &tx);
        assert_eq!(app.workspace.focus.overlay, crate::app::AppMode::Normal);
        assert_eq!(
            app.workspace
                .documents
                .active()
                .unwrap()
                .editor
                .cursor_position()
                .line,
            0
        );

        // Esc dismisses the open panel.
        app.toggle_diagnostics_panel().unwrap();
        handle_key_event(&mut app, make_key(KeyCode::Esc), &tx);
        assert_eq!(app.workspace.focus.overlay, crate::app::AppMode::Normal);

        // Mouse: geometry drives hit-testing without a live renderer.
        app.toggle_diagnostics_panel().unwrap();
        let panel = app.diagnostics_panel.as_mut().unwrap();
        panel.area = ratatui::layout::Rect::new(10, 4, 40, 6);
        panel.row_rects = vec![
            ratatui::layout::Rect::new(10, 5, 40, 1),
            ratatui::layout::Rect::new(10, 6, 40, 1),
        ];
        // Click row 2 → selection moves and applies (line 1 → cursor).
        handle_mouse_event(&mut app, make_mouse_down_left(15, 6), &tx);
        assert_eq!(app.workspace.focus.overlay, crate::app::AppMode::Normal);
        assert_eq!(
            app.workspace
                .documents
                .active()
                .unwrap()
                .editor
                .cursor_position()
                .line,
            1
        );

        // Reopen: scroll events step the selection; inside-but-not-on-a-row
        // stays open; other mouse kinds are ignored.
        app.toggle_diagnostics_panel().unwrap();
        let panel = app.diagnostics_panel.as_mut().unwrap();
        panel.area = ratatui::layout::Rect::new(10, 4, 40, 6);
        panel.row_rects = vec![
            ratatui::layout::Rect::new(10, 5, 40, 1),
            ratatui::layout::Rect::new(10, 6, 40, 1),
        ];
        handle_mouse_event(&mut app, make_mouse_scroll_down(15, 5), &tx);
        handle_mouse_event(&mut app, make_mouse_scroll_up(15, 5), &tx);
        assert_eq!(app.diagnostics_panel.as_ref().unwrap().selected, 0);
        handle_mouse_event(&mut app, make_mouse_down_left(15, 4), &tx); // title row
        assert_eq!(
            app.workspace.focus.overlay,
            crate::app::AppMode::Diagnostics
        );
        handle_mouse_event(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: 15,
                row: 5,
                modifiers: KeyModifiers::NONE,
            },
            &tx,
        );
        assert_eq!(
            app.workspace.focus.overlay,
            crate::app::AppMode::Diagnostics
        );
        // Click outside the surface dismisses.
        handle_mouse_event(&mut app, make_mouse_down_left(60, 40), &tx);
        assert_eq!(app.workspace.focus.overlay, crate::app::AppMode::Normal);

        // Overlay open but the surface was dropped → scroll no-ops.
        app.toggle_diagnostics_panel().unwrap();
        app.diagnostics_panel = None;
        handle_mouse_event(&mut app, make_mouse_scroll_down(15, 5), &tx);
        assert_eq!(
            app.workspace.focus.overlay,
            crate::app::AppMode::Diagnostics
        );
    }

    #[test]
    fn mouse_click_preview_switches_focus() {
        let (_dir, mut app) = setup_app();
        app.tree_area = ratatui::layout::Rect::new(0, 0, 40, 20);
        app.tree_content_area = ratatui::widgets::Block::bordered().inner(app.tree_area);
        app.preview_area = ratatui::layout::Rect::new(40, 0, 60, 20);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);

        let tx = make_event_tx();
        handle_mouse_event(&mut app, make_mouse_click(50, 5), &tx);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
    }

    #[test]
    fn mouse_scroll_tree_navigates() {
        let (_dir, mut app) = setup_app();
        app.tree_area = ratatui::layout::Rect::new(0, 0, 40, 20);
        app.tree_content_area = ratatui::widgets::Block::bordered().inner(app.tree_area);
        app.preview_area = ratatui::layout::Rect::new(40, 0, 60, 20);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
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
        app.tree_content_area = ratatui::widgets::Block::bordered().inner(app.tree_area);
        app.preview_area = ratatui::layout::Rect::new(40, 0, 60, 20);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
        app.preview_state.total_lines = 100;

        let tx = make_event_tx();
        handle_mouse_event(&mut app, make_mouse_scroll_down(50, 5), &tx);
        assert_eq!(app.preview_state.scroll_offset, 1);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
    }

    #[test]
    fn mouse_drag_in_preview_updates_selection() {
        let (_dir, mut app) = setup_app();
        app.preview_area = ratatui::layout::Rect::new(40, 0, 60, 20);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
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
        app.tree_content_area = ratatui::widgets::Block::bordered().inner(app.tree_area);
        app.workspace.focus.overlay = AppMode::Dialog(DialogKind::CreateFile);
        let idx = app.tree_state.selected_index;

        let tx = make_event_tx();
        handle_mouse_event(&mut app, make_mouse_click(10, 2), &tx);
        assert_eq!(app.tree_state.selected_index, idx);
    }

    // === Directional focus keybinding tests ===

    #[test]
    fn ctrl_left_moves_focus_left() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Preview;
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Left, KeyModifiers::CONTROL),
        );
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
    }

    #[test]
    fn ctrl_right_moves_focus_right() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Tree;
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Right, KeyModifiers::CONTROL),
        );
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
    }

    #[test]
    fn ctrl_up_moves_focus_up_from_terminal() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Up, KeyModifiers::CONTROL),
        );
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
    }

    #[test]
    fn ctrl_down_moves_focus_down_to_terminal() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Tree;
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Down, KeyModifiers::CONTROL),
        );
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Terminal);
    }

    #[test]
    fn ctrl_shift_up_resizes_terminal_larger() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.layout.set_terminal_height(7);
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        );
        assert_eq!(app.workspace.layout.terminal_height(), 9);
    }

    #[test]
    fn ctrl_shift_down_resizes_terminal_smaller() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.layout.set_terminal_height(7);
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Down, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        );
        assert_eq!(app.workspace.layout.terminal_height(), 5);
    }

    #[test]
    fn ctrl_arrow_intercepted_when_terminal_focused() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        // Ctrl+Right should switch focus to Preview even when terminal is focused
        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Right, KeyModifiers::CONTROL),
        );
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
    }

    #[test]
    fn tab_still_cycles_focus() {
        let (_dir, mut app) = setup_app();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
        handle_key(&mut app, make_key(KeyCode::Tab));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
    }

    // === Terminal mouse selection tests ===

    #[test]
    fn terminal_click_sets_selection_anchor() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        // Set terminal area with border
        app.terminal_area = ratatui::layout::Rect::new(1, 21, 78, 8);

        let tx = make_event_tx();
        // Click inside terminal inner area (accounting for border)
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 22, // inner_y starts at 21 (20+1)
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse_event(&mut app, mouse, &tx);

        assert_eq!(app.workspace.focus.panel, FocusedPanel::Terminal);
        assert!(app.terminal_state.selection.anchor.is_some());
    }

    #[test]
    fn terminal_drag_updates_selection_endpoint() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.terminal_area = ratatui::layout::Rect::new(1, 21, 78, 8);

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
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.terminal_area = ratatui::layout::Rect::new(1, 21, 78, 8);

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
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.terminal_area = ratatui::layout::Rect::new(1, 21, 78, 8);

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
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.terminal_area = ratatui::layout::Rect::new(1, 21, 78, 8);

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
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.terminal_area = ratatui::layout::Rect::new(1, 21, 78, 8);
        app.tree_area = ratatui::layout::Rect::new(0, 0, 40, 20);
        app.tree_content_area = ratatui::widgets::Block::bordered().inner(app.tree_area);

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
    fn keymap_esc_clears_selection_but_always_forwards_to_terminal() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;

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
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Terminal);

        // Esc is ordinary shell input, even after selection has been cleared.
        handle_key(&mut app, make_key(KeyCode::Esc));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Terminal);
    }

    #[test]
    fn ctrl_shift_c_in_terminal_triggers_copy() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;

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
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;

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
    fn keymap_ctrl_c_with_selection_is_shell_input_not_copy() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        app.status_message = None;

        app.terminal_state.emulator.process(b"Hello");
        let sb = app.terminal_state.emulator.scrollback_len();
        app.terminal_state
            .selection
            .set_anchor(crate::terminal::TerminalCoord { line: sb, col: 0 });
        app.terminal_state
            .selection
            .set_endpoint(crate::terminal::TerminalCoord { line: sb, col: 4 });

        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(app.status_message.is_none());
        assert!(!app.terminal_state.selection.is_active());
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Terminal);
    }

    #[test]
    fn ctrl_c_without_selection_in_terminal_is_not_copy() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
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
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;

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
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;

        handle_key(
            &mut app,
            make_key_with_modifiers(KeyCode::Insert, KeyModifiers::CONTROL),
        );

        assert!(app.status_message.is_some());
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(msg.contains("No terminal text selected"));
    }

    #[tokio::test]
    async fn terminal_right_click_copies_selection() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.terminal_area = ratatui::layout::Rect::new(1, 21, 78, 8);
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;

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
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn preview_right_click_copies_selection() {
        let (_dir, mut app) = setup_app();
        app.preview_area = ratatui::layout::Rect::new(40, 0, 60, 20);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
        app.workspace.focus.panel = FocusedPanel::Preview;
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
        app.shutdown_background().await;
    }

    #[test]
    fn typing_in_terminal_clears_selection() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;

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
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// Helper: create a MouseEvent (Up/Left) at (col, row).
    fn make_mouse_up_left(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: col,
            row,
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
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
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
        let (_dir, mut app) = setup_app();
        let tx = make_event_tx();

        // Set up preview area and content with many lines
        app.preview_area = ratatui::layout::Rect::new(20, 0, 40, 10);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
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
        app.tree_content_area = ratatui::widgets::Block::bordered().inner(app.tree_area);

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

    #[test]
    fn lsp_trust_dialog_keys_approve_and_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let mut config = crate::config::AppConfig::default();
        config.lsp_local.servers.insert(
            "rust".to_string(),
            crate::lsp::config::ServerEntry {
                argv: vec!["/bin/true".to_string()],
                root_markers: vec!["Cargo.toml".to_string()],
            },
        );
        let mut app = App::new(dir.path(), config).unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.event_tx = Some(tx.clone());

        // Opening a Rust file queues the trust prompt for the project argv.
        app.open_document_path(&file, true);
        assert!(app.lsp.next_pending_trust().is_some());
        assert!(matches!(
            app.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::LspTrust { .. })
        ));

        // An unrelated key is a no-op; the prompt re-ask is gated on a
        // Normal overlay (calling it now is an early return).
        app.maybe_prompt_lsp_trust();
        handle_key_event(&mut app, make_key(KeyCode::Char('x')), &tx);
        assert!(matches!(
            app.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::LspTrust { .. })
        ));

        // 'n' refuses: queue drains, overlay closes, argv A stays denied.
        handle_key_event(&mut app, make_key(KeyCode::Char('n')), &tx);
        assert!(matches!(app.workspace.focus.overlay, AppMode::Normal));
        assert!(app.lsp.next_pending_trust().is_none());
        app.maybe_start_lsp_for_path(&file);
        assert_eq!(
            app.lsp.session_status("rust"),
            Some(&crate::lsp::SessionStatus::Denied("refused"))
        );
        // A status-only session has no channel → restart reports the Err arm.
        assert!(app.restart_lsp_current().is_err());

        // The project command changed → fresh prompt; 'y' approves and the
        // session spawns (/bin/sh exits fast; Starting/Dead both fine).
        app.config.lsp_local.servers.get_mut("rust").unwrap().argv = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "exit 0".to_string(),
        ];
        app.maybe_start_lsp_for_path(&file);
        assert!(matches!(
            app.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::LspTrust { .. })
        ));
        handle_key_event(&mut app, make_key(KeyCode::Char('y')), &tx);
        assert!(matches!(app.workspace.focus.overlay, AppMode::Normal));
        assert!(app.lsp.next_pending_trust().is_none());
        assert!(app.lsp.session_status("rust").is_some());
    }

    #[test]
    fn lsp_app_surfaces_status_restart_dismiss_and_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        let mut config = crate::config::AppConfig::default();
        config.lsp_global.servers.insert(
            "rust".to_string(),
            crate::lsp::config::ServerEntry {
                // /bin/cat stays alive — the session channel stays open so
                // the restart path is exercised, not just the error arm.
                argv: vec!["/bin/cat".to_string()],
                root_markers: vec!["Cargo.toml".to_string()],
            },
        );
        config
            .lsp_global
            .trust
            .push(crate::lsp::config::TrustGrantEntry {
                root: dir.path().to_string_lossy().into_owned(),
                argv: vec!["/bin/cat".to_string()],
            });
        let mut app = App::new(dir.path(), config).unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.event_tx = Some(tx.clone());

        // Error arm first: no active document → no language → Err.
        assert!(app.restart_lsp_current().is_err());

        // Global grant + trusted argv → the document's server spawns.
        app.init_lsp_trust();
        app.open_document_path(&file, true);
        app.start_lsp_for_open_documents();
        assert!(app.lsp.has_session("rust"));
        assert!(app.restart_lsp_current().is_ok());

        // Status dialog opens, ignores unrelated keys, dismisses on Esc.
        app.show_lsp_status();
        assert!(matches!(
            app.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::LspStatus { .. })
        ));
        handle_key_event(&mut app, make_key(KeyCode::Char('x')), &tx);
        assert!(matches!(
            app.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::LspStatus { .. })
        ));
        handle_key_event(&mut app, make_key(KeyCode::Esc), &tx);
        assert!(matches!(app.workspace.focus.overlay, AppMode::Normal));

        app.shutdown_lsp();
        assert!(!app.lsp.has_session("rust"));
    }
}
