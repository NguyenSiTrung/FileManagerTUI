use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget, Wrap},
};

use crate::app::{AppMode, DialogKind, DialogState};
use crate::theme::ThemeColors;

/// Dialog widget that renders a centered modal overlay.
pub struct DialogWidget<'a> {
    mode: &'a AppMode,
    dialog_state: &'a DialogState,
    theme: &'a ThemeColors,
}

impl<'a> DialogWidget<'a> {
    pub fn new(mode: &'a AppMode, dialog_state: &'a DialogState, theme: &'a ThemeColors) -> Self {
        Self {
            mode,
            dialog_state,
            theme,
        }
    }

    /// Calculate a centered rectangle within the given area.
    fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
        let x = area.x + area.width.saturating_sub(width) / 2;
        let y = area.y + area.height.saturating_sub(height) / 2;
        let w = width.min(area.width);
        let h = height.min(area.height);
        Rect::new(x, y, w, h)
    }
}

impl<'a> Widget for DialogWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let kind = match &self.mode {
            AppMode::Dialog(kind) => kind,
            _ => return,
        };

        match kind {
            DialogKind::CreateFile => {
                render_input_dialog("Create New File", self.dialog_state, self.theme, area, buf);
            }
            DialogKind::CreateDirectory => {
                render_input_dialog(
                    "Create New Directory",
                    self.dialog_state,
                    self.theme,
                    area,
                    buf,
                );
            }
            DialogKind::Rename { .. } => {
                render_input_dialog("Rename", self.dialog_state, self.theme, area, buf);
            }
            DialogKind::DeleteConfirm { targets } => {
                render_confirm_dialog(targets, self.theme, area, buf);
            }
            DialogKind::Error { message } => {
                render_error_dialog(message, self.theme, area, buf);
            }
            DialogKind::Progress {
                message,
                current,
                total,
            } => {
                render_progress_dialog(message, *current, *total, self.theme, area, buf);
            }
            DialogKind::SaveConfirm | DialogKind::FocusBackConfirm => {
                render_save_confirm_dialog(
                    matches!(kind, DialogKind::FocusBackConfirm),
                    self.theme,
                    area,
                    buf,
                );
            }
            DialogKind::DocumentDecision { path, quitting, .. } => {
                let rect = DialogWidget::centered_rect(72, 9, area);
                Clear.render(rect, buf);
                let block = Block::default()
                    .title(if *quitting {
                        " Quit: Unsaved Document "
                    } else {
                        " Close: Unsaved Document "
                    })
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.warning_fg));
                let inner = block.inner(rect);
                block.render(rect, buf);
                if inner.width > 0 && inner.height > 0 {
                    let choices = [
                        "[s/y] Save",
                        "[d] Discard this buffer",
                        "[c/Esc] Cancel — keep remaining documents",
                    ];
                    let choices_height = 3.min(inner.height);
                    let message_height = inner.height.saturating_sub(choices_height);
                    Paragraph::new(path.display().to_string())
                        .style(Style::default().fg(self.theme.status_fg))
                        .wrap(Wrap { trim: false })
                        .render(
                            Rect::new(inner.x, inner.y, inner.width, message_height),
                            buf,
                        );
                    if inner.height < 3 {
                        buf.set_line(
                            inner.x,
                            inner.bottom() - 1,
                            &Line::from("[s] Save [d] Discard [c/Esc] Cancel"),
                            inner.width,
                        );
                    } else {
                        for (index, text) in choices.iter().enumerate() {
                            buf.set_line(
                                inner.x,
                                inner.bottom() - choices_height + index as u16,
                                &Line::from(*text),
                                inner.width,
                            );
                        }
                    }
                }
            }
            DialogKind::SaveConflict {
                message, normalize, ..
            } => {
                render_save_choices(message, *normalize, false, self.theme, area, buf);
            }
            DialogKind::EditorSaveAs { normalize, .. } => {
                let title = if *normalize {
                    "Save As (normalize to LF)"
                } else {
                    "Save As (new path)"
                };
                render_input_dialog(title, self.dialog_state, self.theme, area, buf);
            }
            DialogKind::SaveOverwrite { normalize, .. } => {
                render_save_choices(
                    "Replace the disk version with this buffer?",
                    *normalize,
                    true,
                    self.theme,
                    area,
                    buf,
                );
            }
            DialogKind::SaveSettings => {
                render_save_settings_dialog(self.theme, area, buf);
            }
            DialogKind::RecoveryPrompt {
                document,
                remaining,
            } => {
                render_recovery_prompt(document, *remaining, self.theme, area, buf);
            }
        }
    }
}

fn render_recovery_prompt(
    document: &std::path::Path,
    remaining: usize,
    theme: &ThemeColors,
    area: Rect,
    buf: &mut Buffer,
) {
    let dialog_width = 72u16.min(area.width.saturating_sub(4));
    let dialog_height = 8u16.min(area.height);
    let rect = DialogWidget::centered_rect(dialog_width, dialog_height, area);

    Clear.render(rect, buf);
    let block = Block::default()
        .title(" Recover Unsaved Work ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.warning_fg))
        .padding(Padding::horizontal(1));
    let inner = block.inner(rect);
    block.render(rect, buf);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let name = document
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| document.display().to_string());
    let further = remaining.saturating_sub(1);
    let further_note = if further == 0 {
        String::new()
    } else {
        let plural = if further == 1 { "" } else { "s" };
        format!(
            "\n{further} further document{plural} with snapshots remain; each is offered in turn."
        )
    };
    let message = format!(
        "A private recovery snapshot exists for {name}.\n\
         Restore this document's unsaved text? The file is not written until\n\
         you save, and declining keeps every snapshot.{further_note}"
    );
    Paragraph::new(message)
        .style(Style::default().fg(theme.status_fg))
        .wrap(Wrap { trim: false })
        .render(
            Rect::new(
                inner.x,
                inner.y,
                inner.width,
                inner.height.saturating_sub(2),
            ),
            buf,
        );
    let choices = "[r] Restore this document  [d] Discard this document  [Esc] Not now";
    buf.set_line(
        inner.x,
        inner.bottom() - 1,
        &Line::from(Span::styled(choices, Style::default().fg(theme.status_fg))),
        inner.width,
    );
}

fn render_save_choices(
    message: &str,
    normalize: bool,
    overwrite: bool,
    theme: &ThemeColors,
    area: Rect,
    buf: &mut Buffer,
) {
    let rect = DialogWidget::centered_rect(
        68.min(area.width.saturating_sub(2)),
        11.min(area.height),
        area,
    );
    Clear.render(rect, buf);
    let block = Block::default()
        .title(if overwrite {
            " Confirm Overwrite "
        } else {
            " Save Conflict "
        })
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.warning_fg))
        .padding(Padding::horizontal(1));
    let inner = block.inner(rect);
    block.render(rect, buf);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let choices: &[&str] = if overwrite {
        &["[y] Overwrite disk with buffer", "[n/c/Esc] Cancel"]
    } else {
        &[
            "[r] Reload (discard unsaved buffer)",
            "[a] Save As (new path)",
            "[o] Overwrite  [c/Esc] Cancel",
        ]
    };
    let choices_height = (choices.len() as u16).min(inner.height);
    let message_height = inner.height.saturating_sub(choices_height + 1);
    let message = if normalize {
        format!("{message}\nConfirmed save will normalize to LF.")
    } else {
        message.to_string()
    };
    Paragraph::new(message)
        .style(Style::default().fg(theme.status_fg))
        .wrap(Wrap { trim: false })
        .render(
            Rect::new(inner.x, inner.y, inner.width, message_height),
            buf,
        );
    for (index, text) in choices.iter().take(choices_height as usize).enumerate() {
        buf.set_line(
            inner.x,
            inner.bottom() - choices_height + index as u16,
            &Line::from(Span::styled(*text, Style::default().fg(theme.status_fg))),
            inner.width,
        );
    }
}

fn render_input_dialog(
    title: &str,
    state: &DialogState,
    theme: &ThemeColors,
    area: Rect,
    buf: &mut Buffer,
) {
    let dialog_width = 50.min(area.width.saturating_sub(4));
    let dialog_height = 5;
    let rect = DialogWidget::centered_rect(dialog_width, dialog_height, area);

    Clear.render(rect, buf);

    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.dialog_border_fg))
        .padding(Padding::horizontal(1));

    let inner = block.inner(rect);
    block.render(rect, buf);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Render input line with cursor
    let input = &state.input;
    let cursor_pos = state.cursor_position;
    let max_width = inner.width as usize;

    let before = input.get(..cursor_pos).unwrap_or(input.as_str());
    let cursor_suffix = input.get(cursor_pos..).unwrap_or("");
    let (cursor_char, after) = if cursor_suffix.is_empty() {
        (" ".to_string(), "")
    } else {
        let mut chars = cursor_suffix.chars();
        let ch = chars.next().unwrap_or(' ');
        (ch.to_string(), chars.as_str())
    };

    // Truncate from left if input is too long
    let before_len = before.chars().count();
    let total_len = before_len + 1 + after.chars().count();
    let before_display = if total_len > max_width && before_len > max_width.saturating_sub(2) {
        let keep = max_width.saturating_sub(2);
        before
            .chars()
            .skip(before_len.saturating_sub(keep))
            .collect::<String>()
    } else {
        before.to_string()
    };

    let input_style = Style::default().fg(theme.status_fg);
    let cursor_style = Style::default()
        .bg(theme.status_fg)
        .fg(theme.dialog_bg)
        .add_modifier(Modifier::BOLD);

    let spans = vec![
        Span::styled(before_display, input_style),
        Span::styled(cursor_char, cursor_style),
        Span::styled(after, input_style),
    ];

    let line = Line::from(spans);
    buf.set_line(inner.x, inner.y + inner.height / 2, &line, inner.width);

    // Render hint at bottom
    let hint = "[Enter] Confirm  [Esc] Cancel";
    let hint_style = Style::default()
        .fg(theme.dim_fg)
        .add_modifier(Modifier::DIM);
    let hint_line = Line::from(Span::styled(hint, hint_style));
    if inner.height > 1 {
        buf.set_line(inner.x, inner.y + inner.height - 1, &hint_line, inner.width);
    }
}

fn render_confirm_dialog(
    targets: &[std::path::PathBuf],
    theme: &ThemeColors,
    area: Rect,
    buf: &mut Buffer,
) {
    let max_name_len = targets
        .iter()
        .filter_map(|p| p.file_name())
        .map(|n| n.to_string_lossy().len())
        .max()
        .unwrap_or(10);

    let dialog_width = (max_name_len as u16 + 10)
        .max(40)
        .min(area.width.saturating_sub(4));
    let dialog_height = (targets.len() as u16 + 6).min(area.height.saturating_sub(2));
    let rect = DialogWidget::centered_rect(dialog_width, dialog_height, area);

    Clear.render(rect, buf);

    let block = Block::default()
        .title(" Delete Confirmation ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.error_fg))
        .padding(Padding::horizontal(1));

    let inner = block.inner(rect);
    block.render(rect, buf);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // "Delete the following?" header
    let header = Line::from(Span::styled(
        "Delete the following?",
        Style::default()
            .fg(theme.warning_fg)
            .add_modifier(Modifier::BOLD),
    ));
    buf.set_line(inner.x, inner.y, &header, inner.width);

    // List targets
    let max_items = (inner.height.saturating_sub(3)) as usize;
    for (i, target) in targets.iter().take(max_items).enumerate() {
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| target.to_string_lossy().to_string());
        let line = Line::from(Span::styled(
            format!("  • {}", name),
            Style::default().fg(theme.status_fg),
        ));
        buf.set_line(inner.x, inner.y + 2 + i as u16, &line, inner.width);
    }

    // Render hint at bottom
    let hint = "[y] Yes  [n/Esc] Cancel";
    let hint_style = Style::default()
        .fg(theme.dim_fg)
        .add_modifier(Modifier::DIM);
    let hint_line = Line::from(Span::styled(hint, hint_style));
    buf.set_line(inner.x, inner.y + inner.height - 1, &hint_line, inner.width);
}

fn render_error_dialog(message: &str, theme: &ThemeColors, area: Rect, buf: &mut Buffer) {
    let dialog_width = (message.len() as u16 + 6)
        .max(30)
        .min(area.width.saturating_sub(4));
    let dialog_height = 5;
    let rect = DialogWidget::centered_rect(dialog_width, dialog_height, area);

    Clear.render(rect, buf);

    let block = Block::default()
        .title(" Error ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.error_fg))
        .padding(Padding::horizontal(1));

    let inner = block.inner(rect);
    block.render(rect, buf);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Error message
    let msg_line = Line::from(Span::styled(message, Style::default().fg(theme.error_fg)));
    buf.set_line(inner.x, inner.y + inner.height / 2, &msg_line, inner.width);

    // Hint
    let hint = "[Enter/Esc] Dismiss";
    let hint_style = Style::default()
        .fg(theme.dim_fg)
        .add_modifier(Modifier::DIM);
    let hint_line = Line::from(Span::styled(hint, hint_style));
    if inner.height > 1 {
        buf.set_line(inner.x, inner.y + inner.height - 1, &hint_line, inner.width);
    }
}

fn render_progress_dialog(
    current_file: &str,
    current: usize,
    total: usize,
    theme: &ThemeColors,
    area: Rect,
    buf: &mut Buffer,
) {
    let dialog_width = 50.min(area.width.saturating_sub(4));
    let dialog_height = 6;
    let rect = DialogWidget::centered_rect(dialog_width, dialog_height, area);

    Clear.render(rect, buf);

    let title = format!(" Processing {}/{} ", current, total);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.warning_fg))
        .padding(Padding::horizontal(1));

    let inner = block.inner(rect);
    block.render(rect, buf);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Current file being processed
    let file_line = Line::from(Span::styled(
        current_file.to_string(),
        Style::default().fg(theme.status_fg),
    ));
    buf.set_line(inner.x, inner.y, &file_line, inner.width);

    // Simple progress bar
    if inner.height > 1 && total > 0 {
        let bar_width = inner.width as usize;
        let filled = (current * bar_width) / total;
        let bar: String = "█".repeat(filled) + &"░".repeat(bar_width.saturating_sub(filled));
        let bar_line = Line::from(Span::styled(bar, Style::default().fg(theme.info_fg)));
        buf.set_line(inner.x, inner.y + 1, &bar_line, inner.width);
    }

    // Hint at bottom
    let hint = "[Esc] Cancel";
    let hint_style = Style::default()
        .fg(theme.dim_fg)
        .add_modifier(Modifier::DIM);
    let hint_line = Line::from(Span::styled(hint, hint_style));
    if inner.height > 2 {
        buf.set_line(inner.x, inner.y + inner.height - 1, &hint_line, inner.width);
    }
}

fn render_save_confirm_dialog(focus_back: bool, theme: &ThemeColors, area: Rect, buf: &mut Buffer) {
    let dialog_width = 50u16.min(area.width.saturating_sub(4));
    let dialog_height = 6;
    let rect = DialogWidget::centered_rect(dialog_width, dialog_height, area);

    Clear.render(rect, buf);

    let block = Block::default()
        .title(if focus_back {
            " Leave Editor (buffer retained) "
        } else {
            " Save Buffer "
        })
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.warning_fg))
        .padding(Padding::horizontal(1));

    let inner = block.inner(rect);
    block.render(rect, buf);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Question text
    let msg = Line::from(Span::styled(
        if focus_back {
            "Save before returning to preview?"
        } else {
            "Save this buffer?"
        },
        Style::default()
            .fg(theme.status_fg)
            .add_modifier(Modifier::BOLD),
    ));
    buf.set_line(inner.x, inner.y + inner.height / 2, &msg, inner.width);

    // Hint at bottom
    let hint = "[y] Save  [n] Keep buffer  [c/Esc] Cancel";
    let hint_style = Style::default()
        .fg(theme.dim_fg)
        .add_modifier(Modifier::DIM);
    let hint_line = Line::from(Span::styled(hint, hint_style));
    if inner.height > 1 {
        buf.set_line(inner.x, inner.y + inner.height - 1, &hint_line, inner.width);
    }
}

fn render_save_settings_dialog(theme: &ThemeColors, area: Rect, buf: &mut Buffer) {
    let dialog_width = 60u16.min(area.width.saturating_sub(4));
    let dialog_height = 7;
    let rect = DialogWidget::centered_rect(dialog_width, dialog_height, area);

    Clear.render(rect, buf);

    let block = Block::default()
        .title(" Save Settings ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.info_fg))
        .padding(Padding::horizontal(1));

    let inner = block.inner(rect);
    block.render(rect, buf);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Question text
    let msg = Line::from(Span::styled(
        "Save modified settings to:",
        Style::default()
            .fg(theme.status_fg)
            .add_modifier(Modifier::BOLD),
    ));
    buf.set_line(inner.x, inner.y, &msg, inner.width);

    // Global option
    let global = Line::from(vec![
        Span::styled(
            "  [G] ",
            Style::default()
                .fg(theme.warning_fg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "Global  (~/.config/fm-tui/config.toml)",
            Style::default().fg(theme.tree_file_fg),
        ),
    ]);
    if inner.height > 1 {
        buf.set_line(inner.x, inner.y + 1, &global, inner.width);
    }

    // Local option
    let local = Line::from(vec![
        Span::styled(
            "  [L] ",
            Style::default()
                .fg(theme.warning_fg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "Local   (.fm-tui.toml in current directory)",
            Style::default().fg(theme.tree_file_fg),
        ),
    ]);
    if inner.height > 2 {
        buf.set_line(inner.x, inner.y + 2, &local, inner.width);
    }

    // Hint at bottom
    let hint = "[g] Global  [l] Local  [c/Esc] Cancel";
    let hint_style = Style::default()
        .fg(theme.dim_fg)
        .add_modifier(Modifier::DIM);
    let hint_line = Line::from(Span::styled(hint, hint_style));
    if inner.height > 3 {
        buf.set_line(inner.x, inner.y + inner.height - 1, &hint_line, inner.width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;
    use std::path::PathBuf;

    fn test_theme() -> ThemeColors {
        theme::dark_theme()
    }

    #[test]
    fn recovery_prompt_names_the_document_and_never_claims_to_discard_all() {
        let document = std::path::PathBuf::from("/tmp/workspace/notes.yaml");
        let mode = AppMode::Dialog(DialogKind::RecoveryPrompt {
            document: document.clone(),
            remaining: 3,
        });
        let state = DialogState::default();
        let tc = test_theme();
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        DialogWidget::new(&mode, &state, &tc).render(area, &mut buf);
        let content = buffer_to_string(&buf, area);
        assert!(content.contains("Recover Unsaved Work"), "{content}");
        // The offer names the specific document it acts on.
        assert!(content.contains("notes.yaml"), "{content}");
        // It discloses that the multi-document set is offered in turn, counting
        // documents (not raw records).
        assert!(
            content.contains("2 further documents with snapshots remain"),
            "{content}"
        );
        // P2-2: no "all" wording on a single-document action.
        assert!(!content.to_lowercase().contains("discard all"), "{content}");
        assert!(content.contains("[r] Restore this document"), "{content}");
        assert!(content.contains("[d] Discard this document"), "{content}");
        for (width, height) in [(0, 0), (1, 1), (3, 2), (20, 4)] {
            let area = Rect::new(2, 3, width, height);
            let mut buf = Buffer::empty(area);
            DialogWidget::new(&mode, &state, &tc).render(area, &mut buf);
        }

        // A single remaining offer omits the "further" disclosure.
        let single = AppMode::Dialog(DialogKind::RecoveryPrompt {
            document,
            remaining: 1,
        });
        let mut buf = Buffer::empty(area);
        DialogWidget::new(&single, &state, &tc).render(area, &mut buf);
        let content = buffer_to_string(&buf, area);
        assert!(!content.contains("further"), "{content}");
    }

    #[test]
    fn document_lifecycle_dialog_long_payload_keeps_choices_and_tiny_geometry_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let mut parent = dir.path().to_path_buf();
        for _ in 0..4 {
            parent.push("long-context-".repeat(10));
        }
        std::fs::create_dir_all(&parent).unwrap();
        let path = parent.join("document.txt");
        std::fs::write(&path, "text").unwrap();
        let mut store = crate::workspace::documents::DocumentStore::new();
        let id = store
            .open(&path, crate::workspace::documents::OpenDisposition::Pinned)
            .unwrap();
        let tc = test_theme();
        let state = DialogState::default();
        for quitting in [false, true] {
            let mode = AppMode::Dialog(DialogKind::DocumentDecision {
                id,
                path: path.clone(),
                quitting,
            });
            let area = Rect::new(0, 0, 80, 24);
            let mut buf = Buffer::empty(area);
            DialogWidget::new(&mode, &state, &tc).render(area, &mut buf);
            let content = buffer_to_string(&buf, area);
            assert!(content.contains("[s/y] Save"));
            assert!(content.contains("[d] Discard"));
            assert!(content.contains("[c/Esc] Cancel"));
            assert!(content.contains(dir.path().to_str().unwrap()));
            for (w, h) in [(0, 0), (1, 1), (2, 2), (3, 5), (8, 3), (16, 6)] {
                let mut buf = Buffer::empty(area);
                for cell in &mut buf.content {
                    cell.set_symbol(".");
                }
                let small = Rect::new(3, 2, w, h);
                DialogWidget::new(&mode, &state, &tc).render(small, &mut buf);
                for y in 0..area.height {
                    for x in 0..area.width {
                        if !small.contains((x, y).into()) {
                            assert_eq!(buf[(x, y)].symbol(), ".");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn test_input_dialog_renders() {
        let mode = AppMode::Dialog(DialogKind::CreateFile);
        let state = DialogState {
            input: "test.txt".to_string(),
            cursor_position: 8,
        };
        let tc = test_theme();
        let widget = DialogWidget::new(&mode, &state, &tc);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        // Check that the dialog title appears
        let content = buffer_to_string(&buf, area);
        assert!(content.contains("Create New File"));
        assert!(content.contains("test.txt"));
    }

    #[test]
    fn test_rename_dialog_renders() {
        let mode = AppMode::Dialog(DialogKind::Rename {
            original: PathBuf::from("/tmp/old_name.txt"),
        });
        let state = DialogState {
            input: "old_name.txt".to_string(),
            cursor_position: 12,
        };
        let tc = test_theme();
        let widget = DialogWidget::new(&mode, &state, &tc);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content = buffer_to_string(&buf, area);
        assert!(content.contains("Rename"));
        assert!(content.contains("old_name.txt"));
    }

    #[test]
    fn test_input_dialog_renders_unicode_without_panic() {
        let mode = AppMode::Dialog(DialogKind::CreateFile);
        let input = "aé한";
        let state = DialogState {
            input: input.to_string(),
            cursor_position: "a".len(),
        };
        let tc = test_theme();
        let widget = DialogWidget::new(&mode, &state, &tc);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content = buffer_to_string(&buf, area);
        assert!(content.contains("Create New File"));
        assert!(content.contains(input));
    }

    #[test]
    fn test_confirm_dialog_renders() {
        let targets = vec![
            PathBuf::from("/tmp/file1.txt"),
            PathBuf::from("/tmp/file2.txt"),
        ];
        let mode = AppMode::Dialog(DialogKind::DeleteConfirm {
            targets: targets.clone(),
        });
        let state = DialogState::default();
        let tc = test_theme();
        let widget = DialogWidget::new(&mode, &state, &tc);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content = buffer_to_string(&buf, area);
        assert!(content.contains("Delete"));
        assert!(content.contains("file1.txt"));
        assert!(content.contains("file2.txt"));
    }

    #[test]
    fn test_error_dialog_renders() {
        let mode = AppMode::Dialog(DialogKind::Error {
            message: "Permission denied".to_string(),
        });
        let state = DialogState::default();
        let tc = test_theme();
        let widget = DialogWidget::new(&mode, &state, &tc);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        let content = buffer_to_string(&buf, area);
        assert!(content.contains("Error"));
        assert!(content.contains("Permission denied"));
    }

    #[test]
    fn test_no_dialog_mode_noop() {
        let mode = AppMode::Normal;
        let state = DialogState::default();
        let tc = test_theme();
        let widget = DialogWidget::new(&mode, &state, &tc);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        // Buffer should be empty (all spaces)
        let content = buffer_to_string(&buf, area);
        assert!(content.trim().is_empty());
    }

    #[test]
    fn save_conflict_exposes_non_destructive_choices() {
        let mode = AppMode::Dialog(DialogKind::SaveConflict {
            message: "Save failed: file changed: config.yaml".to_string(),
            exit_after_save: false,
            normalize: false,
        });
        let state = DialogState::default();
        let tc = test_theme();
        for (width, height) in [(120, 40), (80, 24), (60, 20)] {
            let area = Rect::new(0, 0, width, height);
            let mut buf = Buffer::empty(area);
            DialogWidget::new(&mode, &state, &tc).render(area, &mut buf);
            let content = buffer_to_string(&buf, area);
            for choice in ["Reload", "Save As", "Overwrite", "Cancel"] {
                assert!(content.contains(choice), "{content}");
            }
        }
    }

    #[test]
    fn overwrite_dialog_warns_about_discarding_disk_and_normalization() {
        let mode = AppMode::Dialog(DialogKind::SaveOverwrite {
            exit_after_save: false,
            normalize: true,
            expected_revision: None,
        });
        let state = DialogState::default();
        let tc = test_theme();
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        DialogWidget::new(&mode, &state, &tc).render(area, &mut buf);
        let content = buffer_to_string(&buf, area);
        assert!(content.contains("Overwrite"));
        assert!(content.contains("LF"));
        assert!(content.contains("[y]"));
        assert!(content.contains("Cancel"));
        assert!(!content.contains("Discard"));
    }

    #[test]
    fn save_dialogs_are_bounded_in_tiny_offset_areas() {
        let kinds = [
            DialogKind::SaveConflict {
                message: "External changes".to_string(),
                exit_after_save: false,
                normalize: true,
            },
            DialogKind::SaveOverwrite {
                exit_after_save: false,
                normalize: true,
                expected_revision: None,
            },
            DialogKind::EditorSaveAs {
                exit_after_save: false,
                normalize: true,
            },
        ];
        let tc = test_theme();
        let state = DialogState::default();
        for kind in kinds {
            for (width, height) in [(0, 0), (1, 1), (3, 2), (20, 4)] {
                let area = Rect::new(2, 3, width, height);
                let mut buf = Buffer::empty(area);
                DialogWidget::new(&AppMode::Dialog(kind.clone()), &state, &tc)
                    .render(area, &mut buf);
            }
        }
    }

    fn buffer_to_string(buf: &Buffer, area: Rect) -> String {
        let mut s = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                s.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            s.push('\n');
        }
        s
    }
}
