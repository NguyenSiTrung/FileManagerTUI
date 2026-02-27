use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode, DialogKind, FocusedPanel};
use crate::fs::operations;

/// Handle a key event and dispatch to the appropriate app method.
pub fn handle_key_event(app: &mut App, key: KeyEvent) {
    match &app.mode {
        AppMode::Normal => handle_normal_mode(app, key),
        AppMode::Dialog(_) => handle_dialog_mode(app, key),
    }
}

fn handle_normal_mode(app: &mut App, key: KeyEvent) {
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
        _ => {}
    }

    // Dispatch based on focused panel
    match app.focused_panel {
        FocusedPanel::Tree => handle_tree_keys(app, key),
        FocusedPanel::Preview => handle_preview_keys(app, key),
    }
}

fn handle_tree_keys(app: &mut App, key: KeyEvent) {
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
            app.preview_half_page_down(30); // Default visible height; actual wired from UI later
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.preview_half_page_up(30);
        }
        // Toggle line wrap
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.preview_state.line_wrap = !app.preview_state.line_wrap;
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
        handle_key_event(&mut app, make_key(KeyCode::Char('j')));
        assert_eq!(app.tree_state.selected_index, 1);
    }

    #[test]
    fn key_k_moves_up() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 2;
        handle_key_event(&mut app, make_key(KeyCode::Char('k')));
        assert_eq!(app.tree_state.selected_index, 1);
    }

    #[test]
    fn key_down_arrow_moves_down() {
        let (_dir, mut app) = setup_app();
        handle_key_event(&mut app, make_key(KeyCode::Down));
        assert_eq!(app.tree_state.selected_index, 1);
    }

    #[test]
    fn key_up_arrow_moves_up() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 1;
        handle_key_event(&mut app, make_key(KeyCode::Up));
        assert_eq!(app.tree_state.selected_index, 0);
    }

    #[test]
    fn key_g_jumps_to_first() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 3;
        handle_key_event(&mut app, make_key(KeyCode::Char('g')));
        assert_eq!(app.tree_state.selected_index, 0);
    }

    #[test]
    fn key_shift_g_jumps_to_last() {
        let (_dir, mut app) = setup_app();
        handle_key_event(&mut app, make_key(KeyCode::Char('G')));
        assert_eq!(
            app.tree_state.selected_index,
            app.tree_state.flat_items.len() - 1
        );
    }

    #[test]
    fn key_enter_expands_directory() {
        let (_dir, mut app) = setup_app();
        handle_key_event(&mut app, make_key(KeyCode::Char('j')));
        assert_eq!(app.tree_state.flat_items[1].name, "alpha");
        handle_key_event(&mut app, make_key(KeyCode::Enter));
        assert!(app.tree_state.flat_items[1].is_expanded);
    }

    #[test]
    fn key_backspace_collapses_directory() {
        let (_dir, mut app) = setup_app();
        handle_key_event(&mut app, make_key(KeyCode::Char('j')));
        handle_key_event(&mut app, make_key(KeyCode::Enter));
        assert!(app.tree_state.flat_items[1].is_expanded);
        handle_key_event(&mut app, make_key(KeyCode::Backspace));
        assert!(!app.tree_state.flat_items[1].is_expanded);
    }

    #[test]
    fn key_dot_toggles_hidden() {
        let (_dir, mut app) = setup_app();
        let before = app.tree_state.flat_items.len();
        handle_key_event(&mut app, make_key(KeyCode::Char('.')));
        assert!(app.tree_state.flat_items.len() > before);
    }

    #[test]
    fn key_q_quits() {
        let (_dir, mut app) = setup_app();
        handle_key_event(&mut app, make_key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn key_ctrl_c_quits() {
        let (_dir, mut app) = setup_app();
        handle_key_event(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(app.should_quit);
    }

    // === Dialog opener tests ===

    #[test]
    fn key_a_opens_create_file_dialog() {
        let (_dir, mut app) = setup_app();
        handle_key_event(&mut app, make_key(KeyCode::Char('a')));
        assert!(matches!(app.mode, AppMode::Dialog(DialogKind::CreateFile)));
    }

    #[test]
    fn key_shift_a_opens_create_dir_dialog() {
        let (_dir, mut app) = setup_app();
        handle_key_event(
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
        handle_key_event(&mut app, make_key(KeyCode::Char('r')));
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
        handle_key_event(&mut app, make_key(KeyCode::Char('d')));
        assert!(matches!(
            app.mode,
            AppMode::Dialog(DialogKind::DeleteConfirm { .. })
        ));
    }

    #[test]
    fn key_d_on_root_is_noop() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 0; // root
        handle_key_event(&mut app, make_key(KeyCode::Char('d')));
        assert!(matches!(app.mode, AppMode::Normal));
    }

    // === Dialog input tests ===

    #[test]
    fn dialog_esc_closes() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        handle_key_event(&mut app, make_key(KeyCode::Esc));
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn dialog_typing_inputs_chars() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        handle_key_event(&mut app, make_key(KeyCode::Char('t')));
        handle_key_event(&mut app, make_key(KeyCode::Char('e')));
        handle_key_event(&mut app, make_key(KeyCode::Char('s')));
        handle_key_event(&mut app, make_key(KeyCode::Char('t')));
        assert_eq!(app.dialog_state.input, "test");
    }

    #[test]
    fn dialog_backspace_deletes() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        handle_key_event(&mut app, make_key(KeyCode::Char('a')));
        handle_key_event(&mut app, make_key(KeyCode::Char('b')));
        handle_key_event(&mut app, make_key(KeyCode::Backspace));
        assert_eq!(app.dialog_state.input, "a");
    }

    // === Integration tests: actual file operations ===

    #[test]
    fn create_file_via_dialog() {
        let (dir, mut app) = setup_app();
        // Open create file dialog
        handle_key_event(&mut app, make_key(KeyCode::Char('a')));
        // Type filename
        for c in "new_file.txt".chars() {
            handle_key_event(&mut app, make_key(KeyCode::Char(c)));
        }
        // Confirm
        handle_key_event(&mut app, make_key(KeyCode::Enter));
        // Verify file was created
        assert!(dir.path().join("new_file.txt").exists());
        assert!(matches!(app.mode, AppMode::Normal));
        assert!(app.status_message.is_some());
    }

    #[test]
    fn create_dir_via_dialog() {
        let (dir, mut app) = setup_app();
        handle_key_event(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('A'), KeyModifiers::SHIFT),
        );
        for c in "new_dir".chars() {
            handle_key_event(&mut app, make_key(KeyCode::Char(c)));
        }
        handle_key_event(&mut app, make_key(KeyCode::Enter));
        assert!(dir.path().join("new_dir").exists());
        assert!(dir.path().join("new_dir").is_dir());
    }

    #[test]
    fn rename_file_via_dialog() {
        let (dir, mut app) = setup_app();
        // Select file_a.txt (index 3)
        app.tree_state.selected_index = 3;
        handle_key_event(&mut app, make_key(KeyCode::Char('r')));
        // Clear existing name and type new one
        for _ in 0..app.dialog_state.input.len() {
            handle_key_event(&mut app, make_key(KeyCode::Backspace));
        }
        for c in "renamed.txt".chars() {
            handle_key_event(&mut app, make_key(KeyCode::Char(c)));
        }
        handle_key_event(&mut app, make_key(KeyCode::Enter));
        assert!(!dir.path().join("file_a.txt").exists());
        assert!(dir.path().join("renamed.txt").exists());
    }

    #[test]
    fn delete_file_via_dialog() {
        let (dir, mut app) = setup_app();
        // Select file_a.txt (index 3)
        app.tree_state.selected_index = 3;
        handle_key_event(&mut app, make_key(KeyCode::Char('d')));
        // Confirm delete
        handle_key_event(&mut app, make_key(KeyCode::Char('y')));
        assert!(!dir.path().join("file_a.txt").exists());
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn delete_cancel_preserves_file() {
        let (dir, mut app) = setup_app();
        app.tree_state.selected_index = 3;
        handle_key_event(&mut app, make_key(KeyCode::Char('d')));
        handle_key_event(&mut app, make_key(KeyCode::Char('n')));
        assert!(dir.path().join("file_a.txt").exists());
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn normal_keys_ignored_in_dialog() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        let idx = app.tree_state.selected_index;
        // 'j' should type 'j', not navigate
        handle_key_event(&mut app, make_key(KeyCode::Char('j')));
        assert_eq!(app.dialog_state.input, "j");
        assert_eq!(app.tree_state.selected_index, idx);
    }

    #[test]
    fn error_dialog_dismiss_on_enter() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::Error {
            message: "test error".to_string(),
        });
        handle_key_event(&mut app, make_key(KeyCode::Enter));
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn error_dialog_dismiss_on_esc() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::Error {
            message: "test error".to_string(),
        });
        handle_key_event(&mut app, make_key(KeyCode::Esc));
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn tree_refreshes_after_create() {
        let (_dir, mut app) = setup_app();
        let before_count = app.tree_state.flat_items.len();
        handle_key_event(&mut app, make_key(KeyCode::Char('a')));
        for c in "brand_new.txt".chars() {
            handle_key_event(&mut app, make_key(KeyCode::Char(c)));
        }
        handle_key_event(&mut app, make_key(KeyCode::Enter));
        // Tree should have one more item
        assert_eq!(app.tree_state.flat_items.len(), before_count + 1);
    }

    // === Focus management tests ===

    #[test]
    fn tab_toggles_focus() {
        let (_dir, mut app) = setup_app();
        assert_eq!(app.focused_panel, FocusedPanel::Tree);
        handle_key_event(&mut app, make_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Preview);
        handle_key_event(&mut app, make_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Tree);
    }

    #[test]
    fn q_quits_from_preview_focus() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        handle_key_event(&mut app, make_key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_quits_from_preview_focus() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        handle_key_event(
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
        handle_key_event(&mut app, make_key(KeyCode::Char('j')));
        assert_eq!(app.preview_state.scroll_offset, 1);
    }

    #[test]
    fn preview_k_scrolls_up() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        app.preview_state.total_lines = 100;
        app.preview_state.scroll_offset = 5;
        handle_key_event(&mut app, make_key(KeyCode::Char('k')));
        assert_eq!(app.preview_state.scroll_offset, 4);
    }

    #[test]
    fn preview_g_jumps_top() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        app.preview_state.total_lines = 100;
        app.preview_state.scroll_offset = 50;
        handle_key_event(&mut app, make_key(KeyCode::Char('g')));
        assert_eq!(app.preview_state.scroll_offset, 0);
    }

    #[test]
    fn preview_shift_g_jumps_bottom() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        app.preview_state.total_lines = 100;
        handle_key_event(&mut app, make_key(KeyCode::Char('G')));
        assert_eq!(app.preview_state.scroll_offset, 99);
    }

    #[test]
    fn preview_ctrl_w_toggles_wrap() {
        let (_dir, mut app) = setup_app();
        app.focused_panel = FocusedPanel::Preview;
        assert!(!app.preview_state.line_wrap);
        handle_key_event(
            &mut app,
            make_key_with_modifiers(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );
        assert!(app.preview_state.line_wrap);
        handle_key_event(
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
        handle_key_event(&mut app, make_key(KeyCode::Char('j')));
        assert_eq!(app.tree_state.selected_index, idx);
    }
}
