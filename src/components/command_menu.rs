//! Bounded command search with shared render/hit-test rectangles.
use crate::commands::{CommandContext, CommandId};
use ratatui::layout::Rect;

pub struct CommandMenu {
    pub origin: CommandContext,
    pub query: String,
    pub selected: usize,
    pub scroll: usize,
    pub feedback: Option<String>,
    pub area: Rect,
    pub rows: Vec<(Rect, CommandId)>,
}

impl CommandMenu {
    pub fn new(origin: CommandContext) -> Self {
        Self {
            origin,
            query: String::new(),
            selected: 0,
            scroll: 0,
            feedback: None,
            area: Rect::default(),
            rows: Vec::new(),
        }
    }

    /// Literal paste/search text only. Control characters become spaces; the
    /// UTF-8 byte budget is checked before appending each complete scalar.
    pub fn input(&mut self, text: &str) {
        for ch in text.chars() {
            let ch = if ch.is_control() { ' ' } else { ch };
            if self.query.len() + ch.len_utf8() > 256 {
                break;
            }
            self.query.push(ch);
        }
        self.reset_query_selection();
    }

    fn reset_query_selection(&mut self) {
        self.selected = 0;
        self.scroll = 0;
        self.feedback = None;
        self.rows.clear();
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.reset_query_selection();
    }

    pub fn filtered(&self) -> Vec<&'static crate::commands::CommandMetadata> {
        let query = self.query.to_lowercase();
        crate::commands::REGISTRY
            .iter()
            .filter(|m| {
                query.split_whitespace().all(|word| {
                    m.label.to_lowercase().contains(word)
                        || m.id.as_str().contains(word)
                        || m.description.to_lowercase().contains(word)
                })
            })
            .collect()
    }

    pub fn move_selection(&mut self, delta: isize) {
        let count = self.filtered().len();
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(count.saturating_sub(1));
        self.feedback = None;
        // Until another render, old hit targets no longer describe the viewport.
        self.rows.clear();
    }

    pub fn selected_command(&self) -> Option<CommandId> {
        self.filtered().get(self.selected).map(|m| m.id)
    }

    pub fn render(&mut self, app: &crate::app::App, frame: &mut ratatui::Frame) {
        self.render_in_area(app, frame, frame.area());
    }

    pub(crate) fn render_in_area(
        &mut self,
        app: &crate::app::App,
        frame: &mut ratatui::Frame,
        area: Rect,
    ) {
        use ratatui::{
            style::{Color, Style},
            widgets::{Block, Borders, Clear, Paragraph},
        };
        self.area = menu_rect(area);
        self.rows.clear();
        if self.area.width == 0 || self.area.height == 0 {
            return;
        }
        frame.render_widget(Clear, self.area);
        let mut labels = app
            .keymap
            .binding_labels(CommandId::Commands, crate::keymap::FocusContext::Menu);
        labels.push("Esc".into());
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" Commands · {} ", labels.join("/")));
        let inner = block.inner(self.area);
        frame.render_widget(block, self.area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let query_area = Rect::new(inner.x, inner.y, inner.width, 1);
        frame.render_widget(Paragraph::new(format!("> {}", self.query)), query_area);
        let entries = self.filtered();
        self.selected = self.selected.min(entries.len().saturating_sub(1));
        let height = inner.height.saturating_sub(3) as usize;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        if self.selected >= self.scroll.saturating_add(height) {
            self.scroll = self.selected.saturating_add(1).saturating_sub(height);
        }
        self.scroll = self.scroll.min(entries.len().saturating_sub(height));
        for (index, metadata) in entries.iter().enumerate().skip(self.scroll).take(height) {
            let row = Rect::new(
                inner.x,
                inner.y + 1 + (index - self.scroll) as u16,
                inner.width,
                1,
            );
            let reason = crate::commands::unavailable_reason(app, &self.origin, metadata.id);
            let text = match reason {
                Some(reason) => format!(
                    "{} × {} — {}",
                    if index == self.selected { ">" } else { " " },
                    metadata.label,
                    reason
                ),
                None => format!(
                    "{} {}",
                    if index == self.selected { ">" } else { " " },
                    metadata.label
                ),
            };
            let style = if index == self.selected {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else if reason.is_some() {
                Style::default().fg(Color::Gray)
            } else {
                Style::default()
            };
            frame.render_widget(Paragraph::new(text).style(style), row);
            self.rows.push((row, metadata.id));
        }
        if inner.height >= 2 {
            let selected = entries.get(self.selected);
            let detail = self
                .feedback
                .as_deref()
                .or_else(|| {
                    selected
                        .and_then(|m| crate::commands::unavailable_reason(app, &self.origin, m.id))
                })
                .or_else(|| selected.map(|m| m.description))
                .unwrap_or("No matching commands");
            frame.render_widget(
                Paragraph::new(detail),
                Rect::new(inner.x, inner.bottom() - 2, inner.width, 1),
            );
            let footer = format!(
                "{}/{} · ↑↓ PgUp/PgDn · Enter · Esc cancels",
                if entries.is_empty() {
                    0
                } else {
                    self.selected + 1
                },
                entries.len()
            );
            frame.render_widget(
                Paragraph::new(footer),
                Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
            );
        }
    }

    pub fn hit(&self, column: u16, row: u16) -> Option<CommandId> {
        self.rows
            .iter()
            .find(|(rect, _)| rect.contains((column, row).into()))
            .map(|(_, id)| *id)
    }
}

/// All geometry derives from the render area, including zero and nonzero origins.
fn menu_rect(area: Rect) -> Rect {
    let width = area.width.min(78);
    let height = area.height.min(20);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{App, AppMode},
        config::AppConfig,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn fixture() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        app.open_document_path(&dir.path().join("a.txt"), true);
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .insert_text("dirty")
            .unwrap();
        (dir, app)
    }

    #[test]
    fn task3_menu_title_uses_menu_bindings_not_entry_context() {
        use crate::keymap::{BindingOverride, FocusContext, KeymapConfig};
        let (_dir, mut app) = fixture();
        app.apply_keymap_config(KeymapConfig {
            bindings: Some(vec![
                BindingOverride {
                    command: "workspace.commands".into(),
                    context: FocusContext::Editor,
                    keys: vec![],
                },
                BindingOverride {
                    command: "workspace.commands".into(),
                    context: FocusContext::Menu,
                    keys: vec!["F9".into()],
                },
            ]),
            ..Default::default()
        })
        .unwrap();
        app.open_command_menu();
        let text = draw(&mut app, 80, 24);
        assert!(text.contains("Commands · F9/Esc"), "{text}");
        assert!(!text.contains("Commands · F8/Esc"));
        app.apply_keymap_config(KeymapConfig {
            bindings: Some(vec![BindingOverride {
                command: "workspace.commands".into(),
                context: FocusContext::Menu,
                keys: vec![],
            }]),
            ..Default::default()
        })
        .unwrap();
        assert!(draw(&mut app, 80, 24).contains("Commands · Esc"));
    }

    #[test]
    fn task3_status_entry_override_and_unbound_mouse_route() {
        use crate::keymap::{BindingOverride, FocusContext, KeymapConfig};
        let (_dir, mut app) = fixture();
        for (keys, expected) in [(vec!["F9".into()], "[Commands F9]"), (vec![], "[Commands]")] {
            app.apply_keymap_config(KeymapConfig {
                bindings: Some(vec![BindingOverride {
                    command: "workspace.commands".into(),
                    context: FocusContext::Editor,
                    keys,
                }]),
                ..Default::default()
            })
            .unwrap();
            assert!(draw(&mut app, 80, 24).contains(expected));
            assert!(app.command_entry_area.width > 0);
            use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
            let (tx, _rx) = crate::event::event_channel(Default::default());
            let rect = app.command_entry_area;
            let origin = app.workspace.documents.active_id();
            crate::handler::handle_mouse_event(
                &mut app,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: rect.x,
                    row: rect.y,
                    modifiers: KeyModifiers::NONE,
                },
                &tx,
            );
            assert_eq!(app.workspace.focus.overlay, AppMode::CommandMenu);
            assert_eq!(app.command_menu.as_ref().unwrap().origin.document, origin);
            app.dismiss_command_menu();
            assert_eq!(app.workspace.focus.panel, crate::app::FocusedPanel::Editor);
        }
    }

    #[test]
    fn command_menu_keyboard_entry_dirty_cancel_restores_bytes_and_find() {
        let (_dir, mut app) = fixture();
        let id = app.workspace.documents.active_id();
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .find_state
            .active = true;
        let (tx, _rx) = crate::event::event_channel(Default::default());
        crate::handler::handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE),
            &tx,
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::CommandMenu);
        crate::handler::handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &tx,
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.documents.active_id(), id);
        let editor = &app.workspace.documents.active().unwrap().editor;
        assert_eq!(editor.buffer[0], "dirtyalpha");
        assert!(editor.find_state.active && editor.modified);
    }

    #[test]
    fn command_menu_literal_query_is_utf8_safe_and_bounded() {
        let (_dir, app) = fixture();
        let mut menu = CommandMenu::new(CommandContext::capture(&app));
        menu.input("q\n中\x1b");
        assert_eq!(menu.query, "q 中 ");
        menu.input(&"😀".repeat(500));
        assert!(menu.query.len() <= 256);
        assert!(menu.query.is_char_boundary(menu.query.len()));
        assert_eq!(
            app.workspace.documents.active().unwrap().editor.buffer[0],
            "dirtyalpha"
        );
    }

    #[test]
    fn command_menu_geometry_render_and_hit_share_rects() {
        let (_dir, app) = fixture();
        for (x, y, w, h) in [
            (0, 0, 0, 0),
            (0, 0, 1, 1),
            (0, 0, 3, 3),
            (0, 0, 60, 20),
            (0, 0, 80, 24),
            (0, 0, 120, 40),
            (7, 9, 60, 20),
        ] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(x + w, y + h)).unwrap();
            let mut menu = CommandMenu::new(CommandContext::capture(&app));
            terminal
                .draw(|frame| {
                    // Nonzero origin is also exercised by the rectangle helper later.
                    menu.render_in_area(&app, frame, Rect::new(x, y, w, h));
                })
                .unwrap();
            if w >= 60 {
                assert!(!menu.rows.is_empty());
                for (rect, id) in &menu.rows {
                    assert_eq!(menu.hit(rect.x, rect.y), Some(*id));
                    assert!(menu.area.contains((rect.x, rect.y).into()));
                }
            }
        }
    }

    fn draw(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|frame| crate::ui::render(app, frame))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn key(app: &mut App, code: KeyCode) {
        let (tx, _rx) = crate::event::event_channel(Default::default());
        crate::handler::handle_key_event(app, KeyEvent::new(code, KeyModifiers::NONE), &tx);
    }

    #[test]
    fn command_menu_mouse_entry_disabled_selection_and_outside_cancel() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let (_dir, mut app) = fixture();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        assert!(draw(&mut app, 80, 24).contains("[Commands F8/Alt+G m]"));
        let entry = app.command_entry_area;
        crate::handler::handle_mouse_event(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: entry.x,
                row: entry.y,
                modifiers: KeyModifiers::NONE,
            },
            &tx,
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::CommandMenu);
        app.command_menu.as_mut().unwrap().input("recovery");
        assert!(draw(&mut app, 80, 24).contains("Phase 7"));
        let (rect, id) = app.command_menu.as_ref().unwrap().rows[1];
        crate::handler::handle_mouse_event(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x,
                row: rect.y,
                modifiers: KeyModifiers::NONE,
            },
            &tx,
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::CommandMenu);
        assert_eq!(
            app.command_menu.as_ref().unwrap().selected_command(),
            Some(id)
        );
        assert!(app
            .command_menu
            .as_ref()
            .unwrap()
            .feedback
            .as_ref()
            .unwrap()
            .contains("not implemented"));
        crate::handler::handle_mouse_event(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            &tx,
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.editor().unwrap().buffer[0], "dirtyalpha");
    }

    #[test]
    fn command_menu_search_paste_scroll_keyboard_and_activation() {
        let (_dir, mut app) = fixture();
        key(&mut app, KeyCode::F(8));
        key(&mut app, KeyCode::End);
        let screen = draw(&mut app, 60, 20);
        let menu = app.command_menu.as_ref().unwrap();
        assert!(menu.scroll > 0);
        assert!(menu
            .rows
            .iter()
            .any(|(_, id)| *id == CommandId::RecoveryDisable));
        assert!(screen.contains("not implemented"));
        key(&mut app, KeyCode::Enter); // disabled: keeps the menu
        assert_eq!(app.workspace.focus.overlay, AppMode::CommandMenu);
        key(&mut app, KeyCode::Home);
        assert_eq!(app.command_menu.as_ref().unwrap().selected, 0);
        key(&mut app, KeyCode::PageDown);
        key(&mut app, KeyCode::PageUp);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Up);
        key(&mut app, KeyCode::Char('x'));
        key(&mut app, KeyCode::Backspace);
        crate::handler::handle_paste_event(&mut app, "document.save");
        assert_eq!(
            app.command_menu.as_ref().unwrap().selected_command(),
            Some(CommandId::Save)
        );
        assert_eq!(app.editor().unwrap().buffer[0], "dirtyalpha");
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert!(!app.editor().unwrap().modified);
    }

    #[test]
    fn command_menu_terminal_literal_paste_no_matches_and_escape_restore() {
        let (_dir, mut app) = fixture();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = crate::app::FocusedPanel::Terminal;
        key(&mut app, KeyCode::F(8));
        crate::handler::handle_paste_event(&mut app, "q\n😀\x1b");
        let screen = draw(&mut app, 80, 24);
        assert!(screen.contains("No matching commands"));
        assert!(app
            .command_menu
            .as_ref()
            .unwrap()
            .selected_command()
            .is_none());
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Tab);
        let (tx, _rx) = crate::event::event_channel(Default::default());
        crate::handler::handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &tx,
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::CommandMenu);
        key(&mut app, KeyCode::F(8));
        assert_eq!(
            app.workspace.focus.panel,
            crate::app::FocusedPanel::Terminal
        );
        assert_eq!(app.editor().unwrap().buffer[0], "dirtyalpha");
        assert!(!app.should_quit);
    }

    #[test]
    fn command_menu_full_ui_zero_tiny_and_unicode_paste_remain_bounded() {
        let (_dir, mut app) = fixture();
        app.open_command_menu();
        for (w, h) in [
            (0, 0),
            (1, 1),
            (2, 4),
            (4, 2),
            (60, 20),
            (80, 24),
            (120, 40),
        ] {
            app.command_menu
                .as_mut()
                .unwrap()
                .input(&"中😀e\u{301}".repeat(100));
            draw(&mut app, w, h);
            let menu = app.command_menu.as_ref().unwrap();
            assert!(menu.query.len() <= 256);
            assert!(menu.area.right() <= w && menu.area.bottom() <= h);
            assert!(app.command_entry_area.right() <= w);
        }
    }

    #[test]
    fn command_menu_nested_copy_and_progress_preserve_menu_origin() {
        let (_dir, mut app) = fixture();
        app.open_command_menu();
        let origin = app.command_menu.as_ref().unwrap().origin.document;
        app.workspace
            .focus
            .open_overlay(
                AppMode::Dialog(crate::app::DialogKind::Progress {
                    message: "work".into(),
                    current: 0,
                    total: 1,
                }),
                origin,
            )
            .unwrap();
        let mut output = Vec::new();
        app.show_copyable_text("copy".into(), &mut output, false);
        app.workspace.focus.retire_progress();
        app.dismiss_copy_overlay();
        assert_eq!(app.workspace.focus.overlay, AppMode::CommandMenu);
        assert_eq!(app.workspace.focus.overlay_document(), origin);
        assert_eq!(app.command_menu.as_ref().unwrap().origin.document, origin);
        app.dismiss_command_menu();
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert!(app.workspace.focus.dismiss_overlay().is_none());
    }

    #[test]
    fn command_menu_origin_cancel_preserves_undo_selection_and_find_query() {
        let (dir, mut app) = fixture();
        let a = app.workspace.documents.active_id().unwrap();
        std::fs::write(dir.path().join("b.txt"), "beta").unwrap();
        app.open_document_path(&dir.path().join("b.txt"), true);
        let b = app.workspace.documents.active_id().unwrap();
        app.activate_document(a);
        let editor = &mut app.workspace.documents.get_mut(a).unwrap().editor;
        editor.selection = Some(crate::editor::Selection::new(0, 1));
        editor.find_state.query = "alpha".into();
        let cursor = (editor.cursor_line, editor.cursor_col);
        app.open_command_menu();
        app.workspace.documents.activate(b).unwrap();
        app.dismiss_command_menu();
        assert_eq!(app.workspace.documents.active_id(), Some(a));
        let editor = &mut app.workspace.documents.get_mut(a).unwrap().editor;
        assert_eq!((editor.cursor_line, editor.cursor_col), cursor);
        assert_eq!(editor.selection, Some(crate::editor::Selection::new(0, 1)));
        assert_eq!(editor.find_state.query, "alpha");
        editor.undo();
        assert_eq!(editor.buffer[0], "alpha");
        editor.redo();
        assert_eq!(editor.buffer[0], "dirtyalpha");
    }

    #[test]
    fn command_menu_mouse_scroll_and_save_use_rendered_rows() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let (_dir, mut app) = fixture();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.open_command_menu();
        draw(&mut app, 80, 24);
        for kind in [MouseEventKind::ScrollDown, MouseEventKind::ScrollUp] {
            crate::handler::handle_mouse_event(
                &mut app,
                MouseEvent {
                    kind,
                    column: 40,
                    row: 10,
                    modifiers: KeyModifiers::NONE,
                },
                &tx,
            );
        }
        draw(&mut app, 80, 24);
        let (rect, id) = app.command_menu.as_ref().unwrap().rows[0];
        assert_eq!(id, CommandId::Save);
        crate::handler::handle_mouse_event(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x,
                row: rect.y,
                modifiers: KeyModifiers::NONE,
            },
            &tx,
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert!(!app.editor().unwrap().modified);
    }
}
