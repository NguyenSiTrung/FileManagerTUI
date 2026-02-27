use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;

/// Handle a key event and dispatch to the appropriate app method.
pub fn handle_key_event(app: &mut App, key: KeyEvent) {
    match key.code {
        // Quit
        KeyCode::Char('q') => app.quit(),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => app.quit(),

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
        // Move to first directory (alpha)
        handle_key_event(&mut app, make_key(KeyCode::Char('j')));
        assert_eq!(app.tree_state.flat_items[1].name, "alpha");
        handle_key_event(&mut app, make_key(KeyCode::Enter));
        assert!(app.tree_state.flat_items[1].is_expanded);
    }

    #[test]
    fn key_backspace_collapses_directory() {
        let (_dir, mut app) = setup_app();
        // Expand alpha first
        handle_key_event(&mut app, make_key(KeyCode::Char('j')));
        handle_key_event(&mut app, make_key(KeyCode::Enter));
        assert!(app.tree_state.flat_items[1].is_expanded);
        // Collapse it
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
}
