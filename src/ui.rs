use ratatui::{
    style::Style,
    widgets::{Block, Borders},
    Frame,
};

use crate::app::{App, AppMode, FocusedPanel};
use crate::components::content_search::ContentSearchWidget;
use crate::components::dialog::DialogWidget;
use crate::components::editor::EditorWidget;
use crate::components::help::HelpOverlay;
use crate::components::preview::PreviewWidget;
use crate::components::search::SearchWidget;
use crate::components::search_action::SearchActionWidget;
use crate::components::status_bar::StatusBarWidget;
use crate::components::terminal::TerminalWidget;
use crate::components::tree::TreeWidget;
use crate::fs::tree::NodeType;

/// Render the application UI.
pub fn render(app: &mut App, frame: &mut Frame) {
    render_in_area(app, frame, frame.area());
}

/// Render a bounded workspace within a larger host viewport.
pub fn render_in_area(app: &mut App, frame: &mut Frame, area: ratatui::layout::Rect) {
    // Update preview when selection changes
    app.update_preview();

    let area = area.intersection(frame.area());
    let theme = app.theme_colors.clone();

    app.update_workspace_geometry(area);
    let rects = app.workspace_rects;
    let tree_area = rects.explorer;
    let preview_area = rects.document;
    let status_area = rects.status;
    let summaries = crate::components::document_tabs::summaries_with_read_only(
        &app.workspace.documents,
        |_| app.is_s3_mode(),
    );
    app.document_tabs = crate::components::document_tabs::TabLayout::new(&summaries, rects.tabs);
    frame.render_widget(
        app.document_tabs.widget(&theme, !app.config.use_icons()),
        rects.tabs,
    );
    app.clamp_preview_scroll();

    // Determine border styles based on focus (using theme colors)
    let focused_border = Style::default().fg(theme.border_focused_fg);
    let unfocused_border = Style::default().fg(theme.border_fg);
    // S3 mode: tint the tree border with S3-specific color for visual context
    let tree_focused_border = if app.is_s3_mode() {
        Style::default().fg(theme.s3_border_fg)
    } else {
        focused_border
    };

    let (tree_border_style, preview_border_style, terminal_border_style) =
        match app.workspace.focus.panel {
            FocusedPanel::Tree => (tree_focused_border, unfocused_border, unfocused_border),
            FocusedPanel::Preview | FocusedPanel::Editor => {
                (unfocused_border, focused_border, unfocused_border)
            }
            FocusedPanel::Terminal => (unfocused_border, unfocused_border, focused_border),
        };

    // Update scroll offset to keep selected item visible,
    // but only if the viewport wasn't explicitly scrolled by mouse/scrollbar.
    let visible_height = app.tree_content_area.height as usize;
    app.tree_visible_height = visible_height;
    if !app.tree_viewport_locked {
        app.tree_state.update_scroll(visible_height);
    }

    let tree_block = Block::default()
        .title(format!(" {} ", app.tree_state.root.name))
        .borders(Borders::ALL)
        .border_style(tree_border_style);

    let tree_widget = TreeWidget::new(&app.tree_state, &theme, app.config.use_icons())
        .s3_mode(app.is_s3_mode())
        .block(tree_block.clone());
    // Store scrollbar column for mouse hit testing
    app.scrollbar_column = tree_widget.scrollbar_x(tree_area);
    let mut tree_widget = TreeWidget::new(&app.tree_state, &theme, app.config.use_icons())
        .s3_mode(app.is_s3_mode())
        .block(tree_block);
    if let Some((git_root, snapshot)) = app.git_render() {
        tree_widget = tree_widget.git(git_root, snapshot);
    }
    frame.render_widget(tree_widget, tree_area);

    // Render preview panel (or editor if in edit mode)
    if app.editor_visible() {
        // Edit mode: render editor widget
        let dirty = app
            .workspace
            .documents
            .active()
            .map(|d| &d.editor)
            .is_some_and(|e| e.modified);
        let editor_title = match app.workspace.documents.active().map(|d| d.path()) {
            Some(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Editor".to_string());
                if dirty {
                    format!(
                        " {} {} [EDIT] ",
                        name,
                        if app.config.use_icons() { "●" } else { "*" }
                    )
                } else {
                    format!(" {} [EDIT] ", name)
                }
            }
            None => " Editor ".to_string(),
        };

        let editor_block = Block::default()
            .title(editor_title)
            .borders(Borders::ALL)
            .border_style(preview_border_style);

        // Update usable dimensions before rendering, but freeze a suppressed view.
        if let Some(editor) = app.workspace.documents.active_mut().map(|d| &mut d.editor) {
            let inner_height = app.preview_content_area.height as usize;
            let height = inner_height.saturating_sub(editor.find_bar_height(inner_height));
            let width = app
                .preview_content_area
                .width
                .saturating_sub(editor.gutter_width()) as usize;
            if width > 0 && height > 0 {
                editor.update_viewport(width, height);
            }
        }

        if let Some(editor) = app.workspace.documents.active().map(|d| &d.editor) {
            let mut editor_widget =
                EditorWidget::new(editor, &theme, &app.syntax_set, &app.syntax_theme)
                    .prepared_only(app.prepared_pipeline);
            if let Some(id) = app.workspace.documents.active_id() {
                if let Some(cache) = app.syntax_cache_for(id) {
                    editor_widget = editor_widget.prepared(cache);
                }
            }
            let diagnostic_lines = app
                .workspace
                .documents
                .active()
                .map(|document| crate::lsp::features::uri_for_path(document.path()))
                .filter(|uri| app.diagnostics.contains(uri))
                .map(|uri| app.diagnostics.lines_for(&uri));
            if let Some(lines) = diagnostic_lines.as_ref().filter(|lines| !lines.is_empty()) {
                editor_widget = editor_widget.diagnostic_lines(lines);
            }
            frame.render_widget(editor_widget.block(editor_block), preview_area);
        }
    } else if !app.config.preview_enabled() {
        frame.render_widget(
            ratatui::widgets::Paragraph::new(
                "Automatic preview disabled\nEnter/e opens supported text",
            )
            .block(
                Block::default()
                    .title(" Preview disabled ")
                    .borders(Borders::ALL)
                    .border_style(preview_border_style),
            ),
            preview_area,
        );
    } else {
        // Normal preview mode
        let preview_title = match &app.preview_state.current_path {
            Some(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Preview".to_string());
                if path.extension().and_then(|e| e.to_str()) == Some("ipynb") {
                    let cell_count = app
                        .preview_state
                        .content_lines
                        .iter()
                        .filter(|l| {
                            l.spans
                                .first()
                                .map(|s| s.content.starts_with('━'))
                                .unwrap_or(false)
                        })
                        .count();
                    format!(" Notebook: {} cells ", cell_count)
                } else {
                    format!(" {} [PREVIEW] ", name)
                }
            }
            None => " Preview ".to_string(),
        };

        let preview_block = Block::default()
            .title(preview_title)
            .borders(Borders::ALL)
            .border_style(preview_border_style);

        let preview_widget = PreviewWidget::new(&app.preview_state, &theme)
            .selection(&app.preview_selection)
            .block(preview_block);
        frame.render_widget(preview_widget, preview_area);
    }

    // Separate header/content leaves: no border inset or second geometry authority.
    let terminal_title = if app.terminal_state.exited {
        " Terminal [exited] "
    } else {
        " Terminal "
    };
    frame.render_widget(
        ratatui::widgets::Paragraph::new(terminal_title)
            .style(terminal_border_style.bg(theme.status_bg)),
        rects.terminal_header,
    );
    frame.render_widget(
        TerminalWidget::new(
            &app.terminal_state,
            &theme,
            app.workspace.focus.panel == FocusedPanel::Terminal,
        ),
        rects.terminal_content,
    );
    frame.render_widget(
        ratatui::widgets::Paragraph::new(
            (0..rects.explorer_split.height)
                .map(|_| ratatui::text::Line::raw(if app.config.use_icons() { "│" } else { "|" }))
                .collect::<Vec<_>>(),
        )
        .style(unfocused_border),
        rects.explorer_split,
    );
    frame.render_widget(
        ratatui::widgets::Paragraph::new(
            (if app.config.use_icons() { "─" } else { "-" })
                .repeat(rects.terminal_split.width as usize),
        )
        .style(unfocused_border),
        rects.terminal_split,
    );

    // Clear expired status messages
    app.clear_expired_status();

    // Build status bar
    let selected_item = app.tree_state.flat_items.get(app.tree_state.selected_index);

    let presented_document = app
        .editor_visible()
        .then(|| app.workspace.documents.active())
        .flatten();
    let presented_path = presented_document
        .map(|d| d.path())
        .or(app.preview_state.current_path.as_deref())
        .or_else(|| selected_item.map(|item| item.path.as_path()))
        .unwrap_or(std::path::Path::new(""));
    let path_str = presented_path.to_string_lossy().to_string();
    app.breadcrumbs = crate::components::workspace_chrome::BreadcrumbLayout::new(
        presented_path,
        rects.breadcrumbs,
        !app.config.use_icons(),
    );
    frame.render_widget(app.breadcrumbs.widget(&theme), rects.breadcrumbs);

    let file_info = selected_item
        .map(|item| match item.node_type {
            NodeType::Directory => {
                if let Some(count) = item.child_count {
                    if let Some(remaining) = item.load_more_remaining {
                        format!("Dir ({}/{} loaded)", count - remaining, count)
                    } else {
                        format!("Dir ({} items)", count)
                    }
                } else {
                    "Dir".to_string()
                }
            }
            NodeType::File => "File".to_string(),
            NodeType::Symlink => "Symlink".to_string(),
            NodeType::LoadMore => {
                if let Some(remaining) = item.load_more_remaining {
                    format!("Load more... (~{} remaining)", remaining)
                } else {
                    "Load more...".to_string()
                }
            }
            NodeType::Loading => "Loading...".to_string(),
        })
        .unwrap_or_default();

    let mut status_widget = StatusBarWidget::new(&path_str, &file_info, &theme);
    use crate::components::workspace_chrome::{DocumentIndicators, DocumentStatus};
    if let Some(document) = presented_document {
        let editor = &document.editor;
        let column = editor.buffer.get(editor.cursor_line).map_or(0, |line| {
            crate::text::byte_to_display_col(line, editor.cursor_col, crate::text::TAB_WIDTH)
        });
        let language = document
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .and_then(|extension| app.syntax_set.find_syntax_by_extension(extension))
            .map_or("Plain Text", |syntax| syntax.name.as_str());
        status_widget = status_widget.document_status(DocumentStatus {
            path: &path_str,
            line: editor.cursor_line + 1,
            column: column + 1,
            language,
            encoding: "UTF-8",
            line_ending: match editor.line_ending {
                crate::editor::LineEnding::Lf => "LF",
                crate::editor::LineEnding::CrLf => "CRLF",
            },
            indicators: DocumentIndicators {
                dirty: editor.modified,
                external_change: document.has_external_change(),
                read_only: app.is_s3_mode(),
                temporary: !document.is_pinned(),
            },
        });
    } else if app.preview_state.current_path.is_some() {
        status_widget = status_widget.document_status(DocumentStatus {
            path: &path_str,
            line: 1,
            column: 1,
            language: "Preview",
            encoding: "—",
            line_ending: "—",
            indicators: DocumentIndicators {
                read_only: true,
                temporary: true,
                ..Default::default()
            },
        });
    }

    // Native controls are separate from configurable workspace routes.
    let key_hints_str = match app.workspace.focus.panel {
        FocusedPanel::Tree => " ↑↓:navigate ?:help ".to_string(),
        FocusedPanel::Preview => " j/k:scroll e:edit ?:help ".to_string(),
        FocusedPanel::Editor => {
            let labels = app.keymap.binding_labels(
                crate::commands::CommandId::Save,
                app.command_entry_context(),
            );
            format!(
                " {}:save Ctrl+F:find Esc:preview ",
                if labels.is_empty() {
                    "unbound".into()
                } else {
                    labels.join("/")
                }
            )
        }
        FocusedPanel::Terminal => " Shell text/edit keys unless explicitly bound ".to_string(),
    };
    status_widget = status_widget.key_hints(&key_hints_str);

    // Show clipboard info if clipboard has content
    let clipboard_info_str;
    if !app.clipboard.is_empty() {
        use crate::fs::clipboard::ClipboardOp;
        let icon = match (app.clipboard.operation, app.config.use_icons()) {
            (Some(ClipboardOp::Copy), true) => "📋",
            (Some(ClipboardOp::Cut), true) => "✂",
            (Some(ClipboardOp::Copy), false) => "Copy",
            (Some(ClipboardOp::Cut), false) => "Cut",
            (None, _) => "",
        };
        clipboard_info_str = format!(
            "{} {} item{}",
            icon,
            app.clipboard.len(),
            if app.clipboard.len() == 1 { "" } else { "s" }
        );
        status_widget = status_widget.clipboard_info(&clipboard_info_str);
    }

    // Show watcher status indicator
    let watcher_indicator = if !app.config.use_icons() {
        if app.is_s3_mode() {
            "S3"
        } else if app.watcher_active {
            "Auto"
        } else {
            "Manual"
        }
        .to_string()
    } else if app.is_s3_mode() {
        "☁ S3".to_string()
    } else if app.watcher_active {
        "👁 Auto".to_string()
    } else {
        "⏸ Manual".to_string()
    };
    status_widget = status_widget.watcher_status(&watcher_indicator);

    // Read-only Git branch/detached/unborn state. Independent of any document.
    let git_branch_label = app.git_snapshot().map(|snapshot| {
        if snapshot.branch.is_unborn() {
            format!("{} (unborn)", snapshot.branch.label())
        } else if snapshot.branch.is_detached() {
            "HEAD (detached)".to_string()
        } else {
            snapshot.branch.label().to_string()
        }
    });
    if let Some(branch) = git_branch_label.as_deref() {
        status_widget = status_widget.git_branch(branch);
    }

    // Severity summary for the active document's published diagnostics;
    // worst severity picks the color. Absent when clean.
    let diagnostics_summary = presented_document
        .map(|document| crate::lsp::features::uri_for_path(document.path()))
        .map(|uri| app.diagnostics.summary(&uri))
        .filter(|summary| summary.total() > 0);
    let diagnostics_label = diagnostics_summary.as_ref().map(|summary| summary.label());
    if let (Some(summary), Some(label)) = (diagnostics_summary, diagnostics_label.as_deref()) {
        let color = if summary.errors > 0 {
            theme.error_fg
        } else if summary.warnings > 0 {
            theme.warning_fg
        } else if summary.informations > 0 {
            theme.editor_line_nr_current
        } else {
            theme.editor_line_nr
        };
        status_widget = status_widget.diagnostics(label, color);
    }

    // Show filter query in status bar when filtering
    let filter_display;
    if app.workspace.focus.overlay == AppMode::Filter || app.tree_state.is_filtering {
        filter_display = format!("Filter: {}_", app.tree_state.filter_query);
        status_widget = status_widget.status_message(&filter_display, false);
    } else if let Some((ref msg, _)) = app.status_message {
        let is_error = msg.starts_with("Error");
        status_widget = status_widget.status_message(msg, is_error);
    }
    // Explicit mouse entry shares its exact rendered rectangle with hit testing.
    let labels = app.keymap.binding_labels(
        crate::commands::CommandId::Commands,
        app.command_entry_context(),
    );
    let entry_label = if labels.is_empty() {
        "[Commands]".into()
    } else {
        format!("[Commands {}]", labels.join("/"))
    };
    let entry_width = status_area.width.min(
        unicode_width::UnicodeWidthStr::width(entry_label.as_str()).min(u16::MAX as usize) as u16,
    );
    app.command_entry_area = ratatui::layout::Rect::new(
        status_area.right().saturating_sub(entry_width),
        status_area.y,
        entry_width,
        status_area.height,
    );
    frame.render_widget(status_widget.reserved_right(entry_width), status_area);
    frame.render_widget(
        ratatui::widgets::Paragraph::new(entry_label)
            .style(Style::default().fg(theme.border_focused_fg)),
        app.command_entry_area,
    );

    if app.workspace.focus.overlay == AppMode::CommandMenu {
        if let Some(mut menu) = app.command_menu.take() {
            if area == frame.area() {
                menu.render(app, frame);
            } else {
                menu.render_in_area(app, frame, area);
            }
            app.command_menu = Some(menu);
        }
    }

    if app.workspace.focus.overlay == AppMode::LanguageFeatures {
        if let Some(mut features) = app.language_features.take() {
            features.render(app, frame);
            app.language_features = Some(features);
        }
    }

    if app.workspace.focus.overlay == AppMode::Diagnostics {
        if let Some(mut panel) = app.diagnostics_panel.take() {
            panel.render(&app.diagnostics, &theme, frame);
            app.diagnostics_panel = Some(panel);
        }
    }

    // Render dialog overlay on top if in dialog mode
    if matches!(app.workspace.focus.overlay, AppMode::Dialog(_)) {
        let dialog_widget =
            DialogWidget::new(&app.workspace.focus.overlay, &app.dialog_state, &theme);
        frame.render_widget(dialog_widget, area);
    }

    // Render search overlay on top if in search mode
    if app.workspace.focus.overlay == AppMode::Search {
        if let Some(ids) = &app.document_list {
            use ratatui::{
                text::Line,
                widgets::{Clear, Paragraph},
            };
            let width = area.width.min(90);
            let height = area.height.min(20);
            let overlay = ratatui::layout::Rect::new(
                area.x + area.width.saturating_sub(width) / 2,
                area.y + area.height.saturating_sub(height) / 2,
                width,
                height,
            );
            frame.render_widget(Clear, overlay);
            let block = Block::default()
                .title(" Open documents: ↑/↓ Enter activate Esc cancel ")
                .borders(Borders::ALL);
            let inner = block.inner(overlay);
            frame.render_widget(block, overlay);
            let rows = inner.height.saturating_sub(1) as usize;
            let start = app
                .document_list_index
                .saturating_sub(rows.saturating_sub(1));
            let lines: Vec<Line> = ids
                .iter()
                .enumerate()
                .skip(start)
                .take(rows)
                .filter_map(|(i, id)| {
                    let tab = summaries.iter().find(|t| t.id == *id)?;
                    Some(Line::styled(
                        format!(
                            "{} {} — {}",
                            if i == app.document_list_index {
                                ">"
                            } else {
                                " "
                            },
                            tab.label,
                            tab.path.display()
                        ),
                        if i == app.document_list_index {
                            Style::default().fg(theme.border_focused_fg)
                        } else {
                            Style::default()
                        },
                    ))
                })
                .collect();
            frame.render_widget(Paragraph::new(lines), inner);
            if inner.height > 0 {
                let footer =
                    ratatui::layout::Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
                frame.render_widget(
                    Paragraph::new("Alt+P pin | Alt+B/N tabs | Alt+R reveal (after activation)"),
                    footer,
                );
            }
        } else if app.content_search_active {
            let content_widget = ContentSearchWidget::new(&app.content_search, &theme);
            frame.render_widget(content_widget, area);
            if area.height > 0 {
                frame.render_widget(ratatui::widgets::Paragraph::new("Content search: Enter opens hit at line | Tab filename mode | Alt+Enter or F2 actions | Esc cancel"),
                    ratatui::layout::Rect::new(area.x, area.bottom()-1, area.width,1));
            }
        } else {
            let search_widget = SearchWidget::new(&app.search_state, &theme);
            frame.render_widget(search_widget, area);
            if area.height > 0 {
                frame.render_widget(ratatui::widgets::Paragraph::new("Quick Open: Enter opens file / navigates directory | Tab content search | Alt+Enter or F2 actions | Esc cancel"),
                    ratatui::layout::Rect::new(area.x, area.bottom()-1, area.width,1));
            }
        }
    }

    // Render search action overlay on top if in search action mode
    if app.workspace.focus.overlay == AppMode::SearchAction {
        if let Some(ref state) = app.search_action_state {
            let action_widget = SearchActionWidget::new(state, &theme);
            frame.render_widget(action_widget, area);
        }
    }

    // Render help overlay on top if in help mode
    if app.workspace.focus.overlay == AppMode::Help {
        // Update settings scroll if on settings tab
        if app.help_state.active_tab == crate::components::help::HelpTab::Settings {
            // Estimate visible content height from area (80% of screen height, minus borders, tab bar, separator)
            let overlay_height = (area.height as f32 * 0.80).min(50.0) as usize;
            let content_height = overlay_height.saturating_sub(4); // border(2) + tab bar(1) + separator(1)
            if let Some(ref mut settings) = app.help_state.settings_state {
                let total_lines = {
                    use crate::components::settings::SettingsWidget;
                    let widget = SettingsWidget::new(settings, &theme);
                    widget.total_lines()
                };
                settings.update_scroll(content_height, total_lines);
            }
        }
        let help_widget = HelpOverlay::new(&theme, &app.help_state).app(app);
        frame.render_widget(help_widget, area);
    }

    // A viewport, not a reflowed copy payload: no inserted soft newlines,
    // line-number prefixes, centering spaces, or trimming of copied text.
    if app.workspace.focus.overlay == AppMode::CopyOverlay {
        if let Some(ref text) = app.copy_overlay_text {
            use ratatui::text::Line;
            use ratatui::widgets::{Clear, Paragraph};
            let width = area.width.saturating_sub(4).max(area.width.min(4));
            let height = area.height.saturating_sub(2).max(area.height.min(2));
            let overlay = ratatui::layout::Rect::new(
                area.x + area.width.saturating_sub(width) / 2,
                area.y + area.height.saturating_sub(height) / 2,
                width,
                height,
            );
            frame.render_widget(Clear, overlay);
            let block = Block::default().title(" Copy text ").borders(Borders::ALL);
            let inner = block.inner(overlay);
            frame.render_widget(block, overlay);
            let content_height = inner.height.saturating_sub(2);
            let content = ratatui::layout::Rect::new(inner.x, inner.y, inner.width, content_height);
            let lines: Vec<Line> = text.split('\n').map(Line::raw).collect();
            let max_y = lines.len().saturating_sub(usize::from(content_height));
            let max_x = text
                .split('\n')
                .map(|line| {
                    crate::text::byte_to_display_col(line, line.len(), crate::text::TAB_WIDTH)
                })
                .max()
                .unwrap_or(0)
                .saturating_sub(usize::from(inner.width));
            app.copy_overlay_scroll.0 = app
                .copy_overlay_scroll
                .0
                .min(max_y.min(u16::MAX as usize) as u16);
            app.copy_overlay_scroll.1 = app
                .copy_overlay_scroll
                .1
                .min(max_x.min(u16::MAX as usize) as u16);
            frame.render_widget(
                Paragraph::new(lines).scroll(app.copy_overlay_scroll),
                content,
            );
            let instructions = if max_y > 0 || max_x > 0 {
                "Partial text: arrows scroll; manual copy covers visible text only"
            } else {
                "Select text → Ctrl+C (trailing newline not guaranteed by browser)"
            };
            let footer = ratatui::layout::Rect::new(
                inner.x,
                inner.y + content_height,
                inner.width,
                inner.height.min(2),
            );
            frame.render_widget(
                Paragraph::new(vec![
                    Line::raw(instructions),
                    Line::raw("Enter/Esc returns; clipboard reads unavailable"),
                ]),
                footer,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn task3_content_search_overlay_renders_widget_and_hint() {
        use super::{render, App};
        use ratatui::{backend::TestBackend, Terminal};
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.open_search();
        app.open_content_search();
        app.content_search.query = "needle".into();
        app.content_search.cursor_position = 6;
        app.content_search.hits.push(crate::search::SearchHit {
            path: dir.path().join("a.txt"),
            line: 1,
            byte: 0,
            column: 0,
            excerpt: "needle".into(),
        });
        app.content_search.complete = true;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(&mut app, frame)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Content Search"));
        assert!(text.contains("Content search: Enter opens hit"));
    }

    #[test]
    fn language_features_overlay_renders_title_rows_and_hint() {
        use super::{render, App};
        use crate::components::language_features::{FeatureView, LanguageFeatures};
        use crate::lsp::features::{CompletionEdit, CompletionEntry};
        use ratatui::{backend::TestBackend, Terminal};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.rs");
        std::fs::write(&path, "let x = 1\n").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
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
                items: vec![CompletionEntry {
                    label: "complete_me".into(),
                    detail: None,
                    kind: None,
                    documentation: None,
                    edit: CompletionEdit::Insert { text: "x".into() },
                    additional_edits: vec![],
                    snippet: false,
                    has_command: false,
                    deprecated: false,
                }],
            },
        ));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(&mut app, frame)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Completion"), "{text}");
        assert!(text.contains("complete_me"), "{text}");
        // The take/restore arm leaves the overlay owned by the app.
        assert!(app.language_features.is_some());
    }

    #[test]
    fn diagnostics_overlay_status_segment_and_gutter_render() {
        use super::{render, App};
        use ratatui::{backend::TestBackend, Terminal};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.rs");
        std::fs::write(&path, "let x = 1\nlet y = 2\n").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.open_document_path(&path, true);
        let uri = crate::lsp::features::uri_for_path(&path);
        let theme = crate::theme::dark_theme();
        let publish = |severity, message: &str| crate::diagnostics::Publish {
            language: "rust".into(),
            generation: 0,
            uri: uri.clone(),
            version: None,
            diagnostics: vec![crate::diagnostics::Diagnostic {
                start_line: 1,
                start_character: 0,
                end_line: 1,
                end_character: 1,
                severity,
                code: None,
                source: Some("t".into()),
                message: message.into(),
            }],
        };

        // Every severity paints the status segment and the gutter number in
        // its own color (worst wins: the entries replace per publish).
        use crate::diagnostics::Severity;
        use ratatui::style::Modifier;
        for (severity, label, color, modifier) in [
            (Severity::Error, "E:1", theme.error_fg, Modifier::BOLD),
            (Severity::Warning, "W:1", theme.warning_fg, Modifier::BOLD),
            (
                Severity::Information,
                "I:1",
                theme.editor_line_nr_current,
                Modifier::empty(),
            ),
            (
                Severity::Hint,
                "H:1",
                theme.editor_line_nr,
                Modifier::UNDERLINED,
            ),
        ] {
            app.diagnostics.apply(publish(severity, "boom"), None);
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal.draw(|frame| render(&mut app, frame)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains(label), "{label} in {text}");
            let painted = terminal.backend().buffer().content.iter().any(|cell| {
                cell.fg == color && cell.symbol() == "2" && cell.modifier.contains(modifier)
            });
            assert!(painted, "{label} gutter painted in its severity style");
        }

        // Panel overlay renders its severity title and rows.
        app.diagnostics
            .apply(publish(Severity::Error, "boom"), None);
        app.toggle_diagnostics_panel().unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(&mut app, frame)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Diagnostics · E:1"), "{text}");
        assert!(text.contains("boom"), "{text}");
        assert!(app.diagnostics_panel.is_some());
    }

    #[test]
    fn review_fix_s3_terminal_availability_matches_lifecycle_gate() {
        use crate::commands::{dispatch_command, unavailable_reason, CommandContext, CommandId};
        use crate::keymap::KeymapProfile;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        for profile in [KeymapProfile::Standard, KeymapProfile::Web] {
            for saved_visible in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let mut config = crate::config::AppConfig::default();
                config.keymap.profile = Some(profile);
                config.layout.terminal_visible = Some(saved_visible);
                let mut app = App::new(dir.path(), config).unwrap();
                app.init_s3_mode(crate::s3::S3Config {
                    path: crate::s3::S3Path::parse("s3://review-fixture/prefix").unwrap(),
                    profile: None,
                });
                let (tx, _rx) = crate::event::event_channel(Default::default());
                app.event_tx = Some(tx.clone());
                let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
                terminal.draw(|frame| render(&mut app, frame)).unwrap();
                let context = CommandContext::capture(&app);
                let reason = "Terminal unavailable in S3 mode";
                assert_eq!(
                    unavailable_reason(&app, &context, CommandId::ToggleTerminal),
                    Some(reason)
                );
                app.open_command_menu();
                app.command_menu
                    .as_mut()
                    .unwrap()
                    .input(CommandId::ToggleTerminal.as_str());
                terminal.draw(|frame| render(&mut app, frame)).unwrap();
                let rendered: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(
                    rendered.contains(reason),
                    "menu must expose shared S3 reason"
                );
                for id in [
                    CommandId::ToggleTerminal,
                    CommandId::MaximizeTerminal,
                    CommandId::GrowTerminal,
                    CommandId::ShrinkTerminal,
                    CommandId::FocusTerminal,
                ] {
                    assert_eq!(
                        unavailable_reason(&app, &context, id),
                        Some(reason),
                        "{id:?}"
                    );
                    assert_eq!(dispatch_command(&mut app, id), Err(reason.into()));
                    assert_eq!(app.workspace.focus.overlay, AppMode::CommandMenu);
                    assert_eq!(
                        app.command_menu.as_ref().unwrap().feedback.as_deref(),
                        Some(reason)
                    );
                }
                app.dismiss_command_menu();
                for suffix in ['t', 'z', 'k', 'j'] {
                    crate::handler::handle_key_event(
                        &mut app,
                        KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT),
                        &tx,
                    );
                    crate::handler::handle_key_event(
                        &mut app,
                        KeyEvent::new(KeyCode::Char(suffix), KeyModifiers::NONE),
                        &tx,
                    );
                    assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
                    assert_eq!(app.workspace.layout.terminal_visible(), saved_visible);
                    assert_eq!(app.workspace.layout.maximized(), None);
                    assert!(app.terminal_state.pty.is_none());
                }
                assert!(!app.open_terminal(&tx));
                assert_eq!(app.workspace.layout.terminal_visible(), saved_visible);
                assert!(app.terminal_state.pty.is_none());
            }
        }
    }
    #[test]
    fn review_fix_terminal_maximize_preserves_editor_usable_viewport() {
        use crate::commands::{dispatch_command, CommandId};
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned.txt");
        let text = (0..200)
            .map(|_| "x".repeat(200))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, &text).unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.open_document_path(&path, true);
        let id = app.workspace.documents.active_id();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(&mut app, frame)).unwrap();
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .set_cursor_position(50, 90);
        terminal.draw(|frame| render(&mut app, frame)).unwrap();
        let area = app.preview_content_area;
        let (tx, _rx) = crate::event::event_channel(Default::default());
        for _ in 0..25 {
            crate::handler::handle_mouse_event(
                &mut app,
                MouseEvent {
                    kind: MouseEventKind::ScrollDown,
                    column: area.x + 5,
                    row: area.y + 2,
                    modifiers: KeyModifiers::NONE,
                },
                &tx,
            );
        }
        terminal.draw(|frame| render(&mut app, frame)).unwrap();
        let viewport = |app: &App| {
            let editor = app.editor().unwrap();
            (
                editor.scroll_offset,
                editor.horizontal_offset,
                editor.visible_width,
                editor.visible_height,
            )
        };
        assert_eq!(viewport(&app), (109, 43, 48, 19));
        for _ in 0..2 {
            dispatch_command(&mut app, CommandId::MaximizeTerminal).unwrap();
            terminal.draw(|frame| render(&mut app, frame)).unwrap();
            assert_eq!(app.preview_content_area.width, 0);
            assert_eq!(
                viewport(&app),
                (109, 43, 48, 19),
                "suppression must freeze the last usable editor viewport"
            );
            dispatch_command(&mut app, CommandId::RestoreLayout).unwrap();
            terminal.draw(|frame| render(&mut app, frame)).unwrap();
            assert_eq!(
                viewport(&app),
                (109, 43, 48, 19),
                "same-sized restore must keep intentional mouse scroll"
            );
        }
        assert_eq!(app.workspace.documents.active_id(), id);
        assert_eq!(app.workspace.documents.active().unwrap().text(), text);
        assert!(app.terminal_state.pty.is_none());
        // A real usable resize still updates dimensions and follows the cursor.
        let mut resized = Terminal::new(TestBackend::new(120, 40)).unwrap();
        resized.draw(|frame| render(&mut app, frame)).unwrap();
        assert_eq!(viewport(&app), (48, 43, 88, 35));
        let area = app.preview_content_area;
        let gutter = app.editor().unwrap().gutter_width();
        crate::handler::handle_mouse_event(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: area.x + gutter + 2,
                row: area.y + 1,
                modifiers: KeyModifiers::NONE,
            },
            &tx,
        );
        assert_eq!(
            (
                app.editor().unwrap().cursor_line,
                app.editor().unwrap().cursor_col
            ),
            (49, 45)
        );
    }
    #[test]
    fn review_fix_disabled_terminal_preserves_document_maximize_and_saved_preferences() {
        use crate::commands::{dispatch_command, CommandId};
        use crate::workspace::layout::MaximizedPane;
        for saved_terminal_visible in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("owned.txt");
            std::fs::write(&path, "owned").unwrap();
            let mut config = crate::config::AppConfig::default();
            config.terminal.enabled = Some(false);
            config.layout.terminal_visible = Some(saved_terminal_visible);
            let mut app = App::new(dir.path(), config).unwrap();
            app.open_document_path(&path, true);
            let id = app.workspace.documents.active_id();
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal.draw(|frame| render(&mut app, frame)).unwrap();
            dispatch_command(&mut app, CommandId::MaximizeEditor).unwrap();
            terminal.draw(|frame| render(&mut app, frame)).unwrap();
            assert_eq!(
                app.workspace.layout.maximized(),
                Some(MaximizedPane::Document)
            );
            assert_eq!(
                app.workspace.layout.terminal_visible(),
                saved_terminal_visible
            );
            assert_eq!(
                app.tree_area.width, 0,
                "disabled-terminal gate must retain document maximization"
            );
            assert_eq!(app.preview_area, ratatui::layout::Rect::new(0, 2, 80, 21));
            assert_eq!(app.terminal_area.width, 0);
            assert_eq!(app.workspace.documents.active_id(), id);
            assert_eq!(app.editor().unwrap().buffer[0], "owned");
            assert!(app.terminal_state.pty.is_none());
            dispatch_command(&mut app, CommandId::RestoreLayout).unwrap();
            terminal.draw(|frame| render(&mut app, frame)).unwrap();
            assert_eq!(app.workspace.layout.maximized(), None);
            assert_eq!(
                app.workspace.layout.terminal_visible(),
                saved_terminal_visible
            );
            assert_eq!(app.tree_area, ratatui::layout::Rect::new(0, 0, 24, 23));
            assert_eq!(app.preview_area, ratatui::layout::Rect::new(25, 2, 55, 21));
            assert!(app.terminal_state.pty.is_none());
        }
    }
    #[test]
    fn adaptive_plain_splitter_is_visible_along_its_entire_hit_strip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "owned").unwrap();
        let mut config = crate::config::AppConfig::default();
        config.tree.use_icons = Some(false);
        let mut app = App::new(dir.path(), config).unwrap();
        app.open_document_path(&path, true);
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .insert_char('X');
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| render(&mut app, f)).unwrap();
        for y in 0..23 {
            assert_eq!(terminal.backend().buffer()[(24, y)].symbol(), "|");
        }
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!text.contains('●') && !text.contains('⏸'));
    }
    #[test]
    fn adaptive_zero_viewport_does_not_edit_an_unrendered_document() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned.txt");
        std::fs::write(&path, "original").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.open_document_path(&path, true);
        let mut terminal = Terminal::new(TestBackend::new(0, 0)).unwrap();
        terminal.draw(|f| render(&mut app, f)).unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        crate::handler::handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE),
            &tx,
        );
        assert_eq!(app.editor().unwrap().buffer[0], "original");
        crate::handler::handle_paste_event(&mut app, "paste");
        assert_eq!(app.editor().unwrap().buffer[0], "original");
    }
    #[test]
    fn adaptive_keyboard_only_runtime_routes_and_disabled_terminal_fallback() {
        use crate::workspace::layout::MaximizedPane;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        for profile in [
            crate::keymap::KeymapProfile::Standard,
            crate::keymap::KeymapProfile::Web,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut config = crate::config::AppConfig::default();
            config.keymap.profile = Some(profile);
            config.general.mouse = Some(false);
            config.layout.terminal_visible = Some(true);
            let mut app = App::new(dir.path(), config).unwrap();
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal.draw(|f| render(&mut app, f)).unwrap();
            let (tx, _rx) = crate::event::event_channel(Default::default());
            let route = |app: &mut App, suffix| {
                crate::handler::handle_key_event(
                    app,
                    KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT),
                    &tx,
                );
                crate::handler::handle_key_event(
                    app,
                    KeyEvent::new(KeyCode::Char(suffix), KeyModifiers::NONE),
                    &tx,
                );
            };
            route(&mut app, 'l');
            assert_eq!(app.workspace.layout.explorer_width(), 26);
            route(&mut app, 'h');
            assert_eq!(app.workspace.layout.explorer_width(), 24);
            route(&mut app, 'k');
            assert_eq!(app.workspace.layout.terminal_height(), 9);
            route(&mut app, 'j');
            assert_eq!(app.workspace.layout.terminal_height(), 7);
            route(&mut app, 'z');
            assert_eq!(
                app.workspace.layout.maximized(),
                Some(MaximizedPane::Terminal)
            );
            route(&mut app, 'r');
            route(&mut app, 'x');
            assert_eq!(
                app.workspace.layout.maximized(),
                Some(MaximizedPane::Document)
            );
            route(&mut app, 'r');
            route(&mut app, 'e');
            assert!(!app.workspace.layout.explorer_visible());
            route(&mut app, 'e');
            assert!(app.workspace.layout.explorer_visible());
            app.workspace.focus.panel = FocusedPanel::Terminal;
            app.config.terminal.enabled = Some(false);
            terminal.draw(|f| render(&mut app, f)).unwrap();
            assert_eq!(app.terminal_area.width, 0);
            assert_ne!(app.workspace.focus.panel, FocusedPanel::Terminal);
            assert!(app.workspace.layout.terminal_visible());
            route(&mut app, 'z');
            assert_eq!(app.workspace.layout.maximized(), None);
            assert!(app.terminal_state.pty.is_none());
            app.config.terminal.enabled = Some(true);
            terminal.draw(|f| render(&mut app, f)).unwrap();
            assert_eq!(app.terminal_area.width, 55);
        }
    }

    #[test]
    fn adaptive_modal_matrix_stays_inside_offset_viewport() {
        use crate::app::DialogKind;
        for mode in [
            AppMode::Dialog(DialogKind::CreateFile),
            AppMode::Dialog(DialogKind::SaveConfirm),
            AppMode::Dialog(DialogKind::Error {
                message: "error".into(),
            }),
            AppMode::Search,
            AppMode::Help,
            AppMode::CopyOverlay,
            AppMode::CommandMenu,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
            if mode == AppMode::CommandMenu {
                app.open_command_menu();
            } else {
                app.set_overlay(mode.clone());
            }
            app.copy_overlay_text = Some("literal\ntext".into());
            app.help_state.active_tab = crate::components::help::HelpTab::Settings;
            app.help_state.settings_state =
                Some(crate::components::settings::SettingsState::from_app(&app));
            for (w, h) in [
                (120, 40),
                (80, 24),
                (60, 20),
                (0, 0),
                (1, 1),
                (3, 4),
                (8, 5),
            ] {
                let area = ratatui::layout::Rect::new(2, 3, w, h);
                let mut terminal = Terminal::new(TestBackend::new(w + 4, h + 6)).unwrap();
                terminal
                    .draw(|f| render_in_area(&mut app, f, area))
                    .unwrap();
                for (index, cell) in terminal.backend().buffer().content.iter().enumerate() {
                    let point = (
                        (index % usize::from(w + 4)) as u16,
                        (index / usize::from(w + 4)) as u16,
                    )
                        .into();
                    if !area.contains(point) {
                        assert_eq!(cell.symbol(), " ", "{mode:?} {w}x{h}");
                    }
                }
            }
        }
    }
    #[test]
    fn adaptive_focus_routes_cannot_change_suppressed_document_presentation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned.txt");
        std::fs::write(&path, "owned").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.open_document_path(&path, true);
        app.show_selected_preview();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| render(&mut app, f)).unwrap();
        crate::commands::dispatch_command(&mut app, crate::commands::CommandId::MaximizeTerminal)
            .unwrap();
        let id = app.workspace.documents.active_id();
        app.focus_right();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Terminal);
        assert_eq!(
            app.right_panel_presentation,
            crate::app::RightPanelPresentation::SelectedPreview
        );
        assert_eq!(app.workspace.documents.active_id(), id);
        assert!(app.terminal_state.pty.is_none());
    }

    #[test]
    fn adaptive_compact_overlay_return_repairs_focus_and_keeps_origin_identity() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "alpha").unwrap();
        std::fs::write(&b, "beta").unwrap();
        let mut app = App::new(
            dir.path(),
            toml::from_str("[layout]\nterminal_visible=true").unwrap(),
        )
        .unwrap();
        app.open_document_path(&a, true);
        let id = app.workspace.documents.active_id();
        app.workspace.focus.panel = FocusedPanel::Terminal;
        app.set_overlay(AppMode::Dialog(crate::app::DialogKind::SaveConfirm));
        app.workspace
            .documents
            .open(&b, crate::workspace::documents::OpenDisposition::Pinned)
            .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
        terminal.draw(|f| render(&mut app, f)).unwrap();
        app.close_dialog();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert_eq!(app.workspace.documents.active_id(), id);
        assert!(
            app.workspace.layout.terminal_visible(),
            "compact suppression must not change preference"
        );
        assert!(app.terminal_state.pty.is_none());
    }
    #[test]
    fn adaptive_offset_full_app_matrix_preserves_preferences_and_bounds_chrome() {
        use crate::workspace::layout::MaximizedPane;
        for scheme in ["dark", "light"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("owned.txt");
            std::fs::write(&path, "abcdef\nsecond").unwrap();
            let mut config = crate::config::AppConfig::default();
            config.theme.scheme = Some(scheme.into());
            config.tree.use_icons = Some(false);
            config.layout.terminal_visible = Some(true);
            let mut app = App::new(dir.path(), config).unwrap();
            app.open_document_path(&path, true);
            let id = app.workspace.documents.active_id();
            app.workspace
                .documents
                .active_mut()
                .unwrap()
                .editor
                .insert_char('X');
            for mode in [
                None,
                Some(MaximizedPane::Document),
                Some(MaximizedPane::Terminal),
            ] {
                app.workspace.layout.restore();
                if let Some(mode) = mode {
                    app.workspace.layout.maximize(mode);
                }
                let preferences = app.workspace.layout.clone();
                for (w, h) in [
                    (120, 40),
                    (80, 24),
                    (60, 20),
                    (30, 8),
                    (0, 0),
                    (1, 1),
                    (3, 4),
                ] {
                    let area = ratatui::layout::Rect::new(2, 3, w, h);
                    let mut terminal = Terminal::new(TestBackend::new(w + 4, h + 6)).unwrap();
                    terminal
                        .draw(|f| render_in_area(&mut app, f, area))
                        .unwrap();
                    assert_eq!(app.workspace_area, Some(area));
                    assert!(app.workspace_rects.all_inside(area));
                    assert_eq!(app.workspace.layout, preferences);
                    assert_eq!(app.workspace.documents.active_id(), id);
                    assert_eq!(app.editor().unwrap().buffer[0], "Xabcdef");
                    assert!(app.terminal_state.pty.is_none());
                    for (index, cell) in terminal.backend().buffer().content.iter().enumerate() {
                        let x = (index % usize::from(w + 4)) as u16;
                        let y = (index / usize::from(w + 4)) as u16;
                        if !area.contains((x, y).into()) {
                            assert_eq!(cell.symbol(), " ");
                        }
                    }
                    for hit in &app.document_tabs.hits {
                        assert!(area.contains((hit.area.x, hit.area.y).into()));
                    }
                    for hit in &app.breadcrumbs.hits {
                        assert!(area.contains((hit.area.x, hit.area.y).into()));
                    }
                }
            }
        }
    }
    #[test]
    fn adaptive_editor_border_is_not_a_content_hit() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned.txt");
        std::fs::write(&path, "abcdef").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.open_document_path(&path, true);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| render(&mut app, f)).unwrap();
        let area = app.preview_area;
        let (tx, _rx) = crate::event::event_channel(Default::default());
        crate::handler::handle_mouse_event(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: area.right() - 1,
                row: area.y + 1,
                modifiers: KeyModifiers::NONE,
            },
            &tx,
        );
        assert_eq!(app.editor().unwrap().cursor_col, 0);
        assert!(app.editor().unwrap().selection.is_none());
    }
    #[test]
    fn adaptive_splitter_drag_owns_gesture_and_breadcrumb_reveal_keeps_dirty_document() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned.txt");
        std::fs::write(&path, "abc").unwrap();
        let mut app = App::new(
            dir.path(),
            toml::from_str("[layout]\nterminal_visible = true").unwrap(),
        )
        .unwrap();
        app.tree_state.reload_dir(dir.path());
        app.open_document_path(&path, true);
        let id = app.workspace.documents.active_id();
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .insert_char('X');
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| render(&mut app, f)).unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        for (kind, column, row) in [
            (MouseEventKind::Down(MouseButton::Left), 24, 5),
            (MouseEventKind::Drag(MouseButton::Left), 34, 23),
            (MouseEventKind::Up(MouseButton::Left), 34, 23),
        ] {
            crate::handler::handle_mouse_event(
                &mut app,
                MouseEvent {
                    kind,
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                },
                &tx,
            );
        }
        assert_eq!(app.workspace.layout.explorer_width(), 34);
        assert!(app.splitter_drag.is_none());
        assert!(app.editor().unwrap().selection.is_none());
        assert!(!app.terminal_state.selection.is_active());
        terminal.draw(|f| render(&mut app, f)).unwrap();
        let split = app.workspace_rects.terminal_split;
        for (kind, column, row) in [
            (MouseEventKind::Down(MouseButton::Left), split.x, split.y),
            (MouseEventKind::Drag(MouseButton::Left), 0, split.y - 2),
            (MouseEventKind::Up(MouseButton::Left), 0, split.y - 2),
        ] {
            crate::handler::handle_mouse_event(
                &mut app,
                MouseEvent {
                    kind,
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                },
                &tx,
            );
        }
        assert_eq!(app.workspace.layout.terminal_height(), 9);
        terminal.draw(|f| render(&mut app, f)).unwrap();
        let hit = app.breadcrumbs.hits.last().unwrap().area;
        crate::handler::handle_mouse_event(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: hit.x,
                row: hit.y,
                modifiers: KeyModifiers::NONE,
            },
            &tx,
        );
        assert_eq!(
            app.tree_state.flat_items[app.tree_state.selected_index].path,
            path
        );
        assert_eq!(app.workspace.documents.active_id(), id);
        assert_eq!(app.editor().unwrap().buffer[0], "Xabc");
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn adaptive_explicit_start_visible_placeholder_keeps_shell_on_layout_changes() {
        use crate::commands::{dispatch_command, CommandId};
        struct Cleanup(App);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                self.0.shutdown_terminal();
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let config = toml::from_str(
            "[terminal]\ndefault_shell = '/bin/sh'\n[layout]\nterminal_visible = true",
        )
        .unwrap();
        let mut guard = Cleanup(App::new(dir.path(), config).unwrap());
        let app = &mut guard.0;
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        } // Pre-integration placeholder mirrors the old authority.
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| render(app, f)).unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        assert!(
            app.toggle_terminal(&tx),
            "explicit start must start a missing shell even when shown"
        );
        assert!(app.terminal_state.pty.as_ref().unwrap().is_alive());
        async fn geometry(app: &App, root: &std::path::Path, sequence: usize) -> (u32, u16, u16) {
            app.terminal_state
                .pty
                .as_ref()
                .unwrap()
                .write(
                    format!("printf '{sequence} %s ' $$ > geometry; stty size >> geometry\n")
                        .as_bytes(),
                )
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if let Ok(text) = std::fs::read_to_string(root.join("geometry")) {
                    let fields: Vec<_> = text.split_whitespace().collect();
                    if fields.len() == 4 && fields[0] == sequence.to_string() {
                        return (
                            fields[1].parse().unwrap(),
                            fields[2].parse().unwrap(),
                            fields[3].parse().unwrap(),
                        );
                    }
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "fresh shell geometry unavailable"
                );
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
        let baseline = geometry(app, dir.path(), 0).await;
        assert_eq!((baseline.1, baseline.2), (6, 55));
        for (sequence, w, h, rows, cols) in
            [(1, 120, 40, 6, 95), (2, 60, 20, 6, 35), (3, 80, 24, 6, 55)]
        {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|f| render(app, f)).unwrap();
            assert_eq!(
                geometry(app, dir.path(), sequence).await,
                (baseline.0, rows, cols)
            );
            assert_eq!(
                (
                    app.terminal_state.emulator.visible_rows(),
                    app.terminal_state.emulator.visible_cols()
                ),
                (usize::from(rows), usize::from(cols))
            );
        }
        dispatch_command(app, CommandId::MaximizeTerminal).unwrap();
        assert_eq!(geometry(app, dir.path(), 4).await, (baseline.0, 22, 80));
        dispatch_command(app, CommandId::RestoreLayout).unwrap();
        assert_eq!(geometry(app, dir.path(), 5).await, baseline);
        assert!(!app.toggle_terminal(&tx));
        assert_eq!(
            app.workspace.focus.panel,
            FocusedPanel::Tree,
            "explicit hide retains the established explorer keyboard return route"
        );
        assert!(app.terminal_state.pty.as_ref().unwrap().is_alive());
        for (w, h) in [(0, 0), (1, 1), (30, 8)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|f| render(app, f)).unwrap();
            assert_eq!(
                geometry(app, dir.path(), usize::from(w) + 6).await,
                baseline
            );
        }
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| render(app, f)).unwrap();
        assert!(app.toggle_terminal(&tx));
        assert_eq!(geometry(app, dir.path(), 40).await, baseline);
    }
    #[test]
    fn adaptive_status_describes_presented_owned_buffer_not_tree_selection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned.rs");
        std::fs::write(&path, "\t中x\r\nsecond\r\n").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.open_document_path(&path, true);
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .set_cursor_position(0, 4);
        let mut terminal = Terminal::new(TestBackend::new(160, 24)).unwrap();
        terminal.draw(|f| render(&mut app, f)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("Ln 1 Col 7 CRLF Rust UTF-8"), "{text}");
        assert!(app.terminal_state.pty.is_none());
    }
    #[test]
    fn adaptive_runtime_uses_saved_absolute_sizes_and_hidden_terminal_startup() {
        let dir = tempfile::tempdir().unwrap();
        let config = toml::from_str(
            "[layout]\nexplorer_width = 30\nterminal_height = 8\nterminal_visible = true",
        )
        .unwrap();
        let mut app = App::new(dir.path(), config).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| render(&mut app, f)).unwrap();
        assert_eq!(app.tree_area, ratatui::layout::Rect::new(0, 0, 30, 23));
        assert_eq!(app.terminal_area, ratatui::layout::Rect::new(31, 16, 49, 7));
        assert!(
            app.terminal_state.pty.is_none(),
            "preferences must not start a shell"
        );
        assert_eq!(app.terminal_state.emulator.visible_rows(), 7);
        assert_eq!(app.terminal_state.emulator.visible_cols(), 49);
    }

    #[test]
    fn adaptive_runtime_pane_commands_preserve_owned_text_and_repair_hidden_focus() {
        use crate::commands::{dispatch_command, CommandId};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned.rs");
        std::fs::write(&path, "\t中x\nsecond").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.open_document_path(&path, true);
        let id = app.workspace.documents.active_id();
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .insert_char('X');
        let text = app.editor().unwrap().buffer.clone();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| render(&mut app, f)).unwrap();
        dispatch_command(&mut app, CommandId::FocusTree).unwrap();
        dispatch_command(&mut app, CommandId::ToggleExplorer).unwrap();
        terminal.draw(|f| render(&mut app, f)).unwrap();
        assert_eq!(app.tree_area.width, 0);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        dispatch_command(&mut app, CommandId::MaximizeEditor).unwrap();
        terminal.draw(|f| render(&mut app, f)).unwrap();
        assert_eq!(app.preview_area.right(), 80);
        dispatch_command(&mut app, CommandId::RestoreLayout).unwrap();
        assert_eq!(app.workspace.documents.active_id(), id);
        assert_eq!(app.editor().unwrap().buffer, text);
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
    use ratatui::{backend::TestBackend, Terminal};

    fn round1_unsupported_single_click_fixture(kind: &str) {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        for dirty in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let a = dir.path().join("owned.txt");
            std::fs::write(&a, "OWNED_A_CONTENT").unwrap();
            let (name, bytes, marker) = match kind {
                "binary" => ("selected.bin", b"\0binary".to_vec(), "selected.bin [PREVIEW]"),
                "notebook" => ("selected.ipynb", br#"{"cells":[{"cell_type":"code","source":["LEGACY_NOTEBOOK_SELECTION"],"outputs":[]}],"metadata":{},"nbformat":4,"nbformat_minor":0}"#.to_vec(), "LEGACY_NOTEBOOK_SELECTION"),
                "readonly" => ("readonly.txt", b"READONLY_SELECTION".to_vec(), "readonly.txt [PREVIEW]"),
                "large" => ("large.txt", "LARGE_SELECTION\n".repeat(30).into_bytes(), "large.txt [PREVIEW]"),
                _ => unreachable!(),
            };
            let selected = dir.path().join(name);
            std::fs::write(&selected, bytes).unwrap();
            if kind == "readonly" {
                let mut permissions = std::fs::metadata(&selected).unwrap().permissions();
                permissions.set_readonly(true);
                std::fs::set_permissions(&selected, permissions).unwrap();
            }
            let mut config = crate::config::AppConfig::default();
            config.preview.max_full_preview_bytes = Some(128);
            config.general.max_editor_bytes = Some(128);
            let mut app = App::new(dir.path(), config).unwrap();
            app.tree_state.reload_dir(dir.path());
            app.open_document_path(&a, true);
            let owner = app.workspace.documents.active_id().unwrap();
            if dirty {
                app.workspace
                    .documents
                    .active_mut()
                    .unwrap()
                    .editor
                    .insert_char('X');
            }
            let text = app.workspace.documents.get(owner).unwrap().text();
            let cursor = (
                app.editor().unwrap().cursor_line,
                app.editor().unwrap().cursor_col,
            );
            app.navigate_to_path(&selected);
            let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
            terminal.draw(|f| render(&mut app, f)).unwrap();
            let index = app
                .tree_state
                .flat_items
                .iter()
                .position(|i| i.path == selected)
                .unwrap();
            let (tx, _rx) = crate::event::event_channel(Default::default());
            let click = MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: app.tree_area.x + 3,
                row: app.tree_area.y + 1 + (index - app.tree_state.scroll_offset) as u16,
                modifiers: KeyModifiers::NONE,
            };
            crate::handler::handle_mouse_event(&mut app, click, &tx);
            terminal.draw(|f| render(&mut app, f)).unwrap();
            let screen: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(screen.contains(marker), "{kind}, dirty={dirty}: {screen}");
            assert!(!screen.contains("OWNED_A_CONTENT"));
            assert!(!screen.contains("owned.txt [EDIT]"));
            assert_eq!(app.preview_state.current_path, Some(selected.clone()));
            assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
            assert_eq!(app.workspace.documents.active_id(), Some(owner));
            assert_eq!(app.workspace.documents.len(), 1);
            assert_eq!(app.workspace.documents.get(owner).unwrap().text(), text);
            assert_eq!(app.editor().unwrap().modified, dirty);
            assert!(!app.editor_visible());
            crate::handler::handle_key_event(
                &mut app,
                crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char(' '),
                    KeyModifiers::NONE,
                ),
                &tx,
            );
            assert!(app.tree_state.multi_selected.contains(&index));
            assert_eq!(app.workspace.documents.get(owner).unwrap().text(), text);
            // Mouse input must use the actually rendered preview, not retained A's code map.
            let preview_area = app.preview_area;
            crate::handler::handle_mouse_event(
                &mut app,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: preview_area.x + 3,
                    row: preview_area.y + 2,
                    modifiers: KeyModifiers::NONE,
                },
                &tx,
            );
            assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
            assert_eq!(
                (
                    app.editor().unwrap().cursor_line,
                    app.editor().unwrap().cursor_col
                ),
                cursor
            );
            app.activate_document(owner);
            terminal.draw(|f| render(&mut app, f)).unwrap();
            let screen: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(screen.contains("OWNED_A_CONTENT"));
            assert!(app.editor_visible());
            assert_eq!(app.workspace.documents.get(owner).unwrap().text(), text);
            if dirty {
                app.workspace.documents.active_mut().unwrap().editor.undo();
                assert_eq!(app.editor().unwrap().buffer[0], "OWNED_A_CONTENT");
                app.workspace.documents.active_mut().unwrap().editor.redo();
                assert_eq!(app.workspace.documents.get(owner).unwrap().text(), text);
            }
            app.tree_last_click = None;
            crate::handler::handle_mouse_event(&mut app, click, &tx);
            assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
            assert!(!app.editor_visible());
            app.focus_right();
            assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
            assert!(app.editor_visible());
            app.focus_left();
            assert!(app.editor_visible());
            // Restore permissions for portable temporary-directory cleanup.
            if kind == "readonly" {
                std::fs::set_permissions(&selected, std::fs::metadata(&a).unwrap().permissions())
                    .unwrap();
            }
        }
    }
    #[test]
    fn task3_no_preview_status_and_retained_editor_are_bounded_and_independent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("中😀.txt");
        std::fs::write(&path, "alpha β 中 😀 long text").unwrap();
        let mut config = crate::config::AppConfig::default();
        config.preview.enabled = Some(false);
        let mut app = App::new(dir.path(), config).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(&mut app, frame)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Automatic preview disabled"));
        assert!(app.preview_state.current_path.is_none());
        assert!(app.open_document_path(&path, true));
        let id = app.workspace.documents.active_id();
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .toggle_wrap();
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .insert_text("dirty")
            .unwrap();
        for (w, h) in [(0, 0), (1, 1), (2, 3), (60, 20), (80, 24), (120, 40)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|frame| render(&mut app, frame)).unwrap();
            assert_eq!(app.workspace.documents.active_id(), id);
            assert!(app.workspace.documents.active().unwrap().editor.modified);
            assert!(app.workspace.documents.active().unwrap().editor.line_wrap);
            assert!(app.preview_state.current_path.is_none());
            assert!(app.command_entry_area.right() <= w && app.command_entry_area.bottom() <= h);
            if w >= 60 {
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(text.contains("[EDIT]"));
            }
        }
    }

    #[test]
    fn round1_binary_single_click_presentation() {
        round1_unsupported_single_click_fixture("binary");
    }
    #[test]
    fn round1_notebook_single_click_presentation() {
        round1_unsupported_single_click_fixture("notebook");
    }
    #[test]
    fn round1_readonly_single_click_presentation() {
        round1_unsupported_single_click_fixture("readonly");
    }
    #[test]
    fn round1_large_single_click_presentation() {
        round1_unsupported_single_click_fixture("large");
    }

    #[test]
    fn task3_tab_mouse_restores_exact_history_and_tiny_layouts_are_safe() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "alpha").unwrap();
        std::fs::write(&b, "beta").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.open_document_path(&a, true);
        let first = app.workspace.documents.active_id().unwrap();
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .insert_char('X');
        app.open_document_path(&b, true);
        let mut terminal = Terminal::new(TestBackend::new(140, 20)).unwrap();
        terminal.draw(|f| render(&mut app, f)).unwrap();
        let hit = app
            .document_tabs
            .hits
            .iter()
            .find(|h| h.id == first)
            .unwrap()
            .area;
        let (tx, _rx) = crate::event::event_channel(Default::default());
        crate::handler::handle_mouse_event(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: hit.x,
                row: hit.y,
                modifiers: KeyModifiers::NONE,
            },
            &tx,
        );
        assert_eq!(app.workspace.documents.active_id(), Some(first));
        assert_eq!(app.editor().unwrap().buffer[0], "Xalpha");
        app.workspace.documents.active_mut().unwrap().editor.undo();
        assert_eq!(app.editor().unwrap().buffer[0], "alpha");
        app.open_document_list();
        terminal.draw(|f| render(&mut app, f)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(screen.contains("Open documents"));
        assert!(screen.contains("Alt+B/N tabs"));
        for (w, h) in [(0, 0), (1, 1), (2, 2), (4, 3), (8, 5)] {
            let mut tiny = Terminal::new(TestBackend::new(w, h)).unwrap();
            tiny.draw(|f| render(&mut app, f)).unwrap();
            assert!(app.preview_area.bottom() <= h);
            assert!(app.document_tabs.area.right() <= w);
        }
    }

    #[test]
    fn task3_tab_chrome_mouse_code_rows_and_owned_title() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned.txt");
        std::fs::write(&path, "abc\ndef").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.open_document_path(&path, true);
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|f| render(&mut app, f)).unwrap();
        assert_eq!(app.document_tabs.area.height, 1);
        assert_eq!(app.preview_area.y, app.document_tabs.area.bottom());
        let area = app.preview_area;
        let gutter = app.editor().unwrap().gutter_width();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        crate::handler::handle_mouse_event(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: area.x + 1 + gutter + 1,
                row: area.y + 2,
                modifiers: KeyModifiers::NONE,
            },
            &tx,
        );
        assert_eq!(app.editor().unwrap().cursor_line, 1);
        assert_eq!(app.editor().unwrap().cursor_col, 1);
        app.preview_state.current_path = Some(dir.path().join("unrelated.txt"));
        terminal.draw(|f| render(&mut app, f)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(screen.contains("owned.txt [EDIT]"));
    }

    #[test]
    fn round1_editor_focus_hint_matches_directional_dispatch_not_local_tab() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "alpha").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.workspace
            .documents
            .open(&path, crate::workspace::documents::OpenDisposition::Pinned)
            .unwrap();
        app.workspace.focus.panel = FocusedPanel::Editor;
        let mut terminal = Terminal::new(TestBackend::new(160, 24)).unwrap();
        terminal.draw(|frame| render(&mut app, frame)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains(&format!(
            "{}:save",
            app.keymap
                .binding_labels(
                    crate::commands::CommandId::Save,
                    crate::keymap::FocusContext::Editor
                )
                .join("/")
        )));
        assert!(!screen.contains("Tab:focus"));
        let (tx, _rx) = crate::event::event_channel(Default::default());
        crate::handler::handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &tx,
        );
        assert!(app.editor().unwrap().modified);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        let text = app.editor().unwrap().buffer.clone();
        crate::handler::handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL),
            &tx,
        );
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
        assert_eq!(app.editor().unwrap().buffer, text);
        app.focus_right();
        let editor = &mut app.workspace.documents.active_mut().unwrap().editor;
        editor.open_find();
        editor.find_state.replace_mode = true;
        crate::handler::handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &tx,
        );
        assert!(app.editor().unwrap().find_state.in_replace_field);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert_eq!(app.editor().unwrap().buffer, text);
    }

    #[test]
    fn editor_viewport_excludes_shared_gutter_and_active_find_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.workspace.focus.overlay = AppMode::Normal;
        app.workspace.focus.panel = FocusedPanel::Editor;
        install_editor(
            &mut app,
            crate::editor::EditorState::new(&"x".repeat(200), "test.txt".into()),
        );
        app.workspace
            .documents
            .active_mut()
            .map(|d| &mut d.editor)
            .unwrap()
            .move_end();
        for (w, h) in [(60, 15), (20, 8), (8, 3), (1, 1)] {
            app.workspace
                .documents
                .active_mut()
                .map(|d| &mut d.editor)
                .unwrap()
                .open_find_replace();
            let editor = app.editor().unwrap();
            let previous_usable_viewport = (editor.visible_width, editor.visible_height);
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|frame| render(&mut app, frame)).unwrap();
            let editor = app.workspace.documents.active().map(|d| &d.editor).unwrap();
            let inner_height = app.preview_area.height.saturating_sub(2) as usize;
            let height = inner_height.saturating_sub(editor.find_bar_height(inner_height));
            let width = app
                .preview_area
                .width
                .saturating_sub(2)
                .saturating_sub(editor.gutter_width()) as usize;
            let expected = if width > 0 && height > 0 {
                (width, height)
            } else {
                previous_usable_viewport
            };
            assert_eq!((editor.visible_width, editor.visible_height), expected);
            if editor.visible_width > 0 {
                assert!(200 - editor.horizontal_offset < editor.visible_width);
            }
        }
    }

    #[test]
    fn redraw_keeps_intentional_preview_scroll_without_cursor_tracking() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "preview").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.update_preview();
        app.preview_state.content_lines = (0..100)
            .map(|n| ratatui::text::Line::from(format!("line{n}")))
            .collect();
        app.preview_state.scroll_offset = 50;
        let mut terminal = Terminal::new(TestBackend::new(60, 15)).unwrap();
        terminal.draw(|frame| render(&mut app, frame)).unwrap();
        assert_eq!(app.preview_state.scroll_offset, 50);
        terminal.draw(|frame| render(&mut app, frame)).unwrap();
        assert_eq!(app.preview_state.scroll_offset, 50);
    }

    #[test]
    fn clipboard_browser_overlay_multiline_and_tiny_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.workspace.focus.overlay = AppMode::CopyOverlay;
        app.copy_overlay_text = Some("first\nsecond\n".into());
        let mut terminal = Terminal::new(TestBackend::new(60, 15)).unwrap();
        terminal.draw(|frame| render(&mut app, frame)).unwrap();
        let rows: Vec<String> = terminal
            .backend()
            .buffer()
            .content
            .chunks(60)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect();
        assert!(rows
            .iter()
            .any(|row| row.contains("first") && !row.contains("second")));
        assert!(rows
            .iter()
            .any(|row| row.contains("second") && !row.contains("first")));
        for (w, h) in [(1, 1), (8, 3), (16, 5)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|frame| render(&mut app, frame)).unwrap();
            assert_eq!(app.copy_overlay_text.as_deref(), Some("first\nsecond\n"));
            if (w, h) == (16, 5) {
                // Both lower corners are on screen, not merely clipped away.
                assert_eq!(terminal.backend().buffer()[(2, 3)].symbol(), "└");
                assert_eq!(terminal.backend().buffer()[(13, 3)].symbol(), "┘");
            }
        }
        app.copy_overlay_text = Some(format!("{}\nlast\n", "x".repeat(100)));
        app.copy_overlay_scroll = (1, 0);
        let mut terminal = Terminal::new(TestBackend::new(20, 7)).unwrap();
        terminal.draw(|frame| render(&mut app, frame)).unwrap();
        let row: String = terminal.backend().buffer().content[40..60]
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(row.contains("last"));
        assert_eq!(
            app.copy_overlay_text.as_deref(),
            Some(format!("{}\nlast\n", "x".repeat(100)).as_str())
        );
    }
    #[test]
    fn editor_title_uses_owned_path_not_transient_preview() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned-a.txt");
        std::fs::write(&path, "content").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.workspace
            .documents
            .open(&path, crate::workspace::documents::OpenDisposition::Pinned)
            .unwrap();
        app.workspace.focus.overlay = AppMode::Normal;
        app.preview_state.current_path = Some(dir.path().join("transient-b.txt"));
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        for panel in [
            FocusedPanel::Editor,
            FocusedPanel::Tree,
            FocusedPanel::Terminal,
        ] {
            if panel == FocusedPanel::Terminal && !app.workspace.layout.terminal_visible() {
                app.workspace.layout.toggle_terminal();
            }
            app.workspace.focus.panel = panel;
            terminal.draw(|frame| render(&mut app, frame)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(text.contains("owned-a.txt"));
            assert!(!text.contains("transient-b.txt"));
            assert_eq!(app.workspace.focus.panel, panel);
        }
        app.workspace.focus.panel = FocusedPanel::Preview;
        assert!(!app.editor_visible());
        assert!(app.editor().is_some());
    }

    /// Text of one rendered rectangle, row-major.
    fn area_text(buf: &ratatui::buffer::Buffer, area: ratatui::layout::Rect) -> String {
        let clipped = area.intersection(buf.area);
        let mut text = String::new();
        for y in clipped.y..clipped.bottom() {
            for x in clipped.x..clipped.right() {
                text.push_str(buf.cell((x, y)).map_or(" ", |cell| cell.symbol()));
            }
            text.push('\n');
        }
        text
    }

    fn git_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .env("LC_ALL", "C")
                .env("GIT_TERMINAL_PROMPT", "0")
                .output()
                .expect("git must be installed for fixtures")
        };
        assert!(git(&["-c", "init.defaultBranch=main", "init", "-q"])
            .status
            .success());
        std::fs::write(dir.path().join("tracked.txt"), "one\n").unwrap();
        assert!(git(&["add", "-A"]).status.success());
        assert!(git(&[
            "-c",
            "user.email=fixture@example.com",
            "-c",
            "user.name=fixture",
            "commit",
            "-q",
            "-m",
            "base",
        ])
        .status
        .success());
        dir
    }

    fn install_git_snapshot(app: &mut crate::app::App, worktree: &std::path::Path) {
        use crate::git::{GitLimits, GitResult};
        use std::sync::atomic::AtomicBool;
        let snapshot = match crate::git::status_bounded(
            worktree,
            GitLimits::default(),
            &AtomicBool::new(false),
        ) {
            GitResult::Snapshot(snapshot) => snapshot,
            other => panic!("expected a snapshot, got {other:?}"),
        };
        let generation = app.git.begin(worktree.to_path_buf());
        app.accept_git_refresh(crate::git::GitRefresh {
            generation,
            root: worktree.to_path_buf(),
            result: GitResult::Snapshot(snapshot),
        });
    }

    #[test]
    fn git_indicators_render_modified_marker_and_branch_over_a_repository_fixture() {
        use super::{render, App};
        use ratatui::{backend::TestBackend, Terminal};
        let dir = git_fixture();
        std::fs::write(dir.path().join("tracked.txt"), "two\n").unwrap();

        let mut config = crate::config::AppConfig::default();
        config.tree.use_icons = Some(false);
        let mut app = App::new(dir.path(), config).unwrap();
        app.tree_state.reload_dir(dir.path());
        install_git_snapshot(&mut app, dir.path());

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(&mut app, frame)).unwrap();
        let buf = terminal.backend().buffer();
        let tree_screen = area_text(buf, app.tree_area);
        let status_screen = area_text(buf, app.workspace_rects.status);
        assert!(
            app.git_snapshot().is_some(),
            "snapshot unexpectedly absent: root={:?} git_root={:?}",
            app.tree_state.root.path,
            app.git.root()
        );
        assert!(
            tree_screen
                .lines()
                .any(|line| line.contains("tracked.txt") && line.contains(" M")),
            "tree marker missing on tracked.txt row: {tree_screen}"
        );
        assert!(
            status_screen.contains("main"),
            "status branch missing: {status_screen}"
        );
    }

    #[test]
    fn git_indicators_are_absent_for_disabled_and_non_repository_roots() {
        use super::{render, App};
        use ratatui::{backend::TestBackend, Terminal};
        let dir = git_fixture();
        std::fs::write(dir.path().join("tracked.txt"), "two\n").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.tree_state.reload_dir(dir.path());
        install_git_snapshot(&mut app, dir.path());

        // Enabled: decorations render on the changed file's own row.
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(&mut app, frame)).unwrap();
        let buf = terminal.backend().buffer();
        let tree_screen = area_text(buf, app.tree_area);
        assert!(
            tree_screen
                .lines()
                .any(|line| line.contains("tracked.txt") && line.contains(" M")),
            "tree marker missing on tracked.txt row: {tree_screen}"
        );
        assert!(area_text(buf, app.workspace_rects.status).contains("main"));

        // Live disable (as `apply_settings_candidate` would write `app.config`):
        // no stale decorations survive and the retained snapshot is not rendered.
        app.config.git.enabled = Some(false);
        terminal.draw(|frame| render(&mut app, frame)).unwrap();
        let buf = terminal.backend().buffer();
        let tree_screen = area_text(buf, app.tree_area);
        assert!(
            !tree_screen
                .lines()
                .any(|line| line.contains("tracked.txt") && line.contains(" M")),
            "stale tree marker survived disable: {tree_screen}"
        );
        assert!(!area_text(buf, app.workspace_rects.status).contains("main"));
        assert!(app.git_snapshot().is_none());

        // Non-repository root: no work-tree, so no indicators at all.
        let plain = tempfile::tempdir().unwrap();
        std::fs::write(plain.path().join("plain.txt"), "x").unwrap();
        let mut app = App::new(plain.path(), crate::config::AppConfig::default()).unwrap();
        app.tree_state.reload_dir(plain.path());
        assert!(app.git_snapshot().is_none());
        assert!(app.git_render().is_none());
    }
}
