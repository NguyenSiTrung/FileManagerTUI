//! Bounded, private recovery snapshots for unsaved document text.
//!
//! A snapshot is keyed by workspace root, document path, and the saved-revision
//! identity (size plus modification time) it was taken from. It holds the
//! unsaved buffer text plus that identity so a restore can refuse when the
//! on-disk file has moved on since capture. Records live under the private
//! state directory in a `recovery/` subtree, are written atomically, are size
//! validated before parsing, and are pruned by count and age only when the app
//! owns them.
//!
//! Restoring never writes the original file: it loads the snapshot into a dirty
//! document so the ordinary conflict-safe save path decides publication. All
//! failures are typed and non-fatal; a corrupt record never aborts startup.

use crate::config::{AppConfig, MAX_RECOVERY_MAX_AGE_SECS, MIN_RECOVERY_MAX_AGE_SECS};
use crate::editor::EditorState;
use crate::workspace::documents::DocumentStore;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Wire schema identifier for a recovery payload. Any other value is refused.
pub const RECOVERY_SCHEMA: &str = "fm-tui-recovery";
/// Current schema version. Higher versions are refused as "future".
pub const RECOVERY_VERSION: u32 = 1;
/// Hard cap on a serialized recovery payload, validated before parsing. Matches
/// the default editor byte cap so any admitted buffer can be snapshotted.
pub const MAX_RECOVERY_BYTES: u64 = 10 * 1024 * 1024;
/// Subdirectory of the private state directory that holds snapshot records.
pub const RECORDS_SUBDIR: &str = "recovery";

/// Disambiguates concurrent temporary record files within one process.
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Saved-revision identity captured from the on-disk file. Equality is exact:
/// size plus nanosecond modification time. It is intentionally cheap (no disk
/// read) so capture never competes with the input path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionRef {
    pub size: u64,
    pub modified_secs: i64,
    pub modified_nanos: u32,
    /// Whether the modification time was actually observed. When false the
    /// record carries a distinguishable identity (see [`Self::unknown_time`])
    /// that must never compare equal to a revision derived from a real file.
    pub modified_known: bool,
}

impl RevisionRef {
    /// Inspect `path` without following a symlinked snapshot source. A missing
    /// or non-regular file is a typed refusal for the caller to interpret.
    pub fn from_path(path: &Path) -> Result<Self, RecoveryError> {
        let metadata = std::fs::symlink_metadata(path).map_err(RecoveryError::Unavailable)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(RecoveryError::UnsafeRecord(path.to_path_buf()));
        }
        Ok(Self::from_metadata(&metadata))
    }

    /// Build an identity from observed metadata. A filesystem that cannot report
    /// a usable modification time (before the Unix epoch, or an error) yields a
    /// sentinel rather than a silent `(0, 0)`, because `(0, 0)` is a legitimate
    /// real timestamp (the Unix epoch) and would compare equal to a real file.
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        match metadata.modified() {
            Ok(time) => match time.duration_since(UNIX_EPOCH) {
                Ok(since) => Self {
                    size: metadata.len(),
                    modified_secs: since.as_secs().min(i64::MAX as u64) as i64,
                    modified_nanos: since.subsec_nanos(),
                    modified_known: true,
                },
                Err(_) => Self::unknown_time(metadata.len()),
            },
            Err(_) => Self::unknown_time(metadata.len()),
        }
    }

    /// A distinguishable identity for a file whose modification time could not
    /// be observed. The nanos sentinel is outside the `0..1_000_000_000` range a
    /// real timestamp can produce, so this value is never equal to a real
    /// revision even when the size and the epoch second coincide.
    fn unknown_time(size: u64) -> Self {
        Self {
            size,
            modified_secs: 0,
            modified_nanos: u32::MAX,
            modified_known: false,
        }
    }

    fn digest(self) -> u64 {
        hash_parts(&[
            &self.size.to_le_bytes(),
            &self.modified_secs.to_le_bytes(),
            &self.modified_nanos.to_le_bytes(),
            &[u8::from(self.modified_known)],
        ])
    }
}

/// One persisted recovery snapshot: the unsaved text plus the saved-revision
/// identity that publication must still match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryRecord {
    pub schema: String,
    pub version: u32,
    pub workspace_root: PathBuf,
    pub document_path: PathBuf,
    pub revision: RevisionRef,
    pub text: String,
    /// Unix seconds at capture; drives age retention only.
    pub captured_secs: i64,
}

impl RecoveryRecord {
    /// Build a snapshot, refusing text above `max_bytes` before it reaches disk.
    pub fn capture(
        workspace_root: &Path,
        document_path: &Path,
        text: &str,
        revision: RevisionRef,
        captured_at: SystemTime,
        max_bytes: usize,
    ) -> Result<Self, RecoveryError> {
        if text.len() > max_bytes {
            return Err(RecoveryError::TooLarge(text.len() as u64));
        }
        Ok(Self {
            schema: RECOVERY_SCHEMA.to_string(),
            version: RECOVERY_VERSION,
            workspace_root: workspace_root.to_path_buf(),
            document_path: document_path.to_path_buf(),
            revision,
            text: text.to_string(),
            captured_secs: unix_secs(captured_at),
        })
    }

    /// Recover this snapshot's text into `editor` without touching disk: the
    /// buffer becomes the unsaved text and is marked modified. The editor's
    /// existing saved revision is preserved so the ordinary conflict-safe save
    /// still compares against the version that was loaded from disk.
    pub fn apply_to_editor(&self, editor: &mut EditorState) {
        let saved_revision = editor.source_revision.clone();
        let mut recovered = EditorState::new(&self.text, self.document_path.clone());
        recovered.source_revision = saved_revision;
        recovered.modified = true;
        *editor = recovered;
    }
}

/// Typed, non-fatal recovery refusal or I/O failure.
#[derive(Debug, thiserror::Error)]
pub enum RecoveryError {
    #[error("recovery record is corrupt: {0}")]
    Corrupt(String),
    #[error("recovery record is {0} bytes, above the {MAX_RECOVERY_BYTES}-byte limit")]
    TooLarge(u64),
    #[error("unknown recovery schema: {0}")]
    UnknownSchema(String),
    #[error("recovery schema version {0} is newer than the supported {RECOVERY_VERSION}")]
    FutureVersion(u32),
    #[error("unsupported recovery schema version {0}")]
    UnsupportedVersion(u32),
    #[error("recovery state is a symlink or not a private directory, refusing it: {0}")]
    UnsafeState(PathBuf),
    #[error("recovery record is unsafe (symlink, foreign owner, or open permissions): {0}")]
    UnsafeRecord(PathBuf),
    #[error("recovery persistence is disabled")]
    Disabled,
    #[error("the document changed on disk since the snapshot was taken: {0}")]
    DiskChanged(PathBuf),
    #[error("the document is not open; open it before restoring: {0}")]
    DocumentNotOpen(PathBuf),
    #[error("recovery state is unavailable: {0}")]
    Unavailable(#[from] std::io::Error),
}

/// Bounded retention and throttling policy for recovery snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryPolicy {
    /// When false, capture is refused entirely.
    pub enabled: bool,
    /// Maximum retained snapshots for one workspace.
    pub max_records: usize,
    /// Maximum snapshot age.
    pub max_age: Duration,
    /// Minimum interval between throttled snapshot writes.
    pub min_interval: Duration,
    /// Maximum unsaved text size admitted in one snapshot.
    pub max_text_bytes: usize,
}

impl Default for RecoveryPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            max_records: crate::config::DEFAULT_RECOVERY_MAX_RECORDS,
            max_age: Duration::from_secs(crate::config::DEFAULT_RECOVERY_MAX_AGE_SECS),
            min_interval: Duration::from_millis(crate::config::DEFAULT_RECOVERY_MIN_INTERVAL_MS),
            max_text_bytes: MAX_RECOVERY_BYTES as usize,
        }
    }
}

impl RecoveryPolicy {
    /// Resolve the effective policy from configuration. The text bound follows
    /// the editor admission limit but never exceeds the record cap. The age is
    /// re-clamped here, independently of the config getter, so a caller that
    /// constructs an extreme `Duration` (or a future config path that bypasses
    /// the getter) still cannot produce a cutoff that deletes fresh records.
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            enabled: config.recovery_enabled(),
            max_records: config.recovery_max_records(),
            max_age: Duration::from_secs(
                config
                    .recovery_max_age_secs()
                    .clamp(MIN_RECOVERY_MAX_AGE_SECS, MAX_RECOVERY_MAX_AGE_SECS),
            ),
            min_interval: Duration::from_millis(config.recovery_min_interval_ms()),
            max_text_bytes: usize::try_from(config.max_editor_bytes())
                .unwrap_or(usize::MAX)
                .min(MAX_RECOVERY_BYTES as usize),
        }
    }

    /// A truncated, non-negative age in whole seconds.
    ///
    /// Retention compares signed seconds, so the age must never be cast from a
    /// value that does not fit `i64`. Clamping here (not only in config) makes
    /// the property local to the arithmetic that depends on it: a policy built
    /// directly in a test with `Duration::from_secs(u64::MAX)` cannot wrap to a
    /// negative cutoff and delete fresh snapshots.
    fn max_age_secs(self) -> i64 {
        self.max_age.as_secs().min(MAX_RECOVERY_MAX_AGE_SECS) as i64
    }
}

/// Monotonic write throttle. `due(now)` returns true at most once per interval;
/// callers must invoke it outside the input path and outside render.
#[derive(Debug, Clone)]
pub struct SnapshotThrottle {
    min_interval: Duration,
    last: Option<Instant>,
}

impl SnapshotThrottle {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            last: None,
        }
    }

    /// Whether a snapshot may be written now. A due call records the attempt so
    /// a burst of edits cannot trigger a burst of writes.
    pub fn due(&mut self, now: Instant) -> bool {
        match self.last {
            Some(last) if now.duration_since(last) < self.min_interval => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }

    /// Time until the next write is permitted, or `None` when it is already
    /// due. The main loop uses this to schedule a bounded wake while dirty
    /// work is pending, so an idle application still captures the last edit
    /// instead of waiting for the next input event.
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        self.last.and_then(|last| {
            let elapsed = now.duration_since(last);
            (elapsed < self.min_interval).then(|| self.min_interval - elapsed)
        })
    }
}

/// A private, injected state directory holding recovery records.
#[derive(Debug, Clone)]
pub struct RecoveryStore {
    directory: PathBuf,
}

impl RecoveryStore {
    /// Inject the private state directory. No user home directory is read here.
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    /// Build a store from the resolved config directory. Disabled persistence
    /// yields no store and no notice; enabled-without-a-directory yields a
    /// visible notice, never a silent disable.
    pub fn from_config_dir(
        enabled: bool,
        directory: Option<PathBuf>,
    ) -> (Option<Self>, Option<String>) {
        if !enabled {
            return (None, None);
        }
        match directory {
            Some(directory) => (Some(Self::new(directory)), None),
            None => (
                None,
                Some("Recovery persistence unavailable: no private state directory".to_string()),
            ),
        }
    }

    fn records_dir(&self) -> PathBuf {
        self.directory.join(RECORDS_SUBDIR)
    }

    /// Deterministic record file for a workspace/document/revision triple:
    /// different revisions never collide, so a newer snapshot cannot overwrite
    /// the recovered text of an older one.
    pub fn record_path(
        &self,
        workspace_root: &Path,
        document_path: &Path,
        revision: &RevisionRef,
    ) -> PathBuf {
        self.records_dir().join(format!(
            "snap-{:016x}-{:016x}-{:016x}.json",
            path_hash(workspace_root),
            path_hash(document_path),
            revision.digest()
        ))
    }

    /// Persist `record` atomically and enforce retention. Refuses when the
    /// policy is disabled or the payload exceeds the size cap.
    pub fn save(
        &self,
        record: &RecoveryRecord,
        policy: &RecoveryPolicy,
    ) -> Result<PathBuf, RecoveryError> {
        if !policy.enabled {
            return Err(RecoveryError::Disabled);
        }
        if record.schema != RECOVERY_SCHEMA {
            return Err(RecoveryError::UnknownSchema(record.schema.clone()));
        }
        if record.version != RECOVERY_VERSION {
            return Err(RecoveryError::UnsupportedVersion(record.version));
        }
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|error| RecoveryError::Corrupt(error.to_string()))?;
        if bytes.len() as u64 > MAX_RECOVERY_BYTES {
            return Err(RecoveryError::TooLarge(bytes.len() as u64));
        }
        let directory = self.ensure_records_dir()?;
        let path = self.record_path(
            &record.workspace_root,
            &record.document_path,
            &record.revision,
        );
        if let Ok(metadata) = std::fs::symlink_metadata(&path) {
            if metadata.file_type().is_symlink() {
                return Err(RecoveryError::UnsafeRecord(path));
            }
        }
        let temp = directory.join(format!(
            ".recovery-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        match write_atomic(&temp, &path, &bytes) {
            Ok(()) => {}
            Err(error) => {
                let _ = std::fs::remove_file(&temp);
                return Err(RecoveryError::Unavailable(error));
            }
        }
        self.prune(&record.workspace_root, policy, SystemTime::now())?;
        Ok(path)
    }

    /// Load and validate one record file. Size is checked before parsing and
    /// every storage-safety check runs before the bytes are read.
    pub fn load(&self, path: &Path) -> Result<RecoveryRecord, RecoveryError> {
        self.open_records_dir()?;
        self.load_record(path)
    }

    /// All currently retained records for `workspace_root`, after applying
    /// count/age retention. Corrupt records are skipped here rather than
    /// aborting the caller; an explicit `load` still returns the typed error.
    pub fn load_all(
        &self,
        workspace_root: &Path,
        policy: &RecoveryPolicy,
        now: SystemTime,
    ) -> Vec<RecoveryRecord> {
        if self.prune(workspace_root, policy, now).is_err() {
            return Vec::new();
        }
        self.owned_records(workspace_root)
            .into_iter()
            .filter_map(|path| self.load(&path).ok())
            .collect()
    }

    /// Recover `record` into the matching open document: refuse when the disk
    /// changed since capture or the document is not open, then make the buffer
    /// the unsaved text. Never writes the original file.
    pub fn restore(
        &self,
        record: &RecoveryRecord,
        documents: &mut DocumentStore,
    ) -> Result<(), RecoveryError> {
        let current = RevisionRef::from_path(&record.document_path)
            .map_err(|_| RecoveryError::DiskChanged(record.document_path.clone()))?;
        if current != record.revision {
            return Err(RecoveryError::DiskChanged(record.document_path.clone()));
        }
        let id = documents
            .iter()
            .find(|document| document.path() == record.document_path)
            .map(|document| document.id())
            .ok_or_else(|| RecoveryError::DocumentNotOpen(record.document_path.clone()))?;
        let document = documents
            .get_mut(id)
            .ok_or_else(|| RecoveryError::DocumentNotOpen(record.document_path.clone()))?;
        record.apply_to_editor(&mut document.editor);
        Ok(())
    }

    /// Delete every retained record for one document that this app owns.
    pub fn discard(
        &self,
        workspace_root: &Path,
        document_path: &Path,
    ) -> Result<usize, RecoveryError> {
        let prefix = format!(
            "snap-{:016x}-{:016x}-",
            path_hash(workspace_root),
            path_hash(document_path)
        );
        self.remove_owned_matching(&|name| is_record_name(name) && name.starts_with(&prefix))
    }

    /// Delete every recovery record this app owns, across all workspaces.
    ///
    /// Uses the same full `snap-<ws>-<doc>-<rev>.json` ownership shape as
    /// `discard`/`owned_records`; a bare `snap-` prefix would also match a
    /// same-owner file such as `snap-notes.json` that this app never created.
    pub fn clear(&self) -> Result<usize, RecoveryError> {
        self.remove_owned_shaped(is_record_name)
    }

    /// Enforce age and count retention for one workspace. Only owned, regular,
    /// non-symlinked record files are ever deleted.
    pub fn prune(
        &self,
        workspace_root: &Path,
        policy: &RecoveryPolicy,
        now: SystemTime,
    ) -> Result<usize, RecoveryError> {
        self.open_records_dir()?;
        let mut entries: Vec<(i64, PathBuf)> = Vec::new();
        for path in self.owned_records(workspace_root) {
            entries.push((self.record_order(&path), path));
        }
        let now_secs = unix_secs(now);
        let age_cutoff = now_secs.saturating_sub(policy.max_age_secs());
        let mut deleted = 0usize;
        entries.sort_by_key(|e| e.0);
        let mut retained: Vec<(i64, PathBuf)> = Vec::new();
        for (order, path) in entries {
            if order < age_cutoff {
                if remove_owned_record(&path) {
                    deleted += 1;
                }
            } else {
                retained.push((order, path));
            }
        }
        let over = retained.len().saturating_sub(policy.max_records.max(1));
        for (_, path) in retained.into_iter().take(over) {
            if remove_owned_record(&path) {
                deleted += 1;
            }
        }
        Ok(deleted)
    }

    /// Owned record files for `workspace_root`, newest-agnostic and unfiltered
    /// by age or count.
    fn owned_records(&self, workspace_root: &Path) -> Vec<PathBuf> {
        let prefix = format!("snap-{:016x}-", path_hash(workspace_root));
        owned_files(&self.records_dir(), &|name| {
            is_record_name(name) && name.starts_with(&prefix)
        })
    }

    /// Best-effort ordering key for a record: its captured time, else the file
    /// modification time. Never follows a symlink.
    fn record_order(&self, path: &Path) -> i64 {
        if let Ok(record) = self.load_record(path) {
            return record.captured_secs;
        }
        std::fs::symlink_metadata(path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|since| since.as_secs() as i64)
            .unwrap_or(i64::MIN)
    }

    /// Remove owned files whose name satisfies `shape`. Never deletes a
    /// foreign, symlinked, non-regular, or non-record-shaped entry.
    fn remove_owned_shaped(&self, shape: fn(&str) -> bool) -> Result<usize, RecoveryError> {
        self.remove_owned_matching(&shape)
    }

    fn remove_owned_matching(&self, shape: &dyn Fn(&str) -> bool) -> Result<usize, RecoveryError> {
        self.open_records_dir()?;
        let mut removed = 0usize;
        for path in owned_files(&self.records_dir(), shape) {
            if remove_owned_record(&path) {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Reject a symlinked or non-directory configured state path. A missing path
    /// is allowed here; callers that create it re-validate afterwards. The
    /// state directory is the owning process's own creation (usually a platform
    /// state root such as `~/.local/state`, which is often group-readable), so
    /// its mode is not part of the private-storage guarantee; the `recovery`
    /// subdirectory and every record inside it are the private boundary.
    fn validate_state_dir(&self) -> Result<(), RecoveryError> {
        if let Ok(metadata) = std::fs::symlink_metadata(&self.directory) {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(RecoveryError::UnsafeState(self.directory.clone()));
            }
        }
        Ok(())
    }

    /// Validate and create the private records directory. Refuses symlinked or
    /// non-owner-only storage and files owned by another user.
    fn ensure_records_dir(&self) -> Result<PathBuf, RecoveryError> {
        self.validate_state_dir()?;
        let dir = self.records_dir();
        match std::fs::symlink_metadata(&dir) {
            Ok(metadata) => {
                check_private_dir(&dir, &metadata)?;
                Ok(dir)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(&dir)?;
                set_private_permissions(&dir);
                let metadata = std::fs::symlink_metadata(&dir)?;
                check_private_dir(&dir, &metadata)?;
                Ok(dir)
            }
            Err(error) => Err(RecoveryError::Unavailable(error)),
        }
    }

    /// Validate the records directory for reading; a missing directory is not
    /// an error at this level (callers treat it as "no records").
    fn open_records_dir(&self) -> Result<PathBuf, RecoveryError> {
        self.validate_state_dir()?;
        let dir = self.records_dir();
        match std::fs::symlink_metadata(&dir) {
            Ok(metadata) => {
                check_private_dir(&dir, &metadata)?;
                Ok(dir)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(dir),
            Err(error) => Err(RecoveryError::Unavailable(error)),
        }
    }

    fn load_record(&self, path: &Path) -> Result<RecoveryRecord, RecoveryError> {
        let metadata = std::fs::symlink_metadata(path).map_err(RecoveryError::Unavailable)?;
        if metadata.file_type().is_symlink() {
            return Err(RecoveryError::UnsafeRecord(path.to_path_buf()));
        }
        if !metadata.is_file() {
            return Err(RecoveryError::UnsafeRecord(path.to_path_buf()));
        }
        if !owned_by_current_user(&metadata) {
            return Err(RecoveryError::UnsafeRecord(path.to_path_buf()));
        }
        if !owner_only_permissions(&metadata) {
            return Err(RecoveryError::UnsafeRecord(path.to_path_buf()));
        }
        if metadata.len() > MAX_RECOVERY_BYTES {
            return Err(RecoveryError::TooLarge(metadata.len()));
        }
        let mut content = String::new();
        {
            use std::io::Read;
            let file = std::fs::File::open(path)?;
            file.take(MAX_RECOVERY_BYTES + 1)
                .read_to_string(&mut content)
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::InvalidData {
                        RecoveryError::Corrupt(error.to_string())
                    } else {
                        RecoveryError::Unavailable(error)
                    }
                })?;
        }
        if content.len() as u64 > MAX_RECOVERY_BYTES {
            return Err(RecoveryError::TooLarge(content.len() as u64));
        }
        parse_record(&content)
    }
}

/// Validate a decoded record payload against the schema and version.
fn parse_record(content: &str) -> Result<RecoveryRecord, RecoveryError> {
    let record: RecoveryRecord =
        serde_json::from_str(content).map_err(|error| RecoveryError::Corrupt(error.to_string()))?;
    if record.schema != RECOVERY_SCHEMA {
        return Err(RecoveryError::UnknownSchema(record.schema));
    }
    if record.version > RECOVERY_VERSION {
        return Err(RecoveryError::FutureVersion(record.version));
    }
    if record.version != RECOVERY_VERSION {
        return Err(RecoveryError::UnsupportedVersion(record.version));
    }
    if record.text.len() as u64 > MAX_RECOVERY_BYTES {
        return Err(RecoveryError::TooLarge(record.text.len() as u64));
    }
    Ok(record)
}

/// Files in `dir` whose name begins with `prefix`, ending in `.json`, that are
/// owned regular files this app may act on. Symlinks and foreign entries are
/// excluded, never deleted.
fn owned_files(dir: &Path, shape: &dyn Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return paths;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !shape(name) {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        if !owned_by_current_user(&metadata) {
            continue;
        }
        paths.push(path);
    }
    paths.sort();
    paths
}

/// Whether `name` is exactly an app-written record:
/// `snap-<16 hex>-<16 hex>-<16 hex>.json`. The three dash-separated groups and
/// the `.json` suffix are load-bearing, so a same-owner file that merely shares
/// the `snap-` prefix (`snap-notes.json`, `snap-anything.json`) is never treated
/// as a record this app owns.
fn is_record_name(name: &str) -> bool {
    let Some(body) = name
        .strip_prefix("snap-")
        .and_then(|rest| rest.strip_suffix(".json"))
    else {
        return false;
    };
    let groups: Vec<&str> = body.split('-').collect();
    groups.len() == 3
        && groups
            .iter()
            .all(|group| group.len() == 16 && group.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn check_private_dir(path: &Path, metadata: &std::fs::Metadata) -> Result<(), RecoveryError> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RecoveryError::UnsafeState(path.to_path_buf()));
    }
    if !owned_by_current_user(metadata) {
        return Err(RecoveryError::UnsafeState(path.to_path_buf()));
    }
    if !owner_only_permissions(metadata) {
        return Err(RecoveryError::UnsafeState(path.to_path_buf()));
    }
    Ok(())
}

fn remove_owned_record(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            std::fs::remove_file(path).is_ok()
        }
        _ => false,
    }
}

fn set_private_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Whether the metadata grants no permission to group or other.
fn owner_only_permissions(metadata: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o077 == 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}

/// Whether the file/directory is owned by this process' effective user.
fn owned_by_current_user(metadata: &std::fs::Metadata) -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::fs::MetadataExt;
        let effective = unsafe { libc::geteuid() };
        metadata.uid() == effective
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = metadata;
        true
    }
}

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

/// FNV-1a over path bytes, matching the session store's keying approach.
fn path_hash(path: &Path) -> u64 {
    hash_parts(&[path.to_string_lossy().as_bytes()])
}

fn hash_parts(parts: &[&[u8]]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in *part {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

fn unix_secs(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::documents::{DocumentStore, OpenDisposition};

    fn policy() -> RecoveryPolicy {
        RecoveryPolicy {
            enabled: true,
            max_records: 8,
            max_age: Duration::from_secs(3_600),
            min_interval: Duration::ZERO,
            max_text_bytes: MAX_RECOVERY_BYTES as usize,
        }
    }

    fn open_document(root: &Path, name: &str, bytes: &[u8]) -> (DocumentStore, PathBuf) {
        let path = root.join(name);
        std::fs::write(&path, bytes).unwrap();
        let mut documents = DocumentStore::new();
        documents.open(&path, OpenDisposition::Pinned).unwrap();
        (documents, path)
    }

    /// Replace the single open document's buffer with exactly `text` and mark it
    /// dirty, mirroring a user edit that has not been saved.
    fn dirty_document(documents: &mut DocumentStore, text: &str) -> PathBuf {
        let id = documents.active_id().unwrap();
        let path = documents.get(id).unwrap().path().to_path_buf();
        let mut editor = EditorState::new(text, path.clone());
        editor.modified = true;
        documents.get_mut(id).unwrap().editor = editor;
        path
    }

    #[test]
    fn unsaved_text_is_captured_without_touching_the_original() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let original_bytes = b"key: 1\n";
        let (mut documents, original) = open_document(root.path(), "doc.yaml", original_bytes);
        let document_path = dirty_document(&mut documents, "unsaved:\n  value: 1\n");
        let document = documents.active().unwrap();
        let revision = RevisionRef::from_path(document.path()).unwrap();

        let snapshot = RecoveryRecord::capture(
            root.path(),
            &document_path,
            &document.text(),
            revision,
            SystemTime::now(),
            MAX_RECOVERY_BYTES as usize,
        )
        .unwrap();
        let store = RecoveryStore::new(state.path());
        store.save(&snapshot, &policy()).unwrap();

        assert_eq!(snapshot.text, "unsaved:\n  value: 1\n");
        assert_eq!(std::fs::read(&original).unwrap(), original_bytes);
    }

    #[test]
    fn a_disk_change_since_capture_is_refused_on_restore() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let (mut documents, original) = open_document(root.path(), "doc.txt", b"one\n");
        let document_path = dirty_document(&mut documents, "unsaved two\n");
        let revision = RevisionRef::from_path(&document_path).unwrap();
        let snapshot = RecoveryRecord::capture(
            root.path(),
            &document_path,
            "unsaved two\n",
            revision,
            SystemTime::now(),
            MAX_RECOVERY_BYTES as usize,
        )
        .unwrap();
        let store = RecoveryStore::new(state.path());
        store.save(&snapshot, &policy()).unwrap();

        // Another writer changes the file after the snapshot was taken.
        std::fs::write(&original, b"changed on disk\n").unwrap();
        match store.restore(&snapshot, &mut documents) {
            Err(RecoveryError::DiskChanged(path)) => assert_eq!(path, document_path),
            other => panic!("disk change must be refused: {other:?}"),
        }
        // The open buffer and the on-disk file are both untouched.
        assert_eq!(documents.active().unwrap().text(), "unsaved two\n");
        assert_eq!(std::fs::read(&original).unwrap(), b"changed on disk\n");
    }

    #[test]
    fn restore_never_overwrites_the_original_and_requires_a_dirty_document() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let original_bytes = b"saved: true\n";
        let (mut captured, original) = open_document(root.path(), "doc.yaml", original_bytes);
        let document_path = dirty_document(&mut captured, "unsaved: yes\n");
        let revision = RevisionRef::from_path(&document_path).unwrap();
        let snapshot = RecoveryRecord::capture(
            root.path(),
            &document_path,
            "unsaved: yes\n",
            revision,
            SystemTime::now(),
            MAX_RECOVERY_BYTES as usize,
        )
        .unwrap();
        let store = RecoveryStore::new(state.path());
        store.save(&snapshot, &policy()).unwrap();

        // Restore into a fresh, cleanly loaded copy of the document; the buffer
        // becomes dirty and the file on disk is never written by the restore.
        let mut documents = DocumentStore::new();
        documents.open(&original, OpenDisposition::Pinned).unwrap();
        assert!(!documents.active().unwrap().editor.modified);
        store.restore(&snapshot, &mut documents).unwrap();
        let (id, dirty, text) = {
            let document = documents.active().unwrap();
            (document.id(), document.editor.modified, document.text())
        };
        assert!(dirty);
        assert_eq!(text, "unsaved: yes\n");
        assert_eq!(std::fs::read(&original).unwrap(), original_bytes);

        // An ordinary save is a separate, explicit decision and is the only
        // path that can publish the recovered text.
        documents.get_mut(id).unwrap().editor.save().unwrap();
        assert_eq!(std::fs::read(&original).unwrap(), b"unsaved: yes\n");
    }

    #[test]
    fn restore_refuses_when_the_document_is_not_open() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let (documents, document_path) = open_document(root.path(), "doc.txt", b"body\n");
        let revision = RevisionRef::from_path(&document_path).unwrap();
        let snapshot = RecoveryRecord::capture(
            root.path(),
            &document_path,
            "unsaved\n",
            revision,
            SystemTime::now(),
            MAX_RECOVERY_BYTES as usize,
        )
        .unwrap();
        let store = RecoveryStore::new(state.path());
        let mut empty = DocumentStore::new();
        let error = store.restore(&snapshot, &mut empty).unwrap_err();
        assert!(matches!(error, RecoveryError::DocumentNotOpen(_)));
        // The original store is unaffected.
        assert_eq!(documents.active().unwrap().text(), "body\n");
    }

    #[test]
    fn corrupt_truncated_and_empty_records_are_typed_refusals() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(state.path());
        let document_path = root.path().join("doc.txt");
        let revision = RevisionRef {
            size: 1,
            modified_secs: 1,
            modified_nanos: 0,
            modified_known: true,
        };
        // Create the private records directory via a valid save.
        let valid = RecoveryRecord::capture(
            root.path(),
            &document_path,
            "x",
            revision,
            SystemTime::now(),
            MAX_RECOVERY_BYTES as usize,
        )
        .unwrap();
        let path = store.save(&valid, &policy()).unwrap();
        for payload in ["", "{ not json", "{\"schema\":\"fm-tui-recovery\",", "\0\0"] {
            std::fs::write(&path, payload).unwrap();
            assert!(
                matches!(store.load(&path), Err(RecoveryError::Corrupt(_))),
                "payload {payload:?} must be refused as corrupt"
            );
        }
        let bytes = serde_json::to_vec(&valid).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(store.load(&path), Err(RecoveryError::Corrupt(_))));

        // Corruption never aborts a bulk load; the bad record is simply skipped.
        std::fs::write(&path, "{ not json").unwrap();
        assert!(store
            .load_all(root.path(), &policy(), SystemTime::now())
            .is_empty());
    }

    #[test]
    fn unknown_future_and_unsupported_versions_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(state.path());
        let document_path = root.path().join("doc.txt");
        let revision = RevisionRef {
            size: 1,
            modified_secs: 1,
            modified_nanos: 0,
            modified_known: true,
        };
        let mut record = RecoveryRecord::capture(
            root.path(),
            &document_path,
            "x",
            revision,
            SystemTime::now(),
            MAX_RECOVERY_BYTES as usize,
        )
        .unwrap();
        let path = store.save(&record, &policy()).unwrap();

        record.version = RECOVERY_VERSION + 1;
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(matches!(
            store.load(&path),
            Err(RecoveryError::FutureVersion(v)) if v == RECOVERY_VERSION + 1
        ));

        record.version = 0;
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(matches!(
            store.load(&path),
            Err(RecoveryError::UnsupportedVersion(0))
        ));
        assert!(matches!(
            store.save(&record, &policy()),
            Err(RecoveryError::UnsupportedVersion(0))
        ));

        record.version = RECOVERY_VERSION;
        record.schema = "other-tool".into();
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(matches!(
            store.load(&path),
            Err(RecoveryError::UnknownSchema(s)) if s == "other-tool"
        ));
        assert!(matches!(
            store.save(&record, &policy()),
            Err(RecoveryError::UnknownSchema(_))
        ));
    }

    #[test]
    fn oversized_records_are_refused_before_parse_and_capture() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(state.path());
        let document_path = root.path().join("doc.txt");
        let revision = RevisionRef {
            size: 1,
            modified_secs: 1,
            modified_nanos: 0,
            modified_known: true,
        };
        let valid = RecoveryRecord::capture(
            root.path(),
            &document_path,
            "x",
            revision,
            SystemTime::now(),
            MAX_RECOVERY_BYTES as usize,
        )
        .unwrap();
        let path = store.save(&valid, &policy()).unwrap();
        std::fs::write(&path, vec![b'x'; (MAX_RECOVERY_BYTES + 1) as usize]).unwrap();
        assert!(matches!(
            store.load(&path),
            Err(RecoveryError::TooLarge(n)) if n == MAX_RECOVERY_BYTES + 1
        ));

        // Capture refuses text above the configured bound before any write.
        let oversized = RecoveryRecord::capture(
            root.path(),
            &document_path,
            "abcdef",
            revision,
            SystemTime::now(),
            4,
        );
        assert!(matches!(oversized, Err(RecoveryError::TooLarge(6))));
    }

    #[test]
    fn retention_prunes_by_age_and_count_on_write_and_load() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(state.path());
        let now = SystemTime::now();
        let mut policy = policy();
        policy.max_records = 2;

        // Age retention: an old record is deleted on write.
        let old = RecoveryRecord::capture(
            root.path(),
            &root.path().join("old.txt"),
            "old",
            RevisionRef {
                size: 1,
                modified_secs: 1,
                modified_nanos: 0,
                modified_known: true,
            },
            now - Duration::from_secs(10_000),
            MAX_RECOVERY_BYTES as usize,
        )
        .unwrap();
        store.save(&old, &policy).unwrap();
        assert!(
            store.load_all(root.path(), &policy, now).is_empty(),
            "expired record must be pruned on write"
        );

        // Count retention: three fresh records keep only the newest two.
        for index in 0..3 {
            let record = RecoveryRecord::capture(
                root.path(),
                &root.path().join(format!("f{index}.txt")),
                "text",
                RevisionRef {
                    size: index as u64,
                    modified_secs: index as i64,
                    modified_nanos: 0,
                    modified_known: true,
                },
                now,
                MAX_RECOVERY_BYTES as usize,
            )
            .unwrap();
            store.save(&record, &policy).unwrap();
        }
        let retained = store.load_all(root.path(), &policy, now);
        assert_eq!(retained.len(), 2, "count retention must cap records");

        // Age retention is also applied on load, using the caller's clock.
        let later = now + Duration::from_secs(10_000);
        assert!(store.load_all(root.path(), &policy, later).is_empty());
    }

    #[test]
    fn extreme_age_configuration_keeps_recent_snapshots_and_still_prunes_old_ones() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(state.path());
        let now = SystemTime::now();

        // Config surface: an extreme value is clamped, not cast into signed
        // negative seconds. Before the upper clamp this returned u64::MAX and
        // the cutoff wrapped, deleting every fresh snapshot on the next save.
        let mut config = AppConfig::default();
        config.recovery.max_age_secs = Some(u64::MAX);
        assert_eq!(
            config.recovery_max_age_secs(),
            crate::config::MAX_RECOVERY_MAX_AGE_SECS
        );
        let resolved = RecoveryPolicy::from_config(&config);
        assert_eq!(
            resolved.max_age,
            Duration::from_secs(crate::config::MAX_RECOVERY_MAX_AGE_SECS)
        );

        // Defensive policy path: a caller-built extreme Duration is clamped by
        // the pruning arithmetic itself.
        let extreme = RecoveryPolicy {
            max_age: Duration::from_secs(u64::MAX),
            ..policy()
        };
        let fresh = RecoveryRecord::capture(
            root.path(),
            &root.path().join("fresh.txt"),
            "unsaved",
            RevisionRef {
                size: 7,
                modified_secs: 7,
                modified_nanos: 0,
                modified_known: true,
            },
            now,
            MAX_RECOVERY_BYTES as usize,
        )
        .unwrap();
        store.save(&fresh, &extreme).unwrap();
        assert_eq!(
            store.load_all(root.path(), &extreme, now).len(),
            1,
            "an extreme configured age must never prune a fresh snapshot"
        );

        // The same extreme policy still prunes a genuinely ancient record.
        let ancient = RecoveryRecord::capture(
            root.path(),
            &root.path().join("ancient.txt"),
            "old",
            RevisionRef {
                size: 3,
                modified_secs: 3,
                modified_nanos: 0,
                modified_known: true,
            },
            now - Duration::from_secs(crate::config::MIN_RECOVERY_MAX_AGE_SECS + 60),
            MAX_RECOVERY_BYTES as usize,
        )
        .unwrap();
        let ancient_policy = RecoveryPolicy {
            max_records: 4,
            ..extreme
        };
        // Capture at an age just past the minimum bound, then prune with the
        // immediately-following clock so it is expired relative to the minimum.
        store.save(&ancient, &ancient_policy).unwrap();
        let just_after = now + Duration::from_secs(crate::config::MIN_RECOVERY_MAX_AGE_SECS + 120);
        let min_policy = RecoveryPolicy {
            max_age: Duration::from_secs(crate::config::MIN_RECOVERY_MAX_AGE_SECS),
            ..policy()
        };
        let retained = store.load_all(root.path(), &min_policy, just_after);
        assert!(
            retained
                .iter()
                .all(|record| record.document_path != root.path().join("ancient.txt")),
            "genuinely ancient records must still be pruned"
        );
    }

    #[test]
    fn failed_atomic_write_leaves_no_temporary_record_and_reports_the_error() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(state.path());
        let record = RecoveryRecord::capture(
            root.path(),
            &root.path().join("doc.txt"),
            "text",
            RevisionRef {
                size: 1,
                modified_secs: 1,
                modified_nanos: 0,
                modified_known: true,
            },
            SystemTime::now(),
            MAX_RECOVERY_BYTES as usize,
        )
        .unwrap();
        // Occupy the destination with a non-empty directory. It passes the store's
        // `is_symlink` preflight (it is a plain directory, not a symlink), the
        // exclusive temp create succeeds, the byte write completes, and then the
        // rename fails and must be cleaned up.
        std::fs::create_dir_all(store.records_dir()).unwrap();
        set_private_permissions(&store.records_dir());
        let destination = store.record_path(
            &record.workspace_root,
            &record.document_path,
            &record.revision,
        );
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("child"), b"x").unwrap();

        let outcome = store.save(&record, &policy());
        assert!(
            matches!(outcome, Err(RecoveryError::Unavailable(_))),
            "a failed atomic rename must be a typed failure: {outcome:?}"
        );
        let leftover: Vec<_> = std::fs::read_dir(store.records_dir())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp"))
            .collect();
        assert!(leftover.is_empty(), "temporary records left: {leftover:?}");
    }

    #[test]
    fn an_unknown_modification_time_is_distinguishable_from_a_real_revision() {
        // A real file at the Unix epoch would compare as (0, 0); the sentinel
        // that stands in for an unreadable mtime must never collide with it.
        let unknown = RevisionRef {
            size: 12,
            ..RevisionRef::unknown_time(12)
        };
        assert!(!unknown.modified_known);
        assert_eq!(unknown.modified_nanos, u32::MAX);
        let epoch_file = RevisionRef {
            size: 12,
            modified_secs: 0,
            modified_nanos: 0,
            modified_known: true,
        };
        assert_ne!(unknown, epoch_file);
        assert_ne!(unknown.digest(), epoch_file.digest());

        // The sentinel's nanos are outside the real subsecond range, which is the
        // property that makes it distinguishable from any observed timestamp.
        assert!(unknown.modified_nanos >= 1_000_000_000);

        // The sentinel is produced only on a degraded read, and a real path
        // always yields a known time equal to its own observed metadata, never
        // the sentinel and never a silent epoch zero.
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("doc.txt");
        std::fs::write(&path, b"body").unwrap();
        let observed = RevisionRef::from_path(&path).unwrap();
        assert!(observed.modified_known);
        assert_eq!(
            observed,
            RevisionRef::from_metadata(&std::fs::metadata(&path).unwrap())
        );
        assert_ne!(observed, unknown);

        // A genuinely epoch-stamped real file is still distinguishable from the
        // sentinel, so a degraded identity can never collide with a real one.
        let epoch_stamped = RevisionRef {
            size: observed.size,
            modified_secs: 0,
            modified_nanos: 0,
            modified_known: true,
        };
        assert_ne!(epoch_stamped, unknown);
    }

    #[test]
    fn prune_never_touches_another_workspace_or_foreign_files() {
        let workspace_a = tempfile::tempdir().unwrap();
        let workspace_b = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        // Save with a generous age bound so the just-written records are
        // retained until the scoped prune below.
        let save_policy = RecoveryPolicy {
            max_age: Duration::from_secs(1_000_000),
            ..policy()
        };
        let store = RecoveryStore::new(state.path());
        // A's record is old enough to expire; B's is fresh and must survive.
        for (root, captured_at) in [
            (workspace_a.path(), now - Duration::from_secs(10_000)),
            (workspace_b.path(), now),
        ] {
            let record = RecoveryRecord::capture(
                root,
                &root.join("doc.txt"),
                "text",
                RevisionRef {
                    size: 1,
                    modified_secs: 1,
                    modified_nanos: 0,
                    modified_known: true,
                },
                captured_at,
                MAX_RECOVERY_BYTES as usize,
            )
            .unwrap();
            store.save(&record, &save_policy).unwrap();
        }
        // A foreign file in the records directory must survive every prune.
        let foreign = store.records_dir().join("keep.txt");
        std::fs::write(&foreign, b"keep").unwrap();

        // Pruning workspace A expires only its own records.
        store
            .prune(workspace_a.path(), &policy(), now)
            .expect("prune is non-fatal");
        assert!(store
            .load_all(workspace_a.path(), &policy(), now)
            .is_empty());
        assert_eq!(store.load_all(workspace_b.path(), &policy(), now).len(), 1);
        assert!(foreign.exists());
    }

    #[test]
    fn clear_removes_only_owned_records_and_discard_is_document_scoped() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(state.path());
        let now = SystemTime::now();
        let mut paths = Vec::new();
        for index in 0..2 {
            let record = RecoveryRecord::capture(
                root.path(),
                &root.path().join(format!("doc{index}.txt")),
                "text",
                RevisionRef {
                    size: index as u64,
                    modified_secs: 1,
                    modified_nanos: 0,
                    modified_known: true,
                },
                now,
                MAX_RECOVERY_BYTES as usize,
            )
            .unwrap();
            paths.push(store.save(&record, &policy()).unwrap());
        }
        let foreign = store.records_dir().join("notes.txt");
        std::fs::write(&foreign, b"not ours").unwrap();
        // Same owner, broader `snap-` prefix: `clear` must not treat a file this
        // app never wrote as an owned record.
        let decoy = store.records_dir().join("snap-notes.json");
        std::fs::write(&decoy, b"not a record").unwrap();

        assert_eq!(
            store
                .discard(root.path(), &root.path().join("doc0.txt"))
                .unwrap(),
            1
        );
        assert!(store
            .load_all(root.path(), &policy(), now)
            .iter()
            .all(|record| record.document_path != root.path().join("doc0.txt")));
        assert_eq!(store.load_all(root.path(), &policy(), now).len(), 1);

        // `clear` deletes the remaining record (doc1.txt) but not the decoy.
        let cleared = store.clear().unwrap();
        let entries: Vec<_> = std::fs::read_dir(store.records_dir())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(cleared, 1, "remaining entries: {entries:?}");
        assert!(store.load_all(root.path(), &policy(), now).is_empty());
        assert!(foreign.exists(), "clear must not delete foreign files");
        assert!(
            decoy.exists(),
            "clear must not delete a same-owner file with a non-record name"
        );
    }

    #[test]
    fn disabled_persistence_refuses_writes_and_yields_a_notice_only_when_needed() {
        let state = tempfile::tempdir().unwrap();
        let (store, notice) = RecoveryStore::from_config_dir(false, Some(state.path().into()));
        assert!(store.is_none());
        assert!(notice.is_none(), "an explicit disable is not a failure");

        let (store, notice) = RecoveryStore::from_config_dir(true, Some(state.path().into()));
        assert!(store.is_some(), "an injected directory yields a store");
        assert!(notice.is_none());

        let (store, notice) = RecoveryStore::from_config_dir(true, None);
        assert!(store.is_none());
        assert!(notice.unwrap().contains("unavailable"));

        let store = RecoveryStore::new(state.path());
        let mut disabled = policy();
        disabled.enabled = false;
        let record = RecoveryRecord::capture(
            state.path(),
            &state.path().join("doc.txt"),
            "text",
            RevisionRef {
                size: 1,
                modified_secs: 1,
                modified_nanos: 0,
                modified_known: true,
            },
            SystemTime::now(),
            MAX_RECOVERY_BYTES as usize,
        )
        .unwrap();
        assert!(matches!(
            store.save(&record, &disabled),
            Err(RecoveryError::Disabled)
        ));
    }

    #[test]
    fn throttle_allows_at_most_one_write_per_interval() {
        let mut throttle = SnapshotThrottle::new(Duration::from_millis(500));
        let start = Instant::now();
        assert!(throttle.due(start));
        assert!(!throttle.due(start + Duration::from_millis(100)));
        assert!(!throttle.due(start + Duration::from_millis(499)));
        assert!(throttle.due(start + Duration::from_millis(500)));
        assert!(!throttle.due(start + Duration::from_millis(600)));
        assert!(throttle.due(start + Duration::from_millis(1_000)));
    }

    #[test]
    fn throttle_remaining_is_none_before_first_write_and_counts_down() {
        let start = Instant::now();
        // Nothing to wait for until a write has been attempted.
        assert_eq!(
            SnapshotThrottle::new(Duration::from_millis(500)).remaining(start),
            None
        );
        let mut throttle = SnapshotThrottle::new(Duration::from_millis(500));
        assert!(throttle.due(start));
        // Exactly after a due write, the full interval remains.
        assert_eq!(throttle.remaining(start), Some(Duration::from_millis(500)));
        assert_eq!(
            throttle.remaining(start + Duration::from_millis(200)),
            Some(Duration::from_millis(300))
        );
        // At or past the interval the next write is permitted: no wait needed.
        assert_eq!(throttle.remaining(start + Duration::from_millis(500)), None);
        assert_eq!(throttle.remaining(start + Duration::from_millis(900)), None);
    }

    #[test]
    fn saved_permissions_are_owner_only_and_untrusted_modes_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(state.path());
        let record = RecoveryRecord::capture(
            root.path(),
            &root.path().join("doc.txt"),
            "text",
            RevisionRef {
                size: 1,
                modified_secs: 1,
                modified_nanos: 0,
                modified_known: true,
            },
            SystemTime::now(),
            MAX_RECOVERY_BYTES as usize,
        )
        .unwrap();
        let path = store.save(&record, &policy()).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir_mode = std::fs::metadata(store.records_dir())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            let file_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(dir_mode & 0o077, 0, "state dir must be owner-only");
            assert_eq!(file_mode & 0o077, 0, "record must be owner-only");

            // Group/other readable snapshot records are refused.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(matches!(
                store.load(&path),
                Err(RecoveryError::UnsafeRecord(_))
            ));
            // A group-readable state directory is refused too.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            let private_dir = store.records_dir();
            std::fs::set_permissions(&private_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(matches!(
                store.load(&path),
                Err(RecoveryError::UnsafeState(_))
            ));
            std::fs::set_permissions(&private_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(store.load(&path).is_ok());
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_storage_and_records_are_refused() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(state.path());
        let record = RecoveryRecord::capture(
            root.path(),
            &root.path().join("doc.txt"),
            "text",
            RevisionRef {
                size: 1,
                modified_secs: 1,
                modified_nanos: 0,
                modified_known: true,
            },
            SystemTime::now(),
            MAX_RECOVERY_BYTES as usize,
        )
        .unwrap();

        // A symlinked records directory is refused for both save and load.
        symlink(elsewhere.path(), store.records_dir()).unwrap();
        assert!(matches!(
            store.save(&record, &policy()),
            Err(RecoveryError::UnsafeState(_))
        ));
        let planted = store
            .records_dir()
            .join("snap-0000000000000000-0000000000000000-0000000000000000.json");
        std::fs::write(&planted, b"{}").unwrap();
        assert!(matches!(
            store.load(&planted),
            Err(RecoveryError::UnsafeState(_))
        ));
        std::fs::remove_file(store.records_dir()).unwrap();

        // A symlinked state path itself is refused for write and load/prune.
        let linked_state = state.path().join("linked-state");
        symlink(elsewhere.path(), &linked_state).unwrap();
        let linked = RecoveryStore::new(&linked_state);
        assert!(matches!(
            linked.save(&record, &policy()),
            Err(RecoveryError::UnsafeState(_))
        ));
        assert!(matches!(
            linked.prune(root.path(), &policy(), SystemTime::now()),
            Err(RecoveryError::UnsafeState(_))
        ));

        // A symlinked record file is refused without being followed.
        let path = store.save(&record, &policy()).unwrap();
        std::fs::remove_file(&path).unwrap();
        let target = state.path().join("target.json");
        std::fs::write(&target, serde_json::to_vec(&record).unwrap()).unwrap();
        symlink(&target, &path).unwrap();
        assert!(matches!(
            store.load(&path),
            Err(RecoveryError::UnsafeRecord(_))
        ));
    }

    #[test]
    fn saved_records_carry_only_reviewed_fields() {
        let record = RecoveryRecord::capture(
            Path::new("/workspace"),
            Path::new("/workspace/doc.txt"),
            "unsaved",
            RevisionRef {
                size: 3,
                modified_secs: 7,
                modified_nanos: 9,
                modified_known: true,
            },
            SystemTime::now(),
            MAX_RECOVERY_BYTES as usize,
        )
        .unwrap();
        let payload = serde_json::to_value(&record).unwrap();
        let mut keys: Vec<&str> = payload
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "captured_secs",
                "document_path",
                "revision",
                "schema",
                "text",
                "version",
                "workspace_root",
            ]
        );
        let mut revision_keys: Vec<&str> = payload["revision"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        revision_keys.sort_unstable();
        assert_eq!(
            revision_keys,
            ["modified_known", "modified_nanos", "modified_secs", "size"]
        );
    }

    #[test]
    fn policy_resolves_config_and_record_path_is_revision_keyed() {
        let mut config = AppConfig::default();
        config.recovery.max_records = Some(usize::MAX);
        config.recovery.max_age_secs = Some(0);
        let resolved = RecoveryPolicy::from_config(&config);
        assert!(resolved.enabled);
        assert_eq!(
            resolved.max_records,
            crate::config::MAX_RECOVERY_MAX_RECORDS
        );
        assert_eq!(
            resolved.max_age,
            Duration::from_secs(crate::config::MIN_RECOVERY_MAX_AGE_SECS)
        );

        let store = RecoveryStore::new("/state");
        let a = store.record_path(
            Path::new("/workspace"),
            Path::new("/workspace/doc.txt"),
            &RevisionRef {
                size: 1,
                modified_secs: 1,
                modified_nanos: 0,
                modified_known: true,
            },
        );
        let b = store.record_path(
            Path::new("/workspace"),
            Path::new("/workspace/doc.txt"),
            &RevisionRef {
                size: 2,
                modified_secs: 1,
                modified_nanos: 0,
                modified_known: true,
            },
        );
        assert_ne!(a, b, "different revisions must not collide");
        assert!(a.starts_with("/state/recovery"));
    }
}
