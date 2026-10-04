//! Stable workspace commands. Availability inspects owned in-memory state only.
use crate::app::App;
use crate::workspace::documents::DocumentId;
use crate::workspace::focus::PanelFocus;

macro_rules! commands {
    ($( $id:ident => ($key:literal, $label:literal, $description:literal) ),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum CommandId { $( $id ),+ }
        impl CommandId {
            pub fn as_str(self) -> &'static str { match self { $(Self::$id => $key),+ } }
        }
        pub const REGISTRY: &[CommandMetadata] = &[
            $(CommandMetadata { id: CommandId::$id, label: $label, description: $description }),+
        ];
    };
}

pub struct CommandMetadata {
    pub id: CommandId,
    pub label: &'static str,
    pub description: &'static str,
}

commands! {
    Save => ("document.save", "Save retained document", "Revision-safe save of the captured owned document"),
    SaveAs => ("document.save_as", "Save retained document as…", "Create a new destination without overwriting"),
    Close => ("document.close", "Close retained document", "Save, discard or cancel unsaved changes"),
    Quit => ("workspace.quit", "Quit workspace", "Resolve every dirty document before quitting"),
    QuickOpen => ("navigation.quick_open", "Quick Open", "Search filenames and open directly"),
    OpenSelection => ("navigation.open_selection", "Open captured selection", "Open a file or navigate to a directory"),
    SelectionActions => ("navigation.selection_actions", "Actions for captured selection", "Existing secondary file navigation actions"),
    Documents => ("document.list", "List open documents", "Select an owned document by identity"),
    Pin => ("document.pin", "Pin retained document", "Keep this document open"),
    Previous => ("document.previous", "Previous document", "Switch relative to the captured document"),
    Next => ("document.next", "Next document", "Switch relative to the captured document"),
    Reveal => ("document.reveal", "Reveal retained document", "Navigate the explorer to its owned path"),
    FocusTree => ("focus.explorer", "Focus explorer", "Move keyboard focus to the tree"),
    FocusEditor => ("focus.editor", "Focus retained editor", "Focus the captured owned document"),
    FocusPreview => ("focus.preview", "Focus selected preview", "Show the browsing preview without dropping documents"),
    FocusTerminal => ("focus.terminal", "Focus terminal", "Focus the visible terminal without starting a process"),
    ToggleTerminal => ("pane.terminal.toggle", "Toggle terminal pane", "Use the existing explicit terminal toggle"),
    Wrap => ("view.wrap.toggle", "Toggle text wrapping", "Wrap the captured editor or focused preview"),
    ToggleExplorer => ("pane.explorer.toggle", "Toggle explorer pane", "Hide or show the explorer without dropping documents"),
    MaximizeEditor => ("pane.editor.maximize", "Maximize editor", "Temporarily maximize the document view"),
    MaximizeTerminal => ("pane.terminal.maximize", "Maximize terminal", "Temporarily maximize terminal without starting a shell"),
    RestoreLayout => ("pane.layout.restore", "Restore pane layout", "Restore saved pane sizes and visibility"),
    GrowExplorer => ("pane.explorer.grow", "Grow explorer", "Increase explorer width by two columns"),
    ShrinkExplorer => ("pane.explorer.shrink", "Shrink explorer", "Decrease explorer width by two columns"),
    Commands => ("workspace.commands", "Workspace commands", "Open the context-captured command menu"),
    FocusLeft => ("focus.left", "Focus left", "Use existing directional focus navigation"),
    FocusRight => ("focus.right", "Focus right", "Use existing directional focus navigation"),
    FocusUp => ("focus.up", "Focus up", "Use existing directional focus navigation"),
    FocusDown => ("focus.down", "Focus down", "Use existing directional focus navigation"),
    FocusCycle => ("focus.cycle", "Cycle focus", "Use existing panel focus cycling"),
    GrowTerminal => ("pane.terminal.grow", "Grow terminal", "Increase terminal height"),
    ShrinkTerminal => ("pane.terminal.shrink", "Shrink terminal", "Decrease terminal height"),
    CyclePreview => ("preview.view_mode", "Cycle selected preview mode", "Bounded full, head/tail, head-only and tail-only preview"),
    RecoveryRestore => ("recovery.restore", "Restore one recovery snapshot", "Restore the offered document's unsaved text (Phase 7)"),
    RecoveryDiscard => ("recovery.discard", "Discard one recovery snapshot", "Discard one document's owned recovery records (Phase 7)"),
    RecoveryClear => ("recovery.clear", "Clear all recovery snapshots", "Clear every owned recovery record (Phase 7)"),
    RecoveryDisable => ("recovery.disable", "Disable recovery", "Disable private snapshots (Phase 7)"),
    RecoveryEnable => ("recovery.enable", "Enable recovery", "Enable private recovery snapshots (Phase 7)"),
    LspStatus => ("lsp.status", "LSP status", "Show language-server capability/session status"),
    LspRestart => ("lsp.restart", "Restart LSP server", "Restart the current document's language server"),
    LspCompletion => ("lsp.completion", "Completion", "Request completion items at the cursor"),
    LspHover => ("lsp.hover", "Hover", "Request hover information at the cursor"),
    LspDefinition => ("lsp.definition", "Go to definition", "Jump to the symbol's definition"),
    LspReferences => ("lsp.references", "Find references", "List references to the symbol at the cursor"),
    LspSymbols => ("lsp.symbols", "Document symbols", "List this document's symbols"),

}

/// Owned origin, not a tree index or a pointer into a changing document list.
#[derive(Debug, Clone)]
pub struct CommandContext {
    pub document: Option<DocumentId>,
    pub panel: PanelFocus,
    pub presentation: crate::app::RightPanelPresentation,
    pub selection: Option<(std::path::PathBuf, bool)>,
}

impl CommandContext {
    /// The visible text view, not merely the most recently active document.
    pub fn text_view_document(&self, app: &App) -> Option<DocumentId> {
        let document = self
            .document
            .and_then(|id| app.workspace.documents.get(id))?;
        if self.panel == PanelFocus::Editor
            || (self.panel != PanelFocus::Preview
                && self.presentation == crate::app::RightPanelPresentation::RetainedDocument
                && (document.is_pinned() || self.panel == PanelFocus::Terminal))
        {
            Some(document.id())
        } else {
            None
        }
    }

    pub fn binding_context(&self, app: &App) -> crate::keymap::FocusContext {
        use crate::keymap::FocusContext;
        match self.panel {
            PanelFocus::Tree => FocusContext::Tree,
            PanelFocus::Preview => FocusContext::Preview,
            PanelFocus::Terminal => FocusContext::Terminal,
            PanelFocus::Editor
                if self
                    .document
                    .and_then(|id| app.workspace.documents.get(id))
                    .is_some_and(|d| d.editor.find_state.active) =>
            {
                FocusContext::EditorFind
            }
            PanelFocus::Editor => FocusContext::Editor,
        }
    }

    pub fn capture(app: &App) -> Self {
        let selection = app
            .tree_state
            .flat_items
            .get(app.tree_state.selected_index)
            .filter(|item| {
                matches!(
                    item.node_type,
                    crate::fs::tree::NodeType::File | crate::fs::tree::NodeType::Directory
                )
            })
            .map(|item| {
                (
                    item.path.clone(),
                    item.node_type == crate::fs::tree::NodeType::Directory,
                )
            });
        Self {
            document: app.workspace.documents.active_id(),
            panel: app.workspace.focus.panel,
            presentation: app.right_panel_presentation,
            selection,
        }
    }
}

/// No filesystem queries, process checks, indexing, or permission broadening.
/// A retained document is always named explicitly; selected previews are never
/// save targets. Admission and the safe save layer remain authoritative.
pub fn unavailable_reason(
    app: &App,
    context: &CommandContext,
    id: CommandId,
) -> Option<&'static str> {
    use CommandId::*;
    let owned = context
        .document
        .and_then(|id| app.workspace.documents.get(id));
    let document_reason = || {
        if context.document.is_some() {
            "Captured document is no longer owned"
        } else {
            "No retained editable document (preview is read-only)"
        }
    };
    if context.document.is_some() && owned.is_none() {
        return Some(document_reason());
    }
    match id {
        CyclePreview if !app.config.preview_enabled() => {
            Some("Automatic preview disabled by configuration")
        }
        CyclePreview if app.preview_state.current_path.is_none() => Some("No selected preview"),
        CyclePreview
            if !app.preview_state.is_large_file
                && !context
                    .document
                    .and_then(|id| app.workspace.documents.get(id))
                    .is_some_and(|d| {
                        app.preview_state.current_path.as_deref() == Some(d.path())
                    }) =>
        {
            Some("Selected preview is not admitted text")
        }
        // Private recovery commands require an injected store. An app built
        // without one (an unconfigured instance) keeps the previously
        // documented staged reason, so no snapshot is ever written or read by
        // such an app.
        RecoveryRestore | RecoveryDiscard | RecoveryClear | RecoveryDisable | RecoveryEnable
            if app.recovery.is_none() =>
        {
            Some("Private recovery is not implemented yet (Phase 7)")
        }
        RecoveryRestore | RecoveryDiscard | RecoveryClear | RecoveryDisable
            if !app.recovery_enabled() =>
        {
            Some("Private recovery is disabled")
        }
        RecoveryRestore | RecoveryDiscard
            if app
                .recovery
                .as_ref()
                .is_some_and(|context| context.records.is_empty()) =>
        {
            Some("No recovery snapshots available")
        }
        RecoveryClear
            if app
                .recovery
                .as_ref()
                .is_some_and(|context| context.records.is_empty()) =>
        {
            Some("No recovery records to clear")
        }
        RecoveryEnable if app.recovery_enabled() => Some("Private recovery already enabled"),
        Save | SaveAs if app.is_s3_mode() => Some("S3 workspace is read-only"),
        Save | SaveAs | Close | Pin | Previous | Next | Reveal | FocusEditor if owned.is_none() => {
            Some(document_reason())
        }
        QuickOpen | OpenSelection | SelectionActions if app.is_s3_mode() => {
            Some("Local document navigation is unavailable in S3 mode")
        }
        OpenSelection | SelectionActions if context.selection.is_none() => {
            Some("No captured file or directory selection")
        }
        Documents if app.workspace.documents.is_empty() => Some("No open documents"),
        FocusTree if !app.pane_available(PanelFocus::Tree) => {
            Some("Explorer is not usable in this layout")
        }
        FocusEditor | FocusPreview if !app.pane_available(PanelFocus::Preview) => {
            Some("Document view is not usable in this layout")
        }
        ToggleTerminal | MaximizeTerminal | GrowTerminal | ShrinkTerminal | FocusTerminal
            if app.is_s3_mode() =>
        {
            Some("Terminal unavailable in S3 mode")
        }
        FocusTerminal if !app.pane_available(PanelFocus::Terminal) => {
            Some("Terminal pane is not usable in this layout")
        }
        ToggleTerminal | MaximizeTerminal | GrowTerminal | ShrinkTerminal
            if !app.config.terminal_enabled() =>
        {
            Some("Terminal disabled by configuration")
        }
        GrowTerminal | ShrinkTerminal
            if app.workspace.layout.maximized().is_some()
                || app.pane_rects().terminal_split.height == 0 =>
        {
            Some("Terminal has no resizable split")
        }
        GrowExplorer | ShrinkExplorer
            if app.workspace.layout.maximized().is_some()
                || app.pane_rects().explorer_split.width == 0 =>
        {
            Some("Explorer has no resizable split")
        }
        ToggleTerminal if app.event_tx.is_none() => Some("Terminal event routing is unavailable"),
        LspStatus | LspRestart | LspCompletion | LspHover | LspDefinition | LspReferences
        | LspSymbols
            if !app.config.lsp.enabled() =>
        {
            Some("LSP disabled by configuration")
        }
        LspCompletion | LspHover | LspDefinition | LspReferences | LspSymbols
            if app.current_lsp_language().is_none() =>
        {
            Some("No LSP language for the current document")
        }
        LspRestart
            if app
                .current_lsp_language()
                .and_then(|l| app.lsp.session_status(&l))
                .is_none() =>
        {
            Some("No LSP session for the current document")
        }
        Wrap if context.text_view_document(app).is_none() && !app.config.preview_enabled() => {
            Some("Preview disabled by configuration")
        }
        Wrap if context.panel != PanelFocus::Preview
            && context.presentation != crate::app::RightPanelPresentation::SelectedPreview
            && owned.is_none() =>
        {
            Some(document_reason())
        }
        _ => None,
    }
}

/// The menu and future keymaps share this dispatch boundary. Recheck against
/// live ownership before dismissing; a missing origin never becomes another ID.
pub fn dispatch_command(app: &mut App, id: CommandId) -> Result<(), String> {
    // Toggle the explicit entry without dismiss/reopen or nested modal frames.
    if id == CommandId::Commands {
        if app.workspace.focus.overlay == crate::app::AppMode::CommandMenu {
            app.dismiss_command_menu();
            return Ok(());
        }
        if app.workspace.focus.overlay != crate::app::AppMode::Normal {
            return Err("Finish the current overlay before opening commands".into());
        }
        app.open_command_menu();
        return Ok(());
    }
    let context = if app.workspace.focus.overlay == crate::app::AppMode::CommandMenu {
        app.command_menu
            .as_ref()
            .map(|m| m.origin.clone())
            .ok_or_else(|| "Command menu origin is missing".to_string())?
    } else if app.workspace.focus.overlay == crate::app::AppMode::Normal {
        CommandContext::capture(app)
    } else {
        return Err("Finish the current overlay before running a command".into());
    };
    if let Some(reason) = unavailable_reason(app, &context, id) {
        if let Some(menu) = app.command_menu.as_mut() {
            menu.feedback = Some(reason.into());
        }
        app.set_status_message(reason.into());
        return Err(reason.into());
    }
    if app.workspace.focus.overlay == crate::app::AppMode::CommandMenu {
        app.dismiss_command_menu();
    }
    // Explicit activation is necessary even when the active ID changed while
    // the menu was open. Checked above, no fallback to the unrelated active ID.
    if let Some(document) = context.document {
        if app.workspace.documents.get(document).is_some() {
            let _ = app.workspace.documents.activate(document);
        }
    }
    use CommandId::*;
    match id {
        Commands => unreachable!("menu entry handled above"),
        FocusLeft => app.focus_left(),
        FocusRight => app.focus_right(),
        FocusUp => app.focus_up(),
        FocusDown => app.focus_down(),
        FocusCycle => app.toggle_focus(),
        GrowTerminal => app.resize_terminal_down(),
        ShrinkTerminal => app.resize_terminal_up(),
        GrowExplorer | ShrinkExplorer => {
            let area = app
                .workspace_area
                .unwrap_or(ratatui::layout::Rect::new(0, 0, 120, 40));
            app.workspace
                .layout
                .resize_explorer(area, if id == GrowExplorer { 2 } else { -2 });
            app.layout_changed();
        }
        ToggleExplorer => {
            app.workspace.layout.toggle_explorer();
            app.layout_changed();
        }
        MaximizeEditor | MaximizeTerminal => {
            app.workspace.layout.maximize(if id == MaximizeEditor {
                crate::workspace::layout::MaximizedPane::Document
            } else {
                crate::workspace::layout::MaximizedPane::Terminal
            });
            app.layout_changed();
            if id == MaximizeTerminal && app.pane_available(PanelFocus::Terminal) {
                app.workspace.focus.panel = PanelFocus::Terminal;
            }
        }
        RestoreLayout => {
            app.workspace.layout.restore();
            app.layout_changed();
        }
        Save => app.save_editor_buffer()?,
        SaveAs => app.open_dialog(crate::app::DialogKind::EditorSaveAs {
            exit_after_save: false,
            normalize: false,
        }),
        Close => app.close_active_document(),
        Quit => app.quit(),
        QuickOpen => app.open_search(),
        OpenSelection => {
            let (path, directory) = context.selection.as_ref().expect("selection checked");
            if *directory {
                app.navigate_to_path(path);
                app.workspace.focus.panel = PanelFocus::Tree;
            } else {
                app.open_document_path(path, true);
            }
        }
        SelectionActions => {
            app.command_selection_actions = true;
            let (path, directory) = context.selection.expect("selection checked");
            let (_, binary) = App::detect_file_type(&path);
            app.search_action_state = Some(crate::app::SearchActionState {
                display: path.to_string_lossy().into_owned(),
                path,
                is_directory: directory,
                is_binary: binary,
            });
            app.set_overlay(crate::app::AppMode::SearchAction);
        }
        Documents => app.open_document_list(),
        Pin => app.pin_document(),
        Previous | Next => app.cycle_document(id == Next),
        Reveal => app.reveal_document(),
        FocusTree => app.workspace.focus.panel = PanelFocus::Tree,
        FocusEditor => app.activate_document(context.document.expect("document checked")),
        FocusPreview => {
            app.show_selected_preview();
            app.workspace.focus.panel = PanelFocus::Preview;
        }
        FocusTerminal => app.workspace.focus.panel = PanelFocus::Terminal,
        ToggleTerminal => {
            let tx = app.event_tx.clone().expect("event routing checked");
            app.toggle_terminal(&tx);
        }
        CyclePreview => app.cycle_view_mode(),
        LspStatus => app.show_lsp_status(),
        LspRestart => app.restart_lsp_current()?,
        LspCompletion => app.lsp_completion()?,
        LspHover => app.lsp_hover()?,
        LspDefinition => app.lsp_definition()?,
        LspReferences => app.lsp_references()?,
        LspSymbols => app.lsp_document_symbols()?,
        Wrap => {
            if let Some(document) = context.text_view_document(app) {
                app.workspace
                    .documents
                    .get_mut(document)
                    .expect("ownership checked")
                    .editor
                    .toggle_wrap();
            } else {
                app.preview_toggle_wrap();
            }
        }
        RecoveryRestore => {
            let message = app.restore_recovery()?;
            app.set_status_message(message);
        }
        RecoveryDiscard => {
            let message = app.discard_recovery()?;
            app.set_status_message(message);
        }
        RecoveryClear => {
            let message = app.clear_recovery()?;
            app.set_status_message(message);
        }
        RecoveryDisable | RecoveryEnable => {
            let message = app.set_recovery_enabled(id == RecoveryEnable);
            app.set_status_message(message);
        }
    }
    app.reconcile_pane_focus();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    fn fixture() -> (tempfile::TempDir, App, DocumentId) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("a.txt");
        std::fs::write(&path, "alpha").unwrap();
        let mut app = App::new(root.path(), AppConfig::default()).unwrap();
        assert!(app.open_document_path(&path, true));
        let id = app.workspace.documents.active_id().unwrap();
        (root, app, id)
    }

    #[test]
    fn task3_terminal_size_command_labels_match_actual_size_changes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.layout.set_terminal_height(7);
        dispatch_command(&mut app, CommandId::GrowTerminal).unwrap();
        assert_eq!(app.workspace.layout.terminal_height(), 9);
        dispatch_command(&mut app, CommandId::ShrinkTerminal).unwrap();
        assert_eq!(app.workspace.layout.terminal_height(), 7);
    }

    #[test]
    fn task3_wrap_targets_selected_preview_not_retained_editor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "long line").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        app.open_document_path(&path, true);
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = PanelFocus::Terminal;
        app.show_selected_preview();
        dispatch_command(&mut app, CommandId::Wrap).unwrap();
        assert!(app.preview_state.line_wrap);
        assert!(!app.workspace.documents.active().unwrap().editor.line_wrap);
    }

    #[test]
    fn commands_metadata_is_complete_and_unique() {
        assert_eq!(REGISTRY.len(), 45);
        let ids: std::collections::HashSet<_> = REGISTRY.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids.len(), REGISTRY.len());
        for m in REGISTRY {
            assert!(!m.label.is_empty() && !m.description.is_empty());
        }
        assert!(ids.contains("document.save"));
        assert!(ids.contains("recovery.clear"));
    }

    #[test]
    fn commands_save_owned_document_and_availability() {
        let (root, mut app, id) = fixture();
        app.workspace
            .documents
            .get_mut(id)
            .unwrap()
            .editor
            .insert_text("dirty")
            .unwrap();
        let context = CommandContext::capture(&app);
        assert_eq!(unavailable_reason(&app, &context, CommandId::Save), None);
        dispatch_command(&mut app, CommandId::Save).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "dirtyalpha"
        );
        assert!(!app.workspace.documents.get(id).unwrap().editor.modified);
    }

    fn open_b(app: &mut App, root: &std::path::Path) -> DocumentId {
        let b = root.join("b.txt");
        std::fs::write(&b, "beta").unwrap();
        app.open_document_path(&b, true);
        app.workspace.documents.active_id().unwrap()
    }

    #[test]
    fn commands_menu_save_and_close_use_captured_id_after_active_changes() {
        let (root, mut app, a) = fixture();
        let b = open_b(&mut app, root.path());
        app.activate_document(a);
        app.workspace
            .documents
            .get_mut(a)
            .unwrap()
            .editor
            .insert_text("dirty")
            .unwrap();
        app.open_command_menu();
        app.workspace.documents.activate(b).unwrap();
        dispatch_command(&mut app, CommandId::Save).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "dirtyalpha"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("b.txt")).unwrap(),
            "beta"
        );
        app.open_command_menu();
        app.workspace.documents.activate(b).unwrap();
        dispatch_command(&mut app, CommandId::Close).unwrap();
        assert!(app.workspace.documents.get(a).is_none());
        assert!(app.workspace.documents.get(b).is_some());
    }

    #[test]
    fn commands_missing_origin_and_disabled_activation_keep_menu_without_side_effects() {
        let (root, mut app, a) = fixture();
        let b = open_b(&mut app, root.path());
        app.activate_document(a);
        app.open_command_menu();
        assert!(dispatch_command(&mut app, CommandId::RecoveryClear).is_err());
        assert_eq!(
            app.workspace.focus.overlay,
            crate::app::AppMode::CommandMenu
        );
        assert!(app
            .command_menu
            .as_ref()
            .unwrap()
            .feedback
            .as_ref()
            .unwrap()
            .contains("Phase 7"));
        app.workspace.documents.discard_and_close(a).unwrap();
        app.workspace.documents.activate(b).unwrap();
        for id in [
            CommandId::Save,
            CommandId::SaveAs,
            CommandId::Close,
            CommandId::Quit,
            CommandId::Wrap,
        ] {
            assert!(dispatch_command(&mut app, id).is_err());
        }
        assert!(!app.should_quit);
        assert_eq!(app.workspace.documents.active_id(), Some(b));
        assert_eq!(
            std::fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "alpha"
        );
        assert_eq!(app.workspace.documents.get(b).unwrap().text(), "beta");
        assert_eq!(
            app.workspace.focus.overlay,
            crate::app::AppMode::CommandMenu
        );
        app.command_menu = None;
        assert!(dispatch_command(&mut app, CommandId::Save)
            .unwrap_err()
            .contains("origin"));
    }

    #[test]
    fn commands_availability_no_doc_binary_readonly_and_s3_never_write_previews() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("binary.dat");
        std::fs::write(&binary, b"\0binary").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        assert!(!app.open_document_path(&binary, true));
        let context = CommandContext::capture(&app);
        for id in [
            CommandId::Save,
            CommandId::SaveAs,
            CommandId::Close,
            CommandId::Pin,
            CommandId::Previous,
            CommandId::Next,
            CommandId::Reveal,
            CommandId::FocusEditor,
            CommandId::Documents,
        ] {
            assert!(unavailable_reason(&app, &context, id).is_some());
            assert!(dispatch_command(&mut app, id).is_err());
        }
        assert_eq!(std::fs::read(&binary).unwrap(), b"\0binary");
        let readonly = dir.path().join("readonly.txt");
        std::fs::write(&readonly, "original").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o444)).unwrap();
            assert!(!app.open_document_path(&readonly, true));
            assert!(dispatch_command(&mut app, CommandId::Save).is_err());
            assert_eq!(std::fs::read_to_string(&readonly).unwrap(), "original");
        }
        let (root, mut app, a) = fixture();
        app.init_s3_mode(crate::s3::S3Config {
            path: crate::s3::S3Path::parse("s3://bucket/key").unwrap(),
            profile: None,
        });
        for id in [
            CommandId::Save,
            CommandId::SaveAs,
            CommandId::QuickOpen,
            CommandId::OpenSelection,
            CommandId::SelectionActions,
        ] {
            assert!(unavailable_reason(&app, &CommandContext::capture(&app), id).is_some());
            assert!(dispatch_command(&mut app, id).is_err());
        }
        assert_eq!(app.workspace.documents.get(a).unwrap().text(), "alpha");
        assert_eq!(
            std::fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "alpha"
        );
    }

    #[test]
    fn commands_retained_save_never_targets_selected_binary_preview() {
        let (root, mut app, a) = fixture();
        let binary = root.path().join("binary.dat");
        std::fs::write(&binary, b"\0keep").unwrap();
        app.workspace
            .documents
            .get_mut(a)
            .unwrap()
            .editor
            .insert_text("dirty")
            .unwrap();
        app.open_document_path(&binary, true);
        assert!(!app.editor_visible());
        app.open_command_menu();
        dispatch_command(&mut app, CommandId::Save).unwrap();
        assert_eq!(std::fs::read(&binary).unwrap(), b"\0keep");
        assert_eq!(
            std::fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "dirtyalpha"
        );
    }

    #[test]
    fn commands_conflict_nested_copy_and_save_as_preserve_origin_and_return_stack() {
        let (root, mut app, a) = fixture();
        let b = open_b(&mut app, root.path());
        app.activate_document(a);
        app.workspace
            .documents
            .get_mut(a)
            .unwrap()
            .editor
            .insert_text("dirty")
            .unwrap();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = PanelFocus::Terminal;
        app.open_command_menu();
        app.workspace.documents.activate(b).unwrap();
        std::fs::write(root.path().join("a.txt"), "external").unwrap();
        assert!(dispatch_command(&mut app, CommandId::Save).is_err());
        assert!(matches!(
            app.workspace.focus.overlay,
            crate::app::AppMode::Dialog(crate::app::DialogKind::SaveConflict { .. })
        ));
        assert_eq!(app.workspace.focus.overlay_document(), Some(a));
        let mut output = Vec::new();
        app.show_copyable_text("literal".into(), &mut output, false);
        app.workspace.documents.activate(b).unwrap();
        app.dismiss_copy_overlay();
        assert_eq!(app.workspace.focus.overlay_document(), Some(a));
        app.open_dialog(crate::app::DialogKind::EditorSaveAs {
            exit_after_save: false,
            normalize: false,
        });
        app.workspace.documents.activate(b).unwrap();
        app.save_editor_as("saved.txt", false, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("saved.txt")).unwrap(),
            "dirtyalpha"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "external"
        );
        // Save As dismisses the conflict workflow, never resurrects the menu.
        assert_eq!(app.workspace.focus.overlay, crate::app::AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, PanelFocus::Terminal);
        assert_eq!(app.workspace.documents.active_id(), Some(a));
        assert!(app.workspace.focus.dismiss_overlay().is_none());
    }

    #[test]
    fn commands_dirty_close_cancel_and_quit_dialog_preserve_owned_origin() {
        let (root, mut app, a) = fixture();
        let b = open_b(&mut app, root.path());
        app.activate_document(a);
        app.workspace
            .documents
            .get_mut(a)
            .unwrap()
            .editor
            .insert_text("dirty")
            .unwrap();
        app.open_command_menu();
        app.workspace.documents.activate(b).unwrap();
        dispatch_command(&mut app, CommandId::Close).unwrap();
        assert_eq!(app.workspace.focus.overlay_document(), Some(a));
        app.close_dialog();
        assert!(app.workspace.documents.get(a).unwrap().editor.modified);
        assert_eq!(app.workspace.documents.active_id(), Some(a));
        app.open_command_menu();
        dispatch_command(&mut app, CommandId::Quit).unwrap();
        assert_eq!(app.workspace.focus.overlay_document(), Some(a));
        app.close_dialog();
        assert!(!app.should_quit);
        assert!(app.workspace.documents.get(a).is_some());
        assert_eq!(app.workspace.focus.overlay, crate::app::AppMode::Normal);
        assert!(app.workspace.focus.dismiss_overlay().is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn commands_navigation_focus_wrap_and_explicit_terminal_hide_dispatch() {
        let (root, mut app, a) = fixture();
        let b = open_b(&mut app, root.path());
        dispatch_command(&mut app, CommandId::Previous).unwrap();
        assert_eq!(app.workspace.documents.active_id(), Some(a));
        dispatch_command(&mut app, CommandId::Next).unwrap();
        assert_eq!(app.workspace.documents.active_id(), Some(b));
        dispatch_command(&mut app, CommandId::Pin).unwrap();
        assert!(app.workspace.documents.get(b).unwrap().is_pinned());
        dispatch_command(&mut app, CommandId::Wrap).unwrap();
        assert!(app.editor().unwrap().line_wrap);
        for (id, panel) in [
            (CommandId::FocusTree, PanelFocus::Tree),
            (CommandId::FocusEditor, PanelFocus::Editor),
            (CommandId::FocusPreview, PanelFocus::Preview),
        ] {
            dispatch_command(&mut app, id).unwrap();
            assert_eq!(app.workspace.focus.panel, panel);
        }
        dispatch_command(&mut app, CommandId::Wrap).unwrap();
        assert!(app.preview_state.line_wrap);
        assert!(dispatch_command(&mut app, CommandId::FocusTerminal).is_err());
        assert!(dispatch_command(&mut app, CommandId::ToggleTerminal).is_err());
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.config.terminal.default_shell = Some("/bin/sh".into());
        assert!(app.open_terminal(&tx));
        app.event_tx = Some(tx);
        dispatch_command(&mut app, CommandId::FocusTerminal).unwrap();
        assert_eq!(app.workspace.focus.panel, PanelFocus::Terminal);
        dispatch_command(&mut app, CommandId::ToggleTerminal).unwrap();
        assert!(!app.workspace.layout.terminal_visible());
        assert!(app.terminal_state.pty.as_ref().unwrap().is_alive());
        app.shutdown_terminal();
        dispatch_command(&mut app, CommandId::Documents).unwrap();
        assert_eq!(app.document_list.as_ref().unwrap(), &vec![a, b]);
        assert!(dispatch_command(&mut app, CommandId::Save).is_err()); // other modal
        app.close_search();
        dispatch_command(&mut app, CommandId::SaveAs).unwrap();
        assert_eq!(app.workspace.focus.overlay_document(), Some(b));
        app.close_dialog();
        dispatch_command(&mut app, CommandId::Reveal).unwrap();
        assert_eq!(app.workspace.focus.panel, PanelFocus::Tree);
        dispatch_command(&mut app, CommandId::QuickOpen).unwrap();
        assert_eq!(app.workspace.focus.overlay, crate::app::AppMode::Search);
    }

    #[test]
    fn commands_secondary_actions_cancel_returns_origin_not_stale_search() {
        let (root, mut app, a) = fixture();
        app.tree_state = crate::fs::tree::TreeState::new(root.path()).unwrap();
        app.navigate_to_path(&root.path().join("a.txt"));
        app.open_command_menu();
        dispatch_command(&mut app, CommandId::SelectionActions).unwrap();
        assert_eq!(
            app.search_action_state.as_ref().unwrap().path,
            root.path().join("a.txt")
        );
        app.search_action_back();
        assert_eq!(app.workspace.focus.overlay, crate::app::AppMode::Normal);
        assert_eq!(app.workspace.documents.active_id(), Some(a));
    }

    #[test]
    fn commands_direct_selection_captures_path_not_changed_row() {
        let (root, mut app, a) = fixture();
        let b = open_b(&mut app, root.path());
        app.tree_state = crate::fs::tree::TreeState::new(root.path()).unwrap();
        app.navigate_to_path(&root.path().join("a.txt"));
        app.open_command_menu();
        app.tree_state.selected_index = 0;
        app.workspace.documents.activate(b).unwrap();
        dispatch_command(&mut app, CommandId::OpenSelection).unwrap();
        assert_eq!(app.workspace.documents.active_id(), Some(a));
        app.tree_state.selected_index = 0;
        app.open_command_menu();
        dispatch_command(&mut app, CommandId::OpenSelection).unwrap();
        assert_eq!(app.workspace.focus.panel, PanelFocus::Tree);
        assert_eq!(app.workspace.documents.active_id(), Some(a));
    }

    #[test]
    fn commands_availability_is_in_memory_and_activation_rechecks_it() {
        let (root, mut app, _a) = fixture();
        let mut context = CommandContext::capture(&app);
        std::fs::remove_file(root.path().join("a.txt")).unwrap();
        assert_eq!(unavailable_reason(&app, &context, CommandId::Save), None);
        context.selection = None;
        assert!(unavailable_reason(&app, &context, CommandId::OpenSelection).is_some());
        assert!(unavailable_reason(&app, &context, CommandId::SelectionActions).is_some());
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.open_command_menu();
        if app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        assert!(dispatch_command(&mut app, CommandId::FocusTerminal).is_err());
        assert_eq!(
            app.workspace.focus.overlay,
            crate::app::AppMode::CommandMenu
        );
        for id in [
            CommandId::RecoveryRestore,
            CommandId::RecoveryDiscard,
            CommandId::RecoveryClear,
            CommandId::RecoveryDisable,
        ] {
            assert!(dispatch_command(&mut app, id).is_err());
        }
    }

    #[test]
    fn lsp_commands_dispatch_and_gate_on_configuration() {
        let (_root, mut app, _doc) = fixture();
        let context = CommandContext::capture(&app);

        // No configured server for the active document → restart is gated.
        assert_eq!(
            unavailable_reason(&app, &context, CommandId::LspRestart),
            Some("No LSP session for the current document")
        );
        // Status is always available while LSP is enabled (it reports the
        // empty state), and dispatch opens the status dialog.
        assert_eq!(
            unavailable_reason(&app, &context, CommandId::LspStatus),
            None
        );
        dispatch_command(&mut app, CommandId::LspStatus).unwrap();
        assert!(matches!(
            app.workspace.focus.overlay,
            crate::app::AppMode::Dialog(crate::app::DialogKind::LspStatus { .. })
        ));
        app.close_dialog();

        // LSP disabled by configuration gates both commands.
        app.config.lsp.enabled = Some(false);
        let context = CommandContext::capture(&app);
        for id in [CommandId::LspStatus, CommandId::LspRestart] {
            assert_eq!(
                unavailable_reason(&app, &context, id),
                Some("LSP disabled by configuration")
            );
        }
    }

    #[test]
    fn lsp_feature_commands_dispatch_to_app_methods() {
        // With LSP enabled and an editor open on a mapped language, all five
        // feature commands reach their dispatch arm — they fail inside the
        // app method (no ready session), which means the arm executed.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        app.open_document_path(&file, true);
        for id in [
            CommandId::LspCompletion,
            CommandId::LspHover,
            CommandId::LspDefinition,
            CommandId::LspReferences,
            CommandId::LspSymbols,
        ] {
            let context = CommandContext::capture(&app);
            assert!(unavailable_reason(&app, &context, id).is_none(), "{id:?}");
            assert!(dispatch_command(&mut app, id).is_err(), "{id:?}");
        }
    }

    #[test]
    fn lsp_restart_dispatches_to_a_live_session() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let mut config = crate::config::AppConfig::default();
        config.lsp_global.servers.insert(
            "rust".to_string(),
            crate::lsp::config::ServerEntry {
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
        app.init_lsp_trust();
        app.open_document_path(&file, true);
        assert_eq!(
            app.lsp.session_status("rust"),
            Some(&crate::lsp::SessionStatus::Starting)
        );

        let context = CommandContext::capture(&app);
        assert_eq!(
            unavailable_reason(&app, &context, CommandId::LspRestart),
            None
        );
        dispatch_command(&mut app, CommandId::LspRestart).unwrap();
        app.shutdown_lsp();
    }
}
