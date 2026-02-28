use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use crate::app::{App, AppMode, DialogKind, FocusedPanel};
use crate::event::Event;
use crate::fs::operations;

/// Handle a key event and dispatch to the appropriate app method.
pub fn handle_key_event(app: &mut App, key: KeyEvent, event_tx: &mpsc::UnboundedSender<Event>) {
    match &app.mode {
        AppMode::Normal => handle_normal_mode(app, key, event_tx),
        AppMode::Dialog(_) => handle_dialog_mode(app, key),
        AppMode::Search => handle_search_mode(app, key),
        AppMode::Filter => handle_filter_mode(app, key),
    }
}

fn handle_normal_mode(app: &mut App, key: KeyEvent, event_tx: &mpsc::UnboundedSender<Event>) {
    // Global keys (work regardless of focus)
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
        _ => {}
    }

    // Dispatch based on focused panel
    match app.focused_panel {
        FocusedPanel::Tree => handle_tree_keys(app, key, event_tx),
        FocusedPanel::Preview => handle_preview_keys(app, key),
    }
}

fn handle_tree_keys(app: &mut App, key: KeyEvent, event_tx: &mpsc::UnboundedSender<Event>) {
    match key.code {
        // Navigation
        KeyCode::Char('j') | KeyCode::Down => app.select_next(),
        KeyCode::Char('k') | KeyCode::Up => app.select_previous(),
        KeyCode::Char('g') | KeyCode::Home => app.select_first(),
        KeyCode::Char('G') | KeyCode::End => app.select_last(),

        // Tree expand/collapse
        KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => app.expand_selected(),
        KeyCode::Backspace | KeyCode::Char('h') | KeyCode::Left => app.collapse_selected(),

        // Toggle hidden files
        KeyCode::Char('.') => app.toggle_hidden(),

        // Multi-select toggle
        KeyCode::Char(' ') => app.tree_state.toggle_multi_select(),

        // Clear multi-selection
        KeyCode::Esc => app.tree_state.clear_multi_select(),

        // Clipboard operations
        KeyCode::Char('y') => app.copy_to_clipboard(),
        KeyCode::Char('x') => app.cut_to_clipboard(),
        KeyCode::Char('p') => app.paste_clipboard_async(event_tx.clone()),

        // File operations — open dialogs
        KeyCode::Char('a') => app.open_dialog(DialogKind::CreateFile),
        KeyCode::Char('A') => app.open_dialog(DialogKind::CreateDirectory),
        KeyCode::Char('r') => {
            if let Some(item) = app.tree_state.flat_items.get(app.tree_state.selected_index) {
                let original = item.path.clone();
                app.open_dialog(DialogKind::Rename { original });
            }
        }
        KeyCode::Char('d') => {
            if let Some(item) = app.tree_state.flat_items.get(app.tree_state.selected_index) {
                // Don't allow deleting the root
                if item.depth > 0 {
                    let targets = vec![item.path.clone()];
                    app.open_dialog(DialogKind::DeleteConfirm { targets });
                }
            }
        }

        _ => {}
    }
}

fn handle_preview_keys(app: &mut App, key: KeyEvent) {
    match key.code {
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
        // Cycle view mode (large files only)
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.cycle_view_mode();
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

fn handle_filter_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.clear_filter(),
        KeyCode::Enter => app.accept_filter(),
        KeyCode::Backspace => app.filter_delete_char(),
        KeyCode::Char(c) => app.filter_input_char(c),
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
        let app = App::new(dir.path()).unwrap();
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
}
