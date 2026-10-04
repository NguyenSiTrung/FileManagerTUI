//! Editor documents with stable identity and conservative preview retention.
use crate::editor::EditorState;
use crate::fs::save::{self, SaveError};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Process-unique opaque identity; closed documents never lend their ID to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DocumentId(u64);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// Opening intent; a preview becomes permanently pinned on its first observed edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenDisposition {
    Preview,
    Pinned,
}
/// Bounded most-recently-opened path list retained for session persistence.
pub const MAX_RECENT_FILES: usize = 32;
/// Hard editor admission limits, independent of legacy preview availability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentLimits {
    pub max_bytes: usize,
    pub max_lines: usize,
}
impl Default for DocumentLimits {
    fn default() -> Self {
        Self {
            max_bytes: 10 * 1024 * 1024,
            max_lines: 100_000,
        }
    }
}
/// Actionable admission/lifecycle failures; opening never writes to disk.
#[derive(Debug, thiserror::Error)]
pub enum DocumentError {
    #[error("unknown document: {0:?}")]
    Unknown(DocumentId),
    #[error("document {0:?} has unsaved changes; save or explicitly discard before closing")]
    Dirty(DocumentId),
    #[error("remote document {0} is preview-only; copy locally to edit")]
    Remote(PathBuf),
    #[error("binary document {0} is preview-only")]
    Binary(PathBuf),
    #[error("document {0} is not UTF-8; convert explicitly before editing")]
    InvalidUtf8(PathBuf),
    #[error("document {path} exceeds {max_lines} lines; use the large-file preview")]
    TooManyLines { path: PathBuf, max_lines: usize },
    #[error("read-only document {0}; copy to a writable location to edit")]
    ReadOnly(PathBuf),
    #[error("document identity space exhausted")]
    IdentityExhausted,
    #[error("destination is owned by another open document: {0}")]
    Ownership(PathBuf),
    #[error(transparent)]
    Load(#[from] SaveError),
}
/// Explicit, sticky disk notification. Getters never inspect the filesystem.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum DiskChange {
    #[default]
    Unchanged,
    Changed,
    Deleted,
}
/// Sole owner of one editor and its stable opening path (not a transient preview).
#[derive(Debug)]
pub struct Document {
    pub editor: EditorState,
    id: DocumentId,
    path: PathBuf,
    pinned: Cell<bool>,
    opening_content_revision: u64,
    disk_change: DiskChange,
}
impl Document {
    /// Stable identity across activation and Save As.
    pub fn id(&self) -> DocumentId {
        self.id
    }
    /// Owned path; only successful, admitted Save As/rename can rekey it.
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Stable basename for tabs; unrelated preview changes cannot affect it.
    pub fn title(&self) -> String {
        self.path
            .file_name()
            .unwrap_or(self.path.as_os_str())
            .to_string_lossy()
            .into_owned()
    }
    /// Current logical text; serialization policy remains owned by the editor.
    pub fn text(&self) -> String {
        self.editor.buffer.join("\n")
    }
    /// Sticky first-edit or explicit pin, including edits undone/saved before lookup.
    pub fn is_pinned(&self) -> bool {
        if self.editor.content_revision() != self.opening_content_revision || self.editor.modified {
            self.pinned.set(true);
        }
        self.pinned.get()
    }
    /// In-memory watcher signal only; never polls disk or discards buffer content.
    /// Acknowledged only by explicit reload, confirmed overwrite, or Save As.
    pub fn has_external_change(&self) -> bool {
        self.disk_change != DiskChange::Unchanged
    }
    pub fn disk_change(&self) -> DiskChange {
        self.disk_change
    }
}
/// Owns all editors, retaining their complete local state across focus changes.
/// At most one untouched clean preview is retained. Admission does not promise
/// writable ownership/mounts/ACLs: authoritative platform save checks still apply.
#[derive(Debug, Default)]
pub struct DocumentStore {
    documents: Vec<Document>,
    active: Option<DocumentId>,
    limits: DocumentLimits,
    recent_files: Vec<PathBuf>,
}
impl DocumentStore {
    /// Default 10 MiB / 100,000 line admission limits.
    pub fn new() -> Self {
        Self::default()
    }
    /// Configure hard editor admission limits.
    pub fn with_limits(limits: DocumentLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }
    /// Bounded, de-duplicated most-recently-pinned list, newest first.
    pub fn recent_files(&self) -> &[PathBuf] {
        &self.recent_files
    }
    /// Seed the recent list from a restored session (newest first), keeping the
    /// bound and dropping duplicates. Never trusts the input order beyond its
    /// documented newest-first contract and never grows past `MAX_RECENT_FILES`.
    pub fn seed_recent_files(&mut self, files: Vec<PathBuf>) {
        for path in files.into_iter().rev() {
            self.note_recent(&path);
        }
    }
    fn note_recent(&mut self, path: &Path) {
        self.recent_files.retain(|existing| existing != path);
        self.recent_files.insert(0, path.to_path_buf());
        self.recent_files.truncate(MAX_RECENT_FILES);
    }
    fn observe(&self) {
        for d in &self.documents {
            d.is_pinned();
        }
    }
    /// Observe sticky edits for every document under a shared borrow. Session
    /// capture needs this before it filters on `is_pinned`, because a caller can
    /// edit an inactive preview directly through `get_mut`.
    pub fn observe_edits(&self) {
        self.observe();
    }
    /// Open or activate a canonical lexical alias; symlink paths/ancestors are refused.
    /// Failed admission preserves all existing documents and active focus.
    pub fn open(
        &mut self,
        path: &Path,
        disposition: OpenDisposition,
    ) -> Result<DocumentId, DocumentError> {
        self.open_with_loader(path, disposition, save::load_document_bounded)
    }
    fn open_with_loader(
        &mut self,
        path: &Path,
        disposition: OpenDisposition,
        load: impl FnOnce(&Path, usize) -> Result<(Vec<u8>, save::FileRevision), SaveError>,
    ) -> Result<DocumentId, DocumentError> {
        self.observe();
        if path.to_string_lossy().starts_with("s3:") {
            return Err(DocumentError::Remote(path.into()));
        }
        // Inspect the original spelling before canonicalization: collapsing `..`
        // first would hide a symlink ancestor traversed by the caller.
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|source| SaveError::Io {
                    operation: "resolve",
                    path: path.into(),
                    source,
                })?
                .join(path)
        };
        let mut prefix = PathBuf::new();
        for part in absolute.components() {
            prefix.push(part);
            let metadata = std::fs::symlink_metadata(&prefix).map_err(|source| SaveError::Io {
                operation: "inspect",
                path: prefix.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() {
                return Err(SaveError::UnsafeTarget {
                    path: path.into(),
                    reason: "symlinks are not supported; open the real file path",
                }
                .into());
            }
        }
        let canonical = std::fs::canonicalize(&absolute).map_err(|source| SaveError::Io {
            operation: "resolve",
            path: path.into(),
            source,
        })?;
        if let Some(id) = self
            .documents
            .iter()
            .find(|d| d.path == canonical)
            .map(|d| d.id)
        {
            if disposition == OpenDisposition::Pinned {
                self.pin(id)?;
                self.note_recent(&canonical);
            }
            self.activate(id)?;
            return Ok(id);
        }
        let metadata = std::fs::metadata(&absolute).map_err(|source| SaveError::Io {
            operation: "inspect",
            path: path.into(),
            source,
        })?;
        let readonly = metadata.permissions().readonly();
        #[cfg(unix)]
        let readonly = {
            use std::os::unix::fs::MetadataExt;
            readonly || metadata.mode() & 0o200 == 0
        };
        if readonly {
            return Err(DocumentError::ReadOnly(path.into()));
        }
        let (bytes, revision) = load(&absolute, self.limits.max_bytes)?;
        if bytes.contains(&0)
            || bytes
                .iter()
                .any(|b| *b < 0x20 && !matches!(*b, b'\t' | b'\r' | b'\n'))
        {
            return Err(DocumentError::Binary(path.into()));
        }
        let content =
            std::str::from_utf8(&bytes).map_err(|_| DocumentError::InvalidUtf8(path.into()))?;
        let lines = if content.is_empty() {
            1
        } else {
            content.lines().count() + usize::from(content.ends_with('\n'))
        };
        if lines > self.limits.max_lines {
            return Err(DocumentError::TooManyLines {
                path: path.into(),
                max_lines: self.limits.max_lines,
            });
        }
        let id = DocumentId(
            NEXT_ID
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                .map_err(|_| DocumentError::IdentityExhausted)?,
        );
        let mut editor = EditorState::new(content, canonical.clone());
        editor.source_revision = Some(revision);
        let opening_content_revision = editor.content_revision();
        self.documents
            .retain(|d| d.is_pinned() || d.editor.modified || d.has_external_change());
        if disposition == OpenDisposition::Pinned {
            self.note_recent(&canonical);
        }
        self.documents.push(Document {
            editor,
            id,
            path: canonical,
            pinned: Cell::new(disposition == OpenDisposition::Pinned),
            opening_content_revision,
            disk_change: DiskChange::Unchanged,
        });
        self.active = Some(id);
        Ok(id)
    }
    /// Activate without resetting any editor state.
    pub fn activate(&mut self, id: DocumentId) -> Result<(), DocumentError> {
        self.observe();
        if self.get(id).is_none() {
            return Err(DocumentError::Unknown(id));
        }
        self.active = Some(id);
        Ok(())
    }
    /// Look up a document, observing sticky edits throughout the store.
    pub fn get(&self, id: DocumentId) -> Option<&Document> {
        self.observe();
        self.documents.iter().find(|d| d.id == id)
    }
    /// Borrow the owned editor; edits are detected by revision on the next store operation.
    pub fn get_mut(&mut self, id: DocumentId) -> Option<&mut Document> {
        self.observe();
        self.documents.iter_mut().find(|d| d.id == id)
    }
    /// Active identity, independent of UI focus.
    pub fn active_id(&self) -> Option<DocumentId> {
        self.observe();
        self.active
    }
    /// Active document, if any.
    pub fn active(&self) -> Option<&Document> {
        self.observe();
        self.active.and_then(|id| self.get(id))
    }
    /// Borrow the active owned document without borrowing the whole application.
    pub fn active_mut(&mut self) -> Option<&mut Document> {
        let id = self.active?;
        self.get_mut(id)
    }
    /// Update admission limits; existing buffers are never evicted.
    pub fn set_limits(&mut self, limits: DocumentLimits) {
        self.limits = limits;
    }
    /// Explicit discard/reload through the same bounded admission policy.
    /// A failed load leaves the current owned editor untouched.
    pub fn reload_active(&mut self) -> Result<(), DocumentError> {
        let Some(id) = self.active_id() else {
            return Ok(());
        };
        self.reload(id)
    }
    /// Reload the captured modal origin without changing active document identity.
    pub fn reload(&mut self, id: DocumentId) -> Result<(), DocumentError> {
        let path = self
            .get(id)
            .ok_or(DocumentError::Unknown(id))?
            .editor
            .file_path
            .clone();
        let mut admission = Self::with_limits(self.limits);
        let loaded = admission.open(&path, OpenDisposition::Pinned)?;
        let editor = admission.documents.remove(0).editor;
        debug_assert_eq!(loaded, admission.active.unwrap());
        self.get_mut(id).unwrap().editor = editor;
        self.acknowledge_disk_change(id)?;
        Ok(())
    }
    /// Permanently retain a preview even when clean.
    pub fn pin(&mut self, id: DocumentId) -> Result<(), DocumentError> {
        self.get(id)
            .ok_or(DocumentError::Unknown(id))?
            .pinned
            .set(true);
        Ok(())
    }
    /// Ordered documents, observing edits before exposure.
    pub fn iter(&self) -> impl Iterator<Item = &Document> {
        self.observe();
        self.documents.iter()
    }
    /// Number of retained documents.
    pub fn len(&self) -> usize {
        self.observe();
        self.documents.len()
    }
    /// Whether the workspace contains no documents.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Close only clean documents; dirty buffers require a later explicit discard flow.
    pub fn close(&mut self, id: DocumentId) -> Result<(), DocumentError> {
        self.observe();
        let index = self
            .documents
            .iter()
            .position(|d| d.id == id)
            .ok_or(DocumentError::Unknown(id))?;
        if self.documents[index].editor.modified {
            return Err(DocumentError::Dirty(id));
        }
        self.remove(index, id);
        Ok(())
    }
    /// Only an explicit document-scoped Discard decision may bypass dirty guard.
    pub fn discard_and_close(&mut self, id: DocumentId) -> Result<(), DocumentError> {
        let index = self
            .documents
            .iter()
            .position(|d| d.id == id)
            .ok_or(DocumentError::Unknown(id))?;
        self.remove(index, id);
        Ok(())
    }
    fn remove(&mut self, index: usize, id: DocumentId) {
        self.documents.remove(index);
        if self.active == Some(id) {
            self.active = self
                .documents
                .get(index.min(self.documents.len().saturating_sub(1)))
                .map(|d| d.id);
        }
    }
    /// Feed a watcher change/deletion notification without touching buffer or load revision.
    /// Retains even a clean preview until explicitly closed; no implicit reload/acknowledgment.
    pub fn mark_external_change(&mut self, id: DocumentId) -> Result<(), DocumentError> {
        let d = self.get_mut(id).ok_or(DocumentError::Unknown(id))?;
        d.disk_change = DiskChange::Changed;
        d.pinned.set(true);
        Ok(())
    }
    /// Successful explicit disk decisions acknowledge the captured document only.
    pub fn acknowledge_disk_change(&mut self, id: DocumentId) -> Result<(), DocumentError> {
        self.get_mut(id)
            .ok_or(DocumentError::Unknown(id))?
            .disk_change = DiskChange::Unchanged;
        Ok(())
    }
    /// Symlink-free absolute identity, including a missing leaf owned by a buffer.
    fn identity_path(path: &Path) -> Result<PathBuf, DocumentError> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|source| SaveError::Io {
                    operation: "resolve",
                    path: path.into(),
                    source,
                })?
                .join(path)
        };
        let mut prefix = PathBuf::new();
        for part in absolute.components() {
            prefix.push(part);
            match std::fs::symlink_metadata(&prefix) {
                Ok(m) if m.file_type().is_symlink() => {
                    return Err(SaveError::UnsafeTarget {
                        path: path.into(),
                        reason: "symlinks are not supported",
                    }
                    .into())
                }
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(source) => {
                    return Err(SaveError::Io {
                        operation: "inspect",
                        path: prefix,
                        source,
                    }
                    .into())
                }
            }
        }
        // Only collapse lexical aliases after checking original symlink spelling.
        let mut normalized = PathBuf::new();
        for part in absolute.components() {
            match part {
                std::path::Component::CurDir => (),
                std::path::Component::ParentDir => {
                    normalized.pop();
                }
                _ => normalized.push(part),
            }
        }
        // Iteratively resolve the nearest existing ancestor. Missing directory
        // chains must not turn attacker-controlled input into recursive calls.
        let mut ancestor = normalized;
        let mut missing = Vec::new();
        loop {
            match std::fs::canonicalize(&ancestor) {
                Ok(mut resolved) => {
                    for component in missing.into_iter().rev() {
                        resolved.push(component);
                    }
                    return Ok(resolved);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(
                        ancestor
                            .file_name()
                            .ok_or_else(|| SaveError::UnsafeTarget {
                                path: path.into(),
                                reason: "destination has no existing ancestor",
                            })?
                            .to_os_string(),
                    );
                    ancestor.pop();
                }
                Err(source) => {
                    return Err(SaveError::Io {
                        operation: "resolve",
                        path: ancestor,
                        source,
                    }
                    .into())
                }
            }
        }
    }
    /// Admission is before any disk write. Missing paths still belong to their buffers.
    pub fn preflight_save_as(&self, id: DocumentId, path: &Path) -> Result<PathBuf, DocumentError> {
        self.get(id).ok_or(DocumentError::Unknown(id))?;
        let path = Self::identity_path(path)?;
        if self.documents.iter().any(|d| d.id != id && d.path == path) {
            return Err(DocumentError::Ownership(path));
        }
        Ok(path)
    }
    /// Infallible commit of a preflighted identity during the same exclusive borrow.
    pub fn commit_save_as(&mut self, id: DocumentId, path: PathBuf) {
        let d = self
            .get_mut(id)
            .expect("preflighted document retained during save");
        d.path = path.clone();
        d.editor.file_path = path;
        d.disk_change = DiskChange::Unchanged;
        d.pinned.set(true);
    }
    /// Capture folder-prefix ownership updates before the filesystem operation.
    pub fn preflight_rename(
        &self,
        from: &Path,
        to: &Path,
    ) -> Result<Vec<(DocumentId, PathBuf)>, DocumentError> {
        let from = Self::identity_path(from)?;
        let to = Self::identity_path(to)?;
        // Conservatively reserve the whole destination subtree, including
        // missing buffers whose on-disk counterparts are not open in the source.
        if let Some(d) = self.iter().find(|d| {
            !d.path.starts_with(&from) && (d.path.starts_with(&to) || to.starts_with(&d.path))
        }) {
            return Err(DocumentError::Ownership(d.path.clone()));
        }
        let changes: Vec<_> = self
            .iter()
            .filter_map(|d| {
                d.path.strip_prefix(&from).ok().map(|suffix| {
                    (
                        d.id,
                        if suffix.as_os_str().is_empty() {
                            to.clone()
                        } else {
                            to.join(suffix)
                        },
                    )
                })
            })
            .collect();
        Ok(changes)
    }
    /// Reconcile only a successful known rename. Never acknowledge an existing conflict.
    pub fn commit_rename(&mut self, changes: Vec<(DocumentId, PathBuf)>) {
        let limit = self.limits.max_bytes;
        for (id, path) in changes {
            let Some(d) = self.get_mut(id) else { continue };
            d.path = path.clone();
            d.editor.file_path = path.clone();
            let reconciled = if !d.has_external_change() {
                save::load_document_bounded(&path, limit)
                    .ok()
                    .and_then(|(_, revision)| {
                        d.editor
                            .source_revision
                            .as_ref()
                            .filter(|old| old.matches_after_known_rename(&revision))
                            .map(|_| revision)
                    })
            } else {
                None
            };
            if let Some(revision) = reconciled {
                d.editor.source_revision = Some(revision);
            } else if d.disk_change == DiskChange::Unchanged {
                d.disk_change = DiskChange::Changed;
            }
            d.pinned.set(true);
        }
    }
    /// A successful owned deletion keeps every buffer and its original revision.
    pub fn mark_deleted_path(&mut self, path: &Path) {
        for d in &mut self.documents {
            if d.path.starts_with(path) {
                d.disk_change = DiskChange::Deleted;
                d.pinned.set(true);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn file(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
    #[test]
    fn recent_files_are_bounded_deduplicated_and_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = DocumentStore::new();
        let a = file(&dir, "a", b"a");
        let b = file(&dir, "b", b"b");
        s.open(&a, OpenDisposition::Pinned).unwrap();
        s.open(&b, OpenDisposition::Pinned).unwrap();
        s.open(&a, OpenDisposition::Pinned).unwrap();
        let recent = s.recent_files();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0], a.canonicalize().unwrap());
        assert_eq!(recent[1], b.canonicalize().unwrap());

        // A preview does not pollute the recent list.
        let c = file(&dir, "c", b"c");
        s.open(&c, OpenDisposition::Preview).unwrap();
        assert_eq!(s.recent_files().len(), 2);

        // Explicit seeding is de-duplicated, capped, and keeps newest-first.
        s.seed_recent_files((0..40).map(|n| dir.path().join(format!("n{n}"))).collect());
        assert_eq!(s.recent_files().len(), MAX_RECENT_FILES);
        assert_eq!(s.recent_files()[0], dir.path().join("n0"));
        s.seed_recent_files(vec![a.clone()]);
        assert_eq!(s.recent_files().len(), MAX_RECENT_FILES);
        assert_eq!(s.recent_files()[0], a);
    }
    #[test]
    fn document_lifecycle_rename_external_bytes_never_refreshes_original_revision() {
        let dir = tempfile::tempdir().unwrap();
        let original = file(&dir, "a", b"original");
        let mut s = DocumentStore::new();
        let id = s.open(&original, OpenDisposition::Pinned).unwrap();
        s.get_mut(id).unwrap().editor.insert_char('x');
        let baseline = s.get(id).unwrap().editor.source_revision.clone();
        let destination = dir.path().join("b");
        let changes = s.preflight_rename(&original, &destination).unwrap();
        fs::write(&original, "external").unwrap();
        fs::rename(&original, &destination).unwrap();
        s.commit_rename(changes);
        assert_eq!(s.get(id).unwrap().path(), destination);
        assert_eq!(s.get(id).unwrap().editor.source_revision, baseline);
        assert_eq!(s.get(id).unwrap().disk_change(), DiskChange::Changed);
        assert_eq!(s.get(id).unwrap().text(), "xoriginal");
        assert!(s.get(id).unwrap().editor.modified);
        assert_eq!(s.open(&destination, OpenDisposition::Pinned).unwrap(), id);
        assert!(s.discard_and_close(DocumentId(u64::MAX)).is_err());
        assert!(s.acknowledge_disk_change(DocumentId(u64::MAX)).is_err());
        assert!(s
            .preflight_save_as(DocumentId(u64::MAX), &destination)
            .is_err());
    }

    #[test]
    fn document_lifecycle_missing_owned_paths_deduplicate_lexical_save_as_admission() {
        let dir = tempfile::tempdir().unwrap();
        let original = file(&dir, "a", b"original");
        let owned = file(&dir, "b", b"owned");
        let mut s = DocumentStore::new();
        let a = s.open(&original, OpenDisposition::Pinned).unwrap();
        let b = s.open(&owned, OpenDisposition::Pinned).unwrap();
        fs::remove_file(&owned).unwrap();
        for path in [dir.path().join("./b"), dir.path().join("missing/../b")] {
            assert!(matches!(
                s.preflight_save_as(a, &path),
                Err(DocumentError::Ownership(_))
            ));
        }
        assert_eq!(
            s.preflight_save_as(a, &dir.path().join("missing/../new"))
                .unwrap(),
            dir.path().join("new")
        );
        s.mark_deleted_path(&owned);
        assert_eq!(s.get(b).unwrap().disk_change(), DiskChange::Deleted);
        assert_eq!(s.get(a).unwrap().disk_change(), DiskChange::Unchanged);
        s.discard_and_close(b).unwrap();
        assert_eq!(s.active_id(), Some(a));
        assert!(s.preflight_save_as(a, &owned).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn document_lifecycle_save_as_and_rename_refuse_symlink_aliases_before_write() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let original = file(&dir, "a", b"original");
        let mut s = DocumentStore::new();
        let id = s.open(&original, OpenDisposition::Pinned).unwrap();
        let link = dir.path().join("link");
        symlink(dir.path(), &link).unwrap();
        for path in [link.join("new"), link.join("../new")] {
            assert!(s.preflight_save_as(id, &path).is_err());
            assert!(s.preflight_rename(&original, &path).is_err());
        }
        let dangling = dir.path().join("dangling");
        symlink(dir.path().join("missing"), &dangling).unwrap();
        assert!(s.preflight_save_as(id, &dangling).is_err());
        assert_eq!(fs::read(&original).unwrap(), b"original");
        assert_eq!(s.get(id).unwrap().path(), original);
    }

    #[test]
    fn deduplicates_aliases_and_keeps_stable_titles() {
        let d = tempfile::tempdir().unwrap();
        let p = file(&d, "a", b"a");
        let mut s = DocumentStore::new();
        let id = s.open(&p, OpenDisposition::Preview).unwrap();
        assert_eq!(
            s.open(&d.path().join("./a"), OpenDisposition::Pinned)
                .unwrap(),
            id
        );
        assert!(s.get(id).unwrap().is_pinned());
        assert_eq!(s.get(id).unwrap().path(), p.canonicalize().unwrap());
        assert_eq!(s.get(id).unwrap().title(), "a");
        assert_eq!(s.len(), 1);
    }
    #[test]
    fn first_edit_pins_even_after_undo_or_save_before_observation() {
        for save in [false, cfg!(any(target_os = "linux", target_os = "macos"))] {
            let d = tempfile::tempdir().unwrap();
            let p = file(&d, "a", b"a");
            let mut s = DocumentStore::new();
            let id = s.open(&p, OpenDisposition::Preview).unwrap();
            let e = &mut s.get_mut(id).unwrap().editor;
            e.insert_text("x").unwrap();
            if save {
                e.save().unwrap();
            } else {
                e.undo();
            }
            assert!(!e.modified);
            assert!(s.get(id).unwrap().is_pinned());
            let q = file(&d, "b", b"b");
            s.open(&q, OpenDisposition::Preview).unwrap();
            assert!(s.get(id).is_some());
        }
    }
    #[test]
    fn preview_replacement_bounded_and_ids_never_reused() {
        let d = tempfile::tempdir().unwrap();
        let mut s = DocumentStore::new();
        let mut last = None;
        for n in 0..12 {
            let p = file(&d, &n.to_string(), b"a");
            let id = s.open(&p, OpenDisposition::Preview).unwrap();
            if let Some(old) = last {
                assert_ne!(id, old);
                assert!(s.get(old).is_none());
            }
            last = Some(id);
            assert_eq!(s.len(), 1);
        }
        let id = last.unwrap();
        s.close(id).unwrap();
        assert!(s.is_empty());
        assert_eq!(s.active_id(), None);
        let mut other = DocumentStore::new();
        assert_ne!(
            other
                .open(&d.path().join("0"), OpenDisposition::Pinned)
                .unwrap(),
            id
        );
    }
    #[test]
    fn dirty_retention_close_guard_and_invalid_ids() {
        let d = tempfile::tempdir().unwrap();
        let p = file(&d, "a", b"a");
        let mut s = DocumentStore::new();
        let id = s.open(&p, OpenDisposition::Preview).unwrap();
        s.get_mut(id).unwrap().editor.insert_char('x');
        assert!(matches!(s.close(id), Err(DocumentError::Dirty(_))));
        let q = file(&d, "b", b"b");
        let second = s.open(&q, OpenDisposition::Preview).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s.iter().count(), 2);
        assert!(s.active().is_some());
        let unknown = DocumentId(u64::MAX);
        assert!(s.activate(unknown).is_err());
        assert_eq!(s.active_id(), Some(second));
        assert!(s.get_mut(unknown).is_none());
        assert!(s.pin(unknown).is_err());
        assert!(s.close(unknown).is_err());
        s.pin(second).unwrap();
        s.close(second).unwrap();
        assert_eq!(s.active_id(), Some(id));
    }
    #[test]
    fn document_local_state_survives_activation() {
        let d = tempfile::tempdir().unwrap();
        let a = file(&d, "a", b"one\ntwo\nthree");
        let b = file(&d, "b", b"other");
        let mut s = DocumentStore::new();
        let id = s.open(&a, OpenDisposition::Pinned).unwrap();
        let e = &mut s.get_mut(id).unwrap().editor;
        e.insert_text("x").unwrap();
        e.copy_line();
        e.cursor_line = 1;
        e.cursor_col = 2;
        e.scroll_offset = 1;
        e.horizontal_offset = 3;
        e.line_wrap = true;
        e.selection = Some(crate::editor::Selection::new(0, 1));
        e.find_state.query = "two".into();
        e.find_state.active = true;
        let rev = e.source_revision.clone();
        let history = e.undo_stack.len();
        let clipboard = e.clipboard_text();
        let second = s.open(&b, OpenDisposition::Pinned).unwrap();
        s.activate(second).unwrap();
        s.activate(id).unwrap();
        let e = &s.get(id).unwrap().editor;
        assert_eq!(
            (
                e.cursor_line,
                e.cursor_col,
                e.scroll_offset,
                e.horizontal_offset
            ),
            (1, 2, 1, 3)
        );
        assert!(e.line_wrap);
        assert!(e.selection.is_some());
        assert!(e.find_state.active);
        assert_eq!(e.find_state.query, "two");
        assert_eq!(e.source_revision, rev);
        assert_eq!(e.undo_stack.len(), history);
        assert_eq!(e.clipboard_text(), clipboard);
        assert!(e.modified);
    }
    #[test]
    fn typed_content_limits_and_rejections_leave_preview_intact() {
        let d = tempfile::tempdir().unwrap();
        let mut s = DocumentStore::with_limits(DocumentLimits {
            max_bytes: 4,
            max_lines: 2,
        });
        let a = file(&d, "a", b"abcd");
        let id = s.open(&a, OpenDisposition::Preview).unwrap();
        for (name, bytes) in [
            ("binary", &b"a\0"[..]),
            ("utf8", &b"\xff"[..]),
            ("large", &b"abcde"[..]),
            ("lines", &b"a\nb\n"[..]),
        ] {
            let p = file(&d, name, bytes);
            let error = s.open(&p, OpenDisposition::Preview).unwrap_err();
            match name {
                "binary" => assert!(matches!(error, DocumentError::Binary(_))),
                "utf8" => assert!(matches!(error, DocumentError::InvalidUtf8(_))),
                "large" => assert!(matches!(
                    error,
                    DocumentError::Load(crate::fs::save::SaveError::TooLarge { .. })
                )),
                _ => assert!(matches!(error, DocumentError::TooManyLines { .. })),
            }
            assert_eq!(fs::read(p).unwrap(), bytes);
            assert_eq!(s.active_id(), Some(id));
        }
        assert!(s.open(d.path(), OpenDisposition::Pinned).is_err());
        assert!(matches!(
            s.open(
                std::path::Path::new("s3://bucket/key"),
                OpenDisposition::Pinned
            ),
            Err(DocumentError::Remote(_))
        ));
        assert!(s
            .open(&d.path().join("missing"), OpenDisposition::Pinned)
            .is_err());
        assert_eq!(
            DocumentLimits::default(),
            DocumentLimits {
                max_bytes: 10 * 1024 * 1024,
                max_lines: 100_000
            }
        );
    }
    #[test]
    fn external_mark_is_explicit_sticky_and_preserves_buffer_and_revision() {
        let d = tempfile::tempdir().unwrap();
        let p = file(&d, "a", b"a");
        let mut s = DocumentStore::new();
        let id = s.open(&p, OpenDisposition::Preview).unwrap();
        let revision = s.get(id).unwrap().editor.source_revision.clone();
        fs::remove_file(&p).unwrap();
        assert!(!s.get(id).unwrap().has_external_change());
        s.mark_external_change(id).unwrap();
        assert!(s.get(id).unwrap().has_external_change());
        assert_eq!(s.get(id).unwrap().text(), "a");
        assert_eq!(s.get(id).unwrap().editor.source_revision, revision);
        let q = file(&d, "b", b"b");
        s.open(&q, OpenDisposition::Preview).unwrap();
        assert!(s.get(id).is_some());
        assert!(s.mark_external_change(DocumentId(u64::MAX)).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn refuses_symlink_files_ancestors_and_owner_readonly() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let d = tempfile::tempdir().unwrap();
        let p = file(&d, "a", b"a");
        let mut s = DocumentStore::new();
        let id = s.open(&p, OpenDisposition::Pinned).unwrap();
        let link = d.path().join("link");
        symlink(&p, &link).unwrap();
        assert!(s.open(&link, OpenDisposition::Pinned).is_err());
        let ancestor = d.path().join("dirlink");
        symlink(d.path(), &ancestor).unwrap();
        assert!(s
            .open(&ancestor.join("a"), OpenDisposition::Pinned)
            .is_err());
        assert!(s
            .open(&ancestor.join("../a"), OpenDisposition::Pinned)
            .is_err());
        assert_eq!(s.active_id(), Some(id));
        for mode in [0o444, 0o460] {
            let q = file(&d, "readonly", b"keep");
            fs::set_permissions(&q, fs::Permissions::from_mode(mode)).unwrap();
            assert!(matches!(
                s.open(&q, OpenDisposition::Pinned),
                Err(DocumentError::ReadOnly(_))
            ));
            assert_eq!(fs::read(q).unwrap(), b"keep");
        }
    }
    #[test]
    fn injected_loader_failure_is_bounded_and_non_destructive() {
        let d = tempfile::tempdir().unwrap();
        let p = file(&d, "a", b"a");
        let q = file(&d, "b", b"b");
        let mut s = DocumentStore::with_limits(DocumentLimits {
            max_bytes: 7,
            max_lines: 3,
        });
        let id = s.open(&p, OpenDisposition::Preview).unwrap();
        let result = s.open_with_loader(&q, OpenDisposition::Preview, |path, limit| {
            assert_eq!(limit, 7);
            assert_eq!(path, q);
            Err(SaveError::Conflict(path.into()))
        });
        assert!(matches!(
            result,
            Err(DocumentError::Load(SaveError::Conflict(_)))
        ));
        assert_eq!(s.active_id(), Some(id));
        assert_eq!(s.len(), 1);
        assert_eq!(s.get(id).unwrap().id(), id);
    }
    #[test]
    fn empty_and_line_boundaries_and_explicit_pin_persist() {
        let d = tempfile::tempdir().unwrap();
        let mut s = DocumentStore::with_limits(DocumentLimits {
            max_bytes: 4,
            max_lines: 2,
        });
        let p = file(&d, "empty", b"");
        let id = s.open(&p, OpenDisposition::Preview).unwrap();
        assert_eq!(s.get(id).unwrap().text(), "");
        s.pin(id).unwrap();
        let q = file(&d, "two", b"a\nb");
        let second = s.open(&q, OpenDisposition::Pinned).unwrap();
        assert!(s.get(id).unwrap().is_pinned());
        s.close(id).unwrap();
        assert_eq!(s.active_id(), Some(second));
        assert_eq!(s.get(second).unwrap().editor.line_count(), 2);
    }
    #[test]
    fn constructor_retains_crlf_and_normalization_policy() {
        let d = tempfile::tempdir().unwrap();
        let mut s = DocumentStore::new();
        for (name, bytes, normalize) in [
            ("crlf", &b"a\r\n"[..], false),
            ("mixed", &b"a\r\nb\n"[..], true),
        ] {
            let p = file(&d, name, bytes);
            let id = s.open(&p, OpenDisposition::Pinned).unwrap();
            let e = &s.get(id).unwrap().editor;
            assert_eq!(e.line_ending, crate::editor::LineEnding::CrLf);
            assert_eq!(e.normalization_required, normalize);
            assert!(e.source_revision.is_some());
        }
    }
}
