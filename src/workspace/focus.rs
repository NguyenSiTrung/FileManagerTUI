//! Input focus and overlays do not own, replace, or evict editor buffers.
use crate::app::DialogKind;

use super::documents::DocumentId;

/// Keyboard/mouse focus, independent of the active document.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PanelFocus {
    #[default]
    Tree,
    Preview,
    Editor,
    Terminal,
}

/// Modal input contexts only. Editing is panel focus, not a global mode.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum InputOverlay {
    #[default]
    Normal,
    Dialog(DialogKind),
    Search,
    SearchAction,
    Filter,
    Help,
    CopyOverlay,
    CommandMenu,
    /// Language-feature results (completion/hover/locations/symbols).
    LanguageFeatures,
    /// Navigable diagnostics panel (published diagnostics across servers).
    Diagnostics,
}

/// Explicit input destination. A focused empty editor must not dispatch tree keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputTarget {
    Overlay,
    Tree,
    Preview,
    Editor(DocumentId),
    Terminal,
    NoDocument,
}

/// Input context returned on dismissal, including the originating document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusContext {
    pub panel: PanelFocus,
    pub document: Option<DocumentId>,
    pub overlay: InputOverlay,
    pub overlay_document: Option<DocumentId>,
}

/// An overlay cannot replace its context when the bounded nesting limit is hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("too many nested input overlays")]
pub struct OverlayLimit;

/// Identity of an opened workflow, never inferred from overlay value equality.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkflowId(u64);

#[derive(Debug)]
struct ReturnFrame {
    context: FocusContext,
    identity: WorkflowId,
}

/// The sole panel/overlay state; document ownership remains in DocumentStore.
#[derive(Debug, Default)]
pub struct FocusState {
    pub panel: PanelFocus,
    pub overlay: InputOverlay,
    overlay_document: Option<DocumentId>,
    return_contexts: Vec<ReturnFrame>,
    identity: WorkflowId,
    next_identity: u64,
}

impl FocusState {
    /// Resolve input without consulting a global edit flag.
    pub fn input_target(&self, active: Option<DocumentId>) -> InputTarget {
        if self.overlay != InputOverlay::Normal {
            return InputTarget::Overlay;
        }
        match self.panel {
            PanelFocus::Tree => InputTarget::Tree,
            PanelFocus::Preview => InputTarget::Preview,
            PanelFocus::Editor => active.map_or(InputTarget::NoDocument, InputTarget::Editor),
            PanelFocus::Terminal => InputTarget::Terminal,
        }
    }

    /// Capture the original context before opening a modal input destination.
    pub fn open_overlay(
        &mut self,
        overlay: InputOverlay,
        document: Option<DocumentId>,
    ) -> Result<(), OverlayLimit> {
        self.open_overlay_for(overlay, document, document)
    }

    /// The save target may differ from the active document at quit initiation.
    pub fn open_overlay_for(
        &mut self,
        overlay: InputOverlay,
        document: Option<DocumentId>,
        target: Option<DocumentId>,
    ) -> Result<(), OverlayLimit> {
        if self.return_contexts.len() >= 8 {
            return Err(OverlayLimit);
        }
        let next = self.next_identity.checked_add(1).ok_or(OverlayLimit)?;
        self.return_contexts.push(ReturnFrame {
            context: FocusContext {
                panel: self.panel,
                document,
                overlay: self.overlay.clone(),
                overlay_document: self.overlay_document,
            },
            identity: self.identity,
        });
        self.next_identity = next;
        self.identity = WorkflowId(next);
        self.overlay = overlay;
        self.overlay_document = target;
        Ok(())
    }

    /// Restore the previous context; the coordinator may activate its document.
    pub fn dismiss_overlay(&mut self) -> Option<FocusContext> {
        let frame = self.return_contexts.pop()?;
        self.identity = frame.identity;
        let context = frame.context;
        self.panel = context.panel;
        self.overlay = context.overlay.clone();
        self.overlay_document = context.overlay_document;
        Some(context)
    }

    /// The document targeted when this overlay opened, not the latest active ID.
    pub fn overlay_document(&self) -> Option<DocumentId> {
        self.overlay_document
    }

    /// Follow-up steps in the same modal workflow retain the original frame.
    pub fn replace_overlay(&mut self, overlay: InputOverlay) {
        self.overlay = overlay;
    }

    pub(crate) fn workflow_identity(&self) -> WorkflowId {
        self.identity
    }

    pub(crate) fn return_workflow(&self) -> Option<(&FocusContext, WorkflowId)> {
        self.return_contexts
            .last()
            .map(|frame| (&frame.context, frame.identity))
    }

    pub(crate) fn can_open_overlay(&self) -> bool {
        self.return_contexts.len() < 8 && self.next_identity != u64::MAX
    }

    fn is_progress(overlay: &InputOverlay) -> bool {
        matches!(overlay, InputOverlay::Dialog(DialogKind::Progress { .. }))
    }

    /// Compatibility envelopes lacking an identity retire the oldest operation,
    /// never a newer nested Progress workflow.
    #[allow(dead_code)]
    pub(crate) fn oldest_progress_identity(&self) -> Option<WorkflowId> {
        self.return_contexts
            .iter()
            .find(|frame| Self::is_progress(&frame.context.overlay))
            .map(|frame| frame.identity)
            .or_else(|| Self::is_progress(&self.overlay).then_some(self.identity))
    }

    /// Remove only the captured Progress frame, preserving every nested origin.
    pub(crate) fn retire_progress_for(&mut self, identity: WorkflowId) -> Option<FocusContext> {
        if self.identity == identity && Self::is_progress(&self.overlay) {
            let context = self.dismiss_overlay();
            if context.is_none() {
                self.overlay = InputOverlay::Normal;
                self.overlay_document = None;
                self.identity = WorkflowId::default();
            }
            return context;
        }
        if let Some(index) = self.return_contexts.iter().position(|frame| {
            frame.identity == identity && Self::is_progress(&frame.context.overlay)
        }) {
            self.return_contexts.remove(index);
        }
        None
    }

    /// Retire only the file-operation progress workflow. A nested overlay keeps
    /// its live input/target and later returns to the context before Progress.
    /// No job generations are inferred: Progress is the existing operation UI.
    #[allow(dead_code)]
    pub fn retire_progress(&mut self) -> Option<FocusContext> {
        let identity = if Self::is_progress(&self.overlay) {
            Some(self.identity)
        } else {
            self.return_contexts
                .iter()
                .rfind(|frame| Self::is_progress(&frame.context.overlay))
                .map(|frame| frame.identity)
        };
        identity.and_then(|identity| self.retire_progress_for(identity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::documents::{DocumentStore, OpenDisposition};

    #[test]
    fn app_jobs_focus_identity_targeting_preserves_new_nested_progress_and_renewal() {
        let mut focus = FocusState::default();
        let progress = || {
            InputOverlay::Dialog(DialogKind::Progress {
                message: "identical".into(),
                current: 0,
                total: 1,
            })
        };
        focus.open_overlay(progress(), None).unwrap();
        let old = focus.workflow_identity();
        focus.open_overlay(InputOverlay::CopyOverlay, None).unwrap();
        let nested = focus.workflow_identity();
        focus.open_overlay(progress(), None).unwrap();
        let newer = focus.workflow_identity();
        assert_ne!(old, newer);
        assert!(focus.retire_progress_for(old).is_none());
        assert_eq!(focus.workflow_identity(), newer);
        focus.dismiss_overlay().unwrap();
        assert_eq!(focus.workflow_identity(), nested);
        focus.dismiss_overlay().unwrap();
        assert_eq!(focus.overlay, InputOverlay::Normal);
        focus.open_overlay(progress(), None).unwrap();
        let renewed = focus.workflow_identity();
        assert_ne!(renewed, old);
        assert!(focus.retire_progress_for(old).is_none());
        assert_eq!(focus.workflow_identity(), renewed);
        assert!(focus.retire_progress_for(renewed).is_some());
        assert_eq!(focus.overlay, InputOverlay::Normal);
    }

    fn documents() -> (tempfile::TempDir, DocumentStore, DocumentId, DocumentId) {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a.txt");
        let b = root.path().join("b.txt");
        std::fs::write(&a, "alpha").unwrap();
        std::fs::write(&b, "beta").unwrap();
        let mut documents = DocumentStore::new();
        let a = documents.open(&a, OpenDisposition::Pinned).unwrap();
        let b = documents.open(&b, OpenDisposition::Pinned).unwrap();
        (root, documents, a, b)
    }

    #[test]
    fn focus_changes_keep_documents_and_route_by_context() {
        let (_root, mut documents, a, b) = documents();
        documents.activate(a).unwrap();
        documents
            .get_mut(a)
            .unwrap()
            .editor
            .insert_text("unsaved")
            .unwrap();
        let mut focus = FocusState::default();
        for (panel, target) in [
            (PanelFocus::Editor, InputTarget::Editor(a)),
            (PanelFocus::Tree, InputTarget::Tree),
            (PanelFocus::Preview, InputTarget::Preview),
            (PanelFocus::Terminal, InputTarget::Terminal),
        ] {
            focus.panel = panel;
            assert_eq!(focus.input_target(documents.active_id()), target);
            assert_eq!(documents.active_id(), Some(a));
        }
        documents.activate(b).unwrap();
        documents.activate(a).unwrap();
        assert!(documents.get(a).unwrap().text().starts_with("unsaved"));
        focus.panel = PanelFocus::Editor;
        assert_eq!(focus.input_target(None), InputTarget::NoDocument);
    }

    #[test]
    fn nested_modal_targets_and_return_focus_survive_activation_changes() {
        let (_root, mut documents, a, b) = documents();
        documents.activate(a).unwrap();
        let mut focus = FocusState {
            panel: PanelFocus::Editor,
            ..FocusState::default()
        };
        focus
            .open_overlay(InputOverlay::Dialog(DialogKind::SaveConfirm), Some(a))
            .unwrap();
        documents.activate(b).unwrap();
        assert_eq!(focus.overlay_document(), Some(a));
        assert_eq!(focus.input_target(Some(b)), InputTarget::Overlay);
        focus.panel = PanelFocus::Terminal;
        focus
            .open_overlay(InputOverlay::CopyOverlay, Some(b))
            .unwrap();
        let copy_return = focus.dismiss_overlay().unwrap();
        assert_eq!(copy_return.document, Some(b));
        assert_eq!(focus.panel, PanelFocus::Terminal);
        assert_eq!(focus.overlay_document(), Some(a));
        assert_eq!(focus.overlay, InputOverlay::Dialog(DialogKind::SaveConfirm));
        let dialog_return = focus.dismiss_overlay().unwrap();
        documents.activate(dialog_return.document.unwrap()).unwrap();
        assert_eq!(documents.active_id(), Some(a));
        assert_eq!(focus.panel, PanelFocus::Editor);
        assert_eq!(focus.overlay, InputOverlay::Normal);
        assert_eq!(focus.overlay_document(), None);
        assert_eq!(focus.dismiss_overlay(), None);
    }

    #[test]
    fn overlay_depth_is_bounded_without_losing_return_context() {
        let (_root, _documents, a, _b) = documents();
        let mut focus = FocusState::default();
        for _ in 0..8 {
            focus.open_overlay(InputOverlay::Help, Some(a)).unwrap();
        }
        assert_eq!(
            focus.open_overlay(InputOverlay::CopyOverlay, None),
            Err(OverlayLimit)
        );
        assert_eq!(focus.overlay, InputOverlay::Help);
        assert_eq!(focus.overlay_document(), Some(a));
        for _ in 0..8 {
            assert!(focus.dismiss_overlay().is_some());
        }
        assert_eq!(focus.overlay, InputOverlay::Normal);
        assert!(focus.dismiss_overlay().is_none());
    }

    #[test]
    fn round1_progress_retirement_preserves_each_other_nested_origin() {
        let (_root, _documents, a, b) = documents();
        let mut focus = FocusState {
            panel: PanelFocus::Editor,
            ..FocusState::default()
        };
        focus
            .open_overlay(InputOverlay::Dialog(DialogKind::SaveConfirm), Some(a))
            .unwrap();
        focus.panel = PanelFocus::Tree;
        focus
            .open_overlay(
                InputOverlay::Dialog(DialogKind::Progress {
                    message: "Pasting".into(),
                    current: 0,
                    total: 1,
                }),
                Some(b),
            )
            .unwrap();
        focus.panel = PanelFocus::Terminal;
        focus
            .open_overlay(InputOverlay::CopyOverlay, Some(b))
            .unwrap();
        focus.panel = PanelFocus::Preview;
        focus.open_overlay(InputOverlay::Help, Some(a)).unwrap();
        assert!(focus.retire_progress().is_none());
        assert_eq!(focus.overlay, InputOverlay::Help);
        assert_eq!(focus.overlay_document(), Some(a));
        assert_eq!(focus.dismiss_overlay().unwrap().document, Some(a));
        assert_eq!(focus.overlay, InputOverlay::CopyOverlay);
        assert_eq!(focus.overlay_document(), Some(b));
        assert_eq!(focus.panel, PanelFocus::Preview);
        assert_eq!(focus.dismiss_overlay().unwrap().document, Some(b));
        assert_eq!(focus.overlay, InputOverlay::Dialog(DialogKind::SaveConfirm));
        assert_eq!(focus.overlay_document(), Some(a));
        assert_eq!(focus.panel, PanelFocus::Tree);
        assert_eq!(focus.dismiss_overlay().unwrap().document, Some(a));
        assert_eq!(focus.panel, PanelFocus::Editor);
        assert_eq!(focus.overlay, InputOverlay::Normal);
        assert!(focus.dismiss_overlay().is_none());
    }

    #[test]
    fn round1_unframed_progress_retirement_is_safe_and_idempotent() {
        let mut focus = FocusState {
            overlay: InputOverlay::Dialog(DialogKind::Progress {
                message: "legacy fixture".into(),
                current: 0,
                total: 1,
            }),
            ..Default::default()
        };
        assert!(focus.retire_progress().is_none());
        assert_eq!(focus.overlay, InputOverlay::Normal);
        assert_eq!(focus.overlay_document(), None);
        assert!(focus.retire_progress().is_none());
        focus.overlay = InputOverlay::Help;
        assert!(focus.retire_progress().is_none());
        assert_eq!(focus.overlay, InputOverlay::Help);
    }
}
