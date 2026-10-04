//! Versioned, workspace-scoped session serialization.
//!
//! A session record persists *paths and view state only*: the workspace root,
//! the open-document paths with their cursors, the pane layout, and the recent
//! file list. It never persists running processes, terminal buffers, executable
//! trust, or secrets, and a record saved for one workspace is never applied to
//! another. State is written atomically (temporary file plus rename) into an
//! injected private directory; unavailable state is a non-fatal, typed failure.

use crate::editor::EditorState;
use crate::workspace::documents::OpenDisposition;
use crate::workspace::layout::LayoutState;
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Wire schema identifier for a session payload. Any other value is refused.
pub const SESSION_SCHEMA: &str = "fm-tui-session";
/// Current schema version. Higher versions are refused as "future".
pub const SESSION_VERSION: u32 = 1;
/// Hard cap on a serialized session payload, validated before parsing.
pub const MAX_SESSION_BYTES: u64 = 1024 * 1024;
/// Hard cap on the number of documents restored from one session.
pub const MAX_SESSION_DOCUMENTS: usize = 256;

/// Disambiguates concurrent temporary state files within one process.
static NEXT_TEMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Persisted absolute pane preferences. Maximization/compact geometry is a
/// transient overlay and is deliberately not part of a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLayout {
    pub explorer_width: u16,
    pub terminal_height: u16,
    pub explorer_visible: bool,
    pub terminal_visible: bool,
}

impl SessionLayout {
    /// Snapshot the durable layout preferences (never the transient maximize).
    pub fn from_layout(layout: &LayoutState) -> Self {
        Self {
            explorer_width: layout.explorer_width(),
            terminal_height: layout.terminal_height(),
            explorer_visible: layout.explorer_visible(),
            terminal_visible: layout.terminal_visible(),
        }
    }

    /// Rebuild a clamping layout state from persisted preferences.
    pub fn into_layout(self) -> LayoutState {
        LayoutState::from_saved(
            self.explorer_width,
            self.explorer_visible,
            self.terminal_height,
            self.terminal_visible,
        )
    }
}

/// One restored document: only the path and its cursor/viewport offsets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSession {
    pub path: PathBuf,
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub scroll_offset: usize,
    pub horizontal_offset: usize,
}

/// A complete, versioned workspace session. Fields are private view state only;
/// there is intentionally no field for processes, terminal content, or trust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub schema: String,
    pub version: u32,
    pub workspace_root: PathBuf,
    pub active_document_path: Option<PathBuf>,
    pub documents: Vec<DocumentSession>,
    pub layout: SessionLayout,
    pub recent_files: Vec<PathBuf>,
}

/// Typed outcome of applying a record; skipped documents (missing/read-only/
/// too large) never abort the restore or the running application.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SessionApply {
    pub opened: usize,
    pub skipped: usize,
    pub active_restored: bool,
}

impl SessionRecord {
    /// Capture the durable view state of `workspace`, keyed to `root`.
    ///
    /// Only *pinned* documents are captured. A transient preview is not durable
    /// state, so recording it would silently promote it to a permanently open
    /// document on the next start. Capturing a preview would also make
    /// `apply_to_workspace` reopen it as `Pinned`, which is the same dishonest
    /// promotion; the two rules are kept symmetric and are pinned by tests.
    pub fn from_workspace(workspace: &Workspace, root: &Path) -> Self {
        // Observe every document first: `is_pinned` records any first edit that
        // happened outside a store operation, so a preview edited directly
        // through `get_mut` is captured as the durable document it now is.
        workspace.documents.observe_edits();
        let active_document_path = workspace
            .documents
            .active()
            .filter(|document| document.is_pinned())
            .map(|document| document.path().to_path_buf());
        let documents = workspace
            .documents
            .iter()
            .filter(|document| document.is_pinned())
            .take(MAX_SESSION_DOCUMENTS)
            .map(|document| DocumentSession {
                path: document.path().to_path_buf(),
                cursor_line: document.editor.cursor_line,
                cursor_col: document.editor.cursor_col,
                scroll_offset: document.editor.scroll_offset,
                horizontal_offset: document.editor.horizontal_offset,
            })
            .collect();
        Self {
            schema: SESSION_SCHEMA.to_string(),
            version: SESSION_VERSION,
            workspace_root: root.to_path_buf(),
            active_document_path,
            documents,
            layout: SessionLayout::from_layout(&workspace.layout),
            recent_files: workspace.documents.recent_files().to_vec(),
        }
    }

    /// Restore documents, cursors and layout into `workspace`.
    pub fn apply_to_workspace(&self, workspace: &mut Workspace) -> SessionApply {
        workspace.layout = self.layout.clone().into_layout();
        workspace
            .documents
            .seed_recent_files(self.recent_files.clone());
        let mut apply = SessionApply::default();
        let mut active_id = None;
        // Truncation rule: even a hand-built record that bypassed `parse_session`
        // can open at most `MAX_SESSION_DOCUMENTS` files at startup. Surplus
        // entries are counted as skipped, never opened.
        apply.skipped = self.documents.len().saturating_sub(MAX_SESSION_DOCUMENTS);
        for saved in self.documents.iter().take(MAX_SESSION_DOCUMENTS) {
            match workspace
                .documents
                .open(&saved.path, OpenDisposition::Pinned)
            {
                Ok(id) => {
                    if let Some(document) = workspace.documents.get_mut(id) {
                        restore_cursor(&mut document.editor, saved);
                    }
                    apply.opened += 1;
                    if self.active_document_path.as_deref() == Some(saved.path.as_path()) {
                        active_id = Some(id);
                    }
                }
                Err(_) => apply.skipped += 1,
            }
        }
        if let Some(id) = active_id {
            if workspace.documents.activate(id).is_ok() {
                apply.active_restored = true;
            }
        }
        apply
    }
}

/// Restore a saved cursor, clamped to the document that actually loaded so a
/// shrunk or changed file cannot place the cursor out of bounds. Viewport
/// offsets are clamped too: a syntactically valid record may carry extreme
/// values, and the render path's viewport math is not overflow-safe on its own.
fn restore_cursor(editor: &mut EditorState, saved: &DocumentSession) {
    let line = saved.cursor_line.min(editor.buffer.len().saturating_sub(1));
    let max_col = editor.buffer.get(line).map(String::len).unwrap_or(0);
    editor.cursor_line = line;
    editor.cursor_col = saved.cursor_col.min(max_col);
    editor.scroll_offset = saved
        .scroll_offset
        .min(editor.buffer.len().saturating_sub(1));
    let display_width = crate::text::byte_to_display_col(
        editor.buffer.get(line).map(String::as_str).unwrap_or(""),
        editor.cursor_col,
        crate::text::TAB_WIDTH,
    );
    editor.horizontal_offset = saved.horizontal_offset.min(display_width.max(1) - 1);
    editor.clamp_viewport();
}

/// Typed, non-fatal session refusal or I/O failure.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session payload is corrupt: {0}")]
    Corrupt(String),
    #[error("session payload is {0} bytes, above the {MAX_SESSION_BYTES}-byte limit")]
    TooLarge(u64),
    #[error("unknown session schema: {0}")]
    UnknownSchema(String),
    #[error("session schema version {0} is newer than the supported {SESSION_VERSION}")]
    FutureVersion(u32),
    #[error("unsupported session schema version {0}")]
    UnsupportedVersion(u32),
    #[error("session carries {0} documents, above the {MAX_SESSION_DOCUMENTS}-document limit")]
    TooManyDocuments(usize),
    #[error("session state is a symlink, refusing to read it: {0}")]
    UnsafeState(PathBuf),
    #[error("session belongs to a different workspace: {0}")]
    WorkspaceMismatch(PathBuf),
    #[error("session state is unavailable: {0}")]
    Unavailable(#[from] std::io::Error),
}

/// A private, injected state directory holding one file per workspace.
#[derive(Debug, Clone)]
pub struct SessionStore {
    directory: PathBuf,
}

impl SessionStore {
    /// Inject the private state directory. No user home directory is read here.
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    /// Deterministic per-workspace state file: different roots never collide.
    pub fn workspace_path(&self, workspace_root: &Path) -> PathBuf {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in workspace_root.to_string_lossy().as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.directory.join(format!("{hash:016x}.json"))
    }

    /// Load the session for `workspace_root`. A missing file is `Ok(None)`.
    pub fn load(&self, workspace_root: &Path) -> Result<Option<SessionRecord>, SessionError> {
        let path = self.workspace_path(workspace_root);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(SessionError::Unavailable(error)),
        };
        if metadata.file_type().is_symlink() {
            return Err(SessionError::UnsafeState(path));
        }
        if metadata.len() > MAX_SESSION_BYTES {
            return Err(SessionError::TooLarge(metadata.len()));
        }
        let mut content = String::new();
        {
            use std::io::Read;
            let file = std::fs::File::open(&path)?;
            file.take(MAX_SESSION_BYTES + 1)
                .read_to_string(&mut content)
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::InvalidData {
                        SessionError::Corrupt(error.to_string())
                    } else {
                        SessionError::Unavailable(error)
                    }
                })?;
        }
        if content.len() as u64 > MAX_SESSION_BYTES {
            return Err(SessionError::TooLarge(content.len() as u64));
        }
        parse_session(workspace_root, &content)
    }

    /// Atomically write `record` to its workspace-keyed state file.
    pub fn save(&self, record: &SessionRecord) -> Result<(), SessionError> {
        if record.schema != SESSION_SCHEMA {
            return Err(SessionError::UnknownSchema(record.schema.clone()));
        }
        if record.version != SESSION_VERSION {
            return Err(SessionError::UnsupportedVersion(record.version));
        }
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|error| SessionError::Corrupt(error.to_string()))?;
        if bytes.len() as u64 > MAX_SESSION_BYTES {
            return Err(SessionError::TooLarge(bytes.len() as u64));
        }
        let path = self.workspace_path(&record.workspace_root);
        std::fs::create_dir_all(&self.directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&self.directory, std::fs::Permissions::from_mode(0o700));
        }
        let temp = self.directory.join(format!(
            ".session-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        match write_atomic(&temp, &path, &bytes) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = std::fs::remove_file(&temp);
                Err(SessionError::Unavailable(error))
            }
        }
    }

    /// Load and apply a stored session in one bounded step. Never fatal: a
    /// corrupt/unavailable record yields a visible notice instead of an error
    /// that would abort startup, and restored documents keep the app usable.
    pub fn restore_into(&self, workspace_root: &Path, workspace: &mut Workspace) -> Option<String> {
        match self.load(workspace_root) {
            Ok(Some(record)) => {
                let outcome = record.apply_to_workspace(workspace);
                (outcome.skipped > 0).then(|| {
                    format!(
                        "Session: restored {} document(s), skipped {}",
                        outcome.opened, outcome.skipped
                    )
                })
            }
            Ok(None) => None,
            Err(error) => Some(format!("Session not restored: {error}")),
        }
    }

    /// Capture and persist the current workspace in one bounded step. Never
    /// fatal: an unwritable state directory yields a notice for stderr.
    pub fn persist_from(&self, workspace_root: &Path, workspace: &Workspace) -> Option<String> {
        let record = SessionRecord::from_workspace(workspace, workspace_root);
        self.save(&record)
            .err()
            .map(|error| format!("workspace session not saved: {error}"))
    }

    /// Build a store from the resolved config directory. Persistence is enabled
    /// but the platform offers no private state directory: the `None` case is a
    /// visible notice, never a silent disable.
    pub fn from_config_dir(directory: Option<PathBuf>) -> (Option<Self>, Option<String>) {
        match directory {
            Some(directory) => (Some(Self::new(directory)), None),
            None => (
                None,
                Some("Session persistence unavailable: no private state directory".to_string()),
            ),
        }
    }
}

/// Validate a decoded payload against the schema, version and workspace key.
fn parse_session(
    workspace_root: &Path,
    content: &str,
) -> Result<Option<SessionRecord>, SessionError> {
    let record: SessionRecord =
        serde_json::from_str(content).map_err(|error| SessionError::Corrupt(error.to_string()))?;
    if record.schema != SESSION_SCHEMA {
        return Err(SessionError::UnknownSchema(record.schema));
    }
    if record.version > SESSION_VERSION {
        return Err(SessionError::FutureVersion(record.version));
    }
    if record.version != SESSION_VERSION {
        return Err(SessionError::UnsupportedVersion(record.version));
    }
    if record.workspace_root != workspace_root {
        return Err(SessionError::WorkspaceMismatch(record.workspace_root));
    }
    if record.documents.len() > MAX_SESSION_DOCUMENTS {
        return Err(SessionError::TooManyDocuments(record.documents.len()));
    }
    Ok(Some(record))
}

/// Write `bytes` to an exclusive temporary file, flush it, then rename it into
/// place. A reader therefore observes either the previous file or the complete
/// new one, never a partial payload.
fn write_atomic(temp: &Path, destination: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temp, destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_workspace(root: &Path) -> Workspace {
        std::fs::write(root.join("a.txt"), "alpha\nbeta\n").unwrap();
        std::fs::write(root.join("b.txt"), "gamma\n").unwrap();
        let mut workspace = Workspace::default();
        let a = workspace
            .documents
            .open(&root.join("a.txt"), OpenDisposition::Pinned)
            .unwrap();
        let _b = workspace
            .documents
            .open(&root.join("b.txt"), OpenDisposition::Pinned)
            .unwrap();
        {
            let document = workspace.documents.get_mut(a).unwrap();
            document.editor.cursor_line = 1;
            document.editor.cursor_col = 2;
            document.editor.scroll_offset = 1;
            document.editor.horizontal_offset = 1;
        }
        workspace.documents.activate(a).unwrap();
        workspace.layout.set_explorer_width(31);
        workspace.layout.set_terminal_height(9);
        workspace.layout.toggle_terminal();
        workspace
    }

    fn minimal_record(root: &Path) -> SessionRecord {
        SessionRecord::from_workspace(&Workspace::default(), root)
    }

    /// The complete set of keys a session payload may contain. Any key outside
    /// the applicable set is a serialization leak (processes, buffers, trust,
    /// secrets, or a new field nobody reviewed) and fails the assertion.
    const ALLOWED_KEYS: &[(&str, &[&str])] = &[
        (
            "$",
            &[
                "schema",
                "version",
                "workspace_root",
                "active_document_path",
                "documents",
                "layout",
                "recent_files",
            ],
        ),
        (
            "documents[]",
            &[
                "path",
                "cursor_line",
                "cursor_col",
                "scroll_offset",
                "horizontal_offset",
            ],
        ),
        (
            "layout",
            &[
                "explorer_width",
                "terminal_height",
                "explorer_visible",
                "terminal_visible",
            ],
        ),
    ];

    fn assert_serialized_keys_are_allowed(value: serde_json::Value) {
        let object = value.as_object().expect("record is a JSON object");
        let keys: Vec<&str> = object.keys().map(String::as_str).collect();
        let allowed = ALLOWED_KEYS
            .iter()
            .find(|(scope, _)| *scope == "$")
            .unwrap()
            .1;
        for key in &keys {
            assert!(
                allowed.contains(key),
                "unexpected top-level session field: {key}"
            );
        }
        assert_eq!(keys.len(), allowed.len(), "top-level field set changed");

        let scope = |name: &str| {
            ALLOWED_KEYS
                .iter()
                .find(|(scope, _)| *scope == name)
                .unwrap()
                .1
        };
        for document in object["documents"].as_array().expect("documents array") {
            for key in document.as_object().unwrap().keys() {
                assert!(
                    scope("documents[]").contains(&key.as_str()),
                    "unexpected document field: {key}"
                );
            }
            assert_eq!(
                document.as_object().unwrap().len(),
                scope("documents[]").len(),
                "document field set changed"
            );
        }
        for key in object["layout"].as_object().unwrap().keys() {
            assert!(
                scope("layout").contains(&key.as_str()),
                "unexpected layout field: {key}"
            );
        }
        assert_eq!(
            object["layout"].as_object().unwrap().len(),
            scope("layout").len(),
            "layout field set changed"
        );
    }

    #[test]
    fn capture_records_only_pinned_documents_and_never_promotes_previews() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("pinned.txt"), "pinned\n").unwrap();
        std::fs::write(root.path().join("preview.txt"), "preview\n").unwrap();
        let mut workspace = Workspace::default();
        let pinned = workspace
            .documents
            .open(&root.path().join("pinned.txt"), OpenDisposition::Pinned)
            .unwrap();
        let preview = workspace
            .documents
            .open(&root.path().join("preview.txt"), OpenDisposition::Preview)
            .unwrap();
        assert!(!workspace.documents.get(preview).unwrap().is_pinned());
        assert!(workspace.documents.get(pinned).unwrap().is_pinned());
        // The transient preview is the active document: the case the review
        // flagged. It must not be captured, and it must not become active.
        assert_eq!(workspace.documents.active_id(), Some(preview));

        // Only the pinned document is captured.
        let record = SessionRecord::from_workspace(&workspace, root.path());
        assert!(!record
            .documents
            .iter()
            .any(|document| document.path == root.path().join("preview.txt")));
        assert_eq!(record.documents.len(), 1);
        // Only the pinned document is captured, so the transient active preview
        // must not be recorded as the active document of the session.
        assert_ne!(
            record.active_document_path,
            Some(root.path().join("preview.txt"))
        );
        assert!(record.layout == SessionLayout::from_layout(&workspace.layout));

        // Pinning the preview (the first-edit rule) makes it durable state, and
        // the pinned document is then captured with its true active path.
        workspace.documents.pin(preview).unwrap();
        workspace.documents.activate(preview).unwrap();
        let record = SessionRecord::from_workspace(&workspace, root.path());
        assert_eq!(
            record.active_document_path,
            Some(root.path().join("preview.txt"))
        );
        assert_eq!(record.documents.len(), 2);
    }

    #[test]
    fn a_preview_only_workspace_captures_no_documents_and_no_active_document() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("preview.txt"), "preview\n").unwrap();
        let mut workspace = Workspace::default();
        let preview = workspace
            .documents
            .open(&root.path().join("preview.txt"), OpenDisposition::Preview)
            .unwrap();
        assert_eq!(workspace.documents.active_id(), Some(preview));

        let record = SessionRecord::from_workspace(&workspace, root.path());
        assert!(record.documents.is_empty());
        assert_eq!(record.active_document_path, None);

        let mut fresh = Workspace::default();
        let outcome = record.apply_to_workspace(&mut fresh);
        assert!(fresh.documents.is_empty());
        assert!(!outcome.active_restored);
        assert_eq!(outcome.opened, 0);
    }

    #[test]
    fn a_missing_private_state_directory_is_a_visible_notice_not_a_silent_disable() {
        let state = tempfile::tempdir().unwrap();
        let (store, notice) = SessionStore::from_config_dir(Some(state.path().to_path_buf()));
        assert!(notice.is_none());
        let store = store.expect("an injected directory yields a store");
        assert_eq!(
            store.workspace_path(Path::new("/workspace")),
            state.path().join(
                SessionStore::new(state.path())
                    .workspace_path(Path::new("/workspace"))
                    .file_name()
                    .unwrap()
            )
        );
        let (store, notice) = SessionStore::from_config_dir(None);
        assert!(store.is_none());
        let notice = notice.expect("persistence-off must be reported");
        assert!(notice.contains("unavailable"), "{notice}");
    }

    #[test]
    fn save_reload_round_trip_preserves_active_document_and_layout() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = fixture_workspace(root.path());
        let saved = SessionRecord::from_workspace(&workspace, root.path());
        let store = SessionStore::new(state.path());
        store.save(&saved).unwrap();
        let serialized_session =
            std::fs::read_to_string(store.workspace_path(root.path())).unwrap();
        assert!(!serialized_session.contains("trusted_server_commands"));
        assert!(!serialized_session.contains("terminal_buffer"));
        // Whitelist form: every serialized key at every level must be one the
        // record is allowed to carry. This fails if any unexpected field is
        // added anywhere in the payload (not just the two substrings above).
        assert_serialized_keys_are_allowed(serde_json::from_str(&serialized_session).unwrap());
        let restored = store.load(root.path()).unwrap().unwrap();
        assert_eq!(restored.active_document_path, saved.active_document_path);
        assert_eq!(restored.layout, saved.layout);
        assert_eq!(restored, saved);
        let mut fresh = Workspace::default();
        let outcome = restored.apply_to_workspace(&mut fresh);
        assert!(outcome.active_restored);
        assert_eq!(outcome.skipped, 0);
        assert_eq!(
            fresh.documents.active().unwrap().path(),
            saved.active_document_path.as_deref().unwrap()
        );
        assert_eq!(fresh.layout.explorer_width(), saved.layout.explorer_width);
        assert_eq!(fresh.layout.terminal_height(), saved.layout.terminal_height);
        assert!(!fresh.layout.terminal_visible());
        let active = fresh.documents.active().unwrap();
        assert_eq!(active.editor.cursor_line, 1);
        assert_eq!(active.editor.cursor_col, 2);
    }

    #[test]
    fn corrupt_truncated_and_empty_payloads_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path());
        let path = store.workspace_path(root.path());
        std::fs::create_dir_all(state.path()).unwrap();
        for payload in [
            "",
            "{ not json",
            "{\"schema\":\"fm-tui-session\",\"version\":1,",
            "\0\0\0",
        ] {
            std::fs::write(&path, payload).unwrap();
            assert!(
                matches!(store.load(root.path()), Err(SessionError::Corrupt(_))),
                "payload {payload:?} must be refused as corrupt"
            );
        }
    }

    #[test]
    fn truncated_valid_record_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path());
        let record = minimal_record(root.path());
        let bytes = serde_json::to_vec(&record).unwrap();
        std::fs::write(store.workspace_path(root.path()), &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(
            store.load(root.path()),
            Err(SessionError::Corrupt(_))
        ));
    }

    #[test]
    fn future_and_unsupported_versions_and_unknown_schema_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path());
        let path = store.workspace_path(root.path());
        std::fs::create_dir_all(state.path()).unwrap();

        let mut record = minimal_record(root.path());
        record.version = SESSION_VERSION + 1;
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(matches!(
            store.load(root.path()),
            Err(SessionError::FutureVersion(v)) if v == SESSION_VERSION + 1
        ));

        record.version = 0;
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(matches!(
            store.load(root.path()),
            Err(SessionError::UnsupportedVersion(0))
        ));

        record.version = SESSION_VERSION;
        record.schema = "some-other-tool".into();
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(matches!(
            store.load(root.path()),
            Err(SessionError::UnknownSchema(s)) if s == "some-other-tool"
        ));
        assert!(matches!(
            store.save(&record),
            Err(SessionError::UnknownSchema(_))
        ));
    }

    #[test]
    fn missing_state_file_is_no_session_not_an_error() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path());
        assert_eq!(store.load(root.path()).unwrap(), None);
    }

    #[test]
    fn oversized_payload_is_refused_before_parse() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path());
        let path = store.workspace_path(root.path());
        std::fs::create_dir_all(state.path()).unwrap();
        std::fs::write(&path, vec![b'x'; (MAX_SESSION_BYTES + 1) as usize]).unwrap();
        assert!(matches!(
            store.load(root.path()),
            Err(SessionError::TooLarge(n)) if n == MAX_SESSION_BYTES + 1
        ));
    }

    #[test]
    fn a_session_is_never_applied_to_another_workspace() {
        let root_a = tempfile::tempdir().unwrap();
        let root_b = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = fixture_workspace(root_a.path());
        let saved = SessionRecord::from_workspace(&workspace, root_a.path());
        let store = SessionStore::new(state.path());
        store.save(&saved).unwrap();

        // A moved root looks up a different key: an honest "no session", never
        // the other workspace's layout or documents.
        assert_eq!(store.load(root_b.path()).unwrap(), None);

        // Even if a record is planted at the other workspace's key, workspace
        // validation refuses it instead of applying foreign state.
        std::fs::create_dir_all(state.path()).unwrap();
        std::fs::write(
            store.workspace_path(root_b.path()),
            serde_json::to_vec(&saved).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            store.load(root_b.path()),
            Err(SessionError::WorkspaceMismatch(p)) if p == root_a.path()
        ));

        // The original workspace still restores exactly.
        assert_eq!(store.load(root_a.path()).unwrap().unwrap(), saved);
    }

    #[test]
    fn unwritable_state_directory_is_a_non_fatal_typed_failure() {
        let base = tempfile::tempdir().unwrap();
        let blocking = base.path().join("not-a-directory");
        std::fs::write(&blocking, b"file").unwrap();
        let store = SessionStore::new(blocking.join("nested").join("state"));
        let root = base.path().join("workspace");
        let record = minimal_record(&root);
        assert!(matches!(
            store.save(&record),
            Err(SessionError::Unavailable(_))
        ));
        assert!(matches!(
            store.load(&root),
            Err(SessionError::Unavailable(_))
        ));
    }

    #[test]
    fn save_leaves_no_temporary_files_behind() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = fixture_workspace(root.path());
        let record = SessionRecord::from_workspace(&workspace, root.path());
        let store = SessionStore::new(state.path());
        store.save(&record).unwrap();
        store.save(&record).unwrap();
        let leftover: Vec<_> = std::fs::read_dir(state.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp"))
            .collect();
        assert!(leftover.is_empty(), "temporary files left: {leftover:?}");
    }

    #[test]
    fn non_utf8_and_oversized_writes_and_versions_are_typed_refusals() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path());
        let path = store.workspace_path(root.path());
        std::fs::create_dir_all(state.path()).unwrap();
        std::fs::write(&path, [0xff, 0xfe, 0x00, b'{']).unwrap();
        assert!(matches!(
            store.load(root.path()),
            Err(SessionError::Corrupt(_))
        ));

        let mut record = minimal_record(root.path());
        record.version = 0;
        assert!(matches!(
            store.save(&record),
            Err(SessionError::UnsupportedVersion(0))
        ));

        let mut huge = minimal_record(root.path());
        huge.recent_files = vec![PathBuf::from("x".repeat(MAX_SESSION_BYTES as usize + 8))];
        assert!(matches!(
            store.save(&huge),
            Err(SessionError::TooLarge(n)) if n > MAX_SESSION_BYTES
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_state_file_is_refused_without_reading_it() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path());
        let target = state.path().join("target.json");
        std::fs::write(
            &target,
            serde_json::to_vec(&minimal_record(root.path())).unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(state.path()).unwrap();
        std::os::unix::fs::symlink(&target, store.workspace_path(root.path())).unwrap();
        assert!(matches!(
            store.load(root.path()),
            Err(SessionError::UnsafeState(_))
        ));
    }

    #[test]
    fn restore_and_persist_helpers_are_non_fatal_and_report_skips() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path());
        // No saved session yet: a missing file is not a notice.
        assert_eq!(
            store.restore_into(root.path(), &mut Workspace::default()),
            None
        );
        let workspace = fixture_workspace(root.path());
        assert_eq!(store.persist_from(root.path(), &workspace), None);

        let mut fresh = Workspace::default();
        assert_eq!(store.restore_into(root.path(), &mut fresh), None);
        assert_eq!(fresh.documents.len(), 2);

        // A now-missing document is reported, and the rest still restores.
        let mut partial = SessionRecord::from_workspace(&workspace, root.path());
        partial.documents.push(DocumentSession {
            path: root.path().join("gone.txt"),
            cursor_line: 0,
            cursor_col: 0,
            scroll_offset: 0,
            horizontal_offset: 0,
        });
        store.save(&partial).unwrap();
        let mut fresh = Workspace::default();
        let notice = store.restore_into(root.path(), &mut fresh).unwrap();
        assert!(notice.contains("skipped 1"), "{notice}");
        assert_eq!(fresh.documents.len(), 2);

        // An unwritable state directory surfaces a notice, never a panic.
        let blocking = root.path().join("not-a-dir");
        std::fs::write(&blocking, b"file").unwrap();
        let broken = SessionStore::new(blocking.join("nested"));
        let warning = broken.persist_from(root.path(), &workspace).unwrap();
        assert!(warning.contains("not saved"), "{warning}");
    }

    #[test]
    fn corrupt_payload_surfaces_a_non_fatal_restore_notice() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path());
        std::fs::create_dir_all(state.path()).unwrap();
        std::fs::write(store.workspace_path(root.path()), b"{ truncated").unwrap();
        let notice = store
            .restore_into(root.path(), &mut Workspace::default())
            .expect("a corrupt record must produce a visible notice");
        assert!(notice.contains("Session not restored"), "{notice}");
    }

    #[test]
    fn rename_failure_cleans_up_temporary_state_and_reports_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path());
        std::fs::create_dir_all(state.path()).unwrap();
        // Occupy the destination with a non-empty directory: the same-directory
        // rename must fail, leaving no partial session and no temp file.
        let destination = store.workspace_path(root.path());
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("child"), b"x").unwrap();
        let record = minimal_record(root.path());
        assert!(matches!(
            store.save(&record),
            Err(SessionError::Unavailable(_))
        ));
        let leftover: Vec<_> = std::fs::read_dir(state.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp"))
            .collect();
        assert!(leftover.is_empty(), "temporary files left: {leftover:?}");
    }

    #[test]
    fn restore_clamps_cursors_to_the_document_that_actually_loaded() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = fixture_workspace(root.path());
        let mut record = SessionRecord::from_workspace(&workspace, root.path());
        for document in &mut record.documents {
            document.cursor_line = 9_999;
            document.cursor_col = 9_999;
        }
        let store = SessionStore::new(state.path());
        store.save(&record).unwrap();
        let mut fresh = Workspace::default();
        store.restore_into(root.path(), &mut fresh);
        for document in fresh.documents.iter() {
            assert!(document.editor.cursor_line < document.editor.buffer.len());
            let line_len = document.editor.buffer[document.editor.cursor_line].len();
            assert!(document.editor.cursor_col <= line_len);
        }
    }

    #[test]
    fn extreme_viewport_offsets_restore_and_survive_the_real_render_path() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let workspace = fixture_workspace(root.path());
        let mut record = SessionRecord::from_workspace(&workspace, root.path());
        for document in &mut record.documents {
            document.scroll_offset = usize::MAX;
            document.horizontal_offset = usize::MAX;
            document.cursor_line = usize::MAX;
            document.cursor_col = usize::MAX;
        }
        let store = SessionStore::new(state.path());
        store.save(&record).unwrap();
        let mut fresh = Workspace::default();
        store.restore_into(root.path(), &mut fresh);

        // The real render path for an 80x24 frame updates every document viewport.
        for id in fresh
            .documents
            .iter()
            .map(|document| document.id())
            .collect::<Vec<_>>()
        {
            let editor = &mut fresh.documents.get_mut(id).unwrap().editor;
            editor.update_viewport(80, 24);
            assert!(editor.scroll_offset <= editor.visual_row_count());
            assert!(editor.horizontal_offset <= editor.buffer[editor.cursor_line].len() + 1);
            assert!(editor.cursor_line < editor.buffer.len());
        }
        // Predictable clamped state: a two-line document cannot scroll at all.
        for document in fresh.documents.iter() {
            assert_eq!(document.editor.scroll_offset, 0);
            assert_eq!(document.editor.horizontal_offset, 0);
        }
    }

    #[test]
    fn over_count_payloads_are_refused_at_load_and_apply() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path());
        std::fs::create_dir_all(state.path()).unwrap();
        let record = minimal_record(root.path());
        let mut value = serde_json::to_value(&record).unwrap();
        let entry = serde_json::to_value(DocumentSession {
            path: root.path().join("doc.txt"),
            cursor_line: 0,
            cursor_col: 0,
            scroll_offset: 0,
            horizontal_offset: 0,
        })
        .unwrap();
        let over = MAX_SESSION_DOCUMENTS + 1;
        value["documents"] = serde_json::Value::Array(vec![entry; over]);
        std::fs::write(store.workspace_path(root.path()), value.to_string()).unwrap();
        assert!(matches!(
            store.load(root.path()),
            Err(SessionError::TooManyDocuments(n)) if n == over
        ));

        // A hand-built record that bypassed parsing is truncated at apply too,
        // so no unbounded number of files is ever opened at startup.
        let mut crafted = record.clone();
        crafted.documents = vec![
            DocumentSession {
                path: root.path().join("doc.txt"),
                cursor_line: 0,
                cursor_col: 0,
                scroll_offset: 0,
                horizontal_offset: 0,
            };
            over
        ];
        std::fs::write(root.path().join("doc.txt"), b"content\n".as_slice()).unwrap();
        let mut fresh = Workspace::default();
        let outcome = crafted.apply_to_workspace(&mut fresh);
        assert_eq!(outcome.opened, MAX_SESSION_DOCUMENTS);
        assert_eq!(outcome.skipped, over - MAX_SESSION_DOCUMENTS);
        assert_eq!(fresh.documents.len(), 1, "the same path opens once");
    }
}
