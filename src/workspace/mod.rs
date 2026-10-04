//! Stable document ownership, independent of application focus and previews.
#[allow(dead_code)] // Public integration surface is consumed in the next phase.
pub mod documents;
pub mod focus;
pub mod layout;

/// Retained documents and independent panel/modal input contexts.
#[derive(Debug, Default)]
pub struct Workspace {
    pub documents: documents::DocumentStore,
    pub focus: focus::FocusState,
    pub layout: layout::LayoutState,
}
