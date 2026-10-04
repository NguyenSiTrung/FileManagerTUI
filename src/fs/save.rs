//! Revision checks are advisory, not compare-and-swap: another writer can act
//! between final validation and rename. Parent directories must be trusted and
//! stable. Linux/macOS rename is supported; other platforms reject replacement.
//! File data is synced, but directory crash durability is not guaranteed.
//! Unix modes and matching ownership are preserved; files with ACLs or extended
//! attributes are refused instead of silently dropping them.
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

/// Exact bytes and filesystem identity of a consistently loaded/saved version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRevision {
    // Exact content fingerprint: no collisions, including same-size/timestamp edits.
    bytes: Arc<[u8]>,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64, u32, u32, u32, u64),
}

impl FileRevision {
    /// Compare a revision after a known successful rename, without acknowledging
    /// an existing conflict. Callers must retain pre-existing change flags.
    /// Ignores only Unix ctime; without filesystem identity, returns false.
    pub fn matches_after_known_rename(&self, other: &Self) -> bool {
        #[cfg(unix)]
        {
            self.bytes == other.bytes
                && self.modified == other.modified
                && (
                    self.identity.0,
                    self.identity.1,
                    self.identity.4,
                    self.identity.5,
                    self.identity.6,
                    self.identity.7,
                ) == (
                    other.identity.0,
                    other.identity.1,
                    other.identity.4,
                    other.identity.5,
                    other.identity.6,
                    other.identity.7,
                )
        }
        #[cfg(not(unix))]
        {
            // Exact bytes and mtime alone cannot establish filesystem identity.
            let _ = other;
            false
        }
    }
}

/// Non-destructive save failures that callers can present without losing a buffer.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("document {path} exceeds byte limit ({max_bytes} bytes)")]
    TooLarge { path: PathBuf, max_bytes: usize },
    #[error("file changed, replaced, or deleted: {0}")]
    Conflict(PathBuf),
    #[error("unsafe save target {path}: {reason}")]
    UnsafeTarget { path: PathBuf, reason: &'static str },
    #[error("destination already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("line endings or encoding require explicit normalization")]
    NormalizationRequired,
    #[error("safe replacement unsupported on this platform")]
    #[allow(dead_code)]
    UnsupportedReplacement,
    #[error("{operation} {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
#[allow(dead_code)]
impl SaveError {
    pub fn is_conflict(&self) -> bool {
        matches!(self, Self::Conflict(_))
    }
    pub fn requires_normalization(&self) -> bool {
        matches!(self, Self::NormalizationRequired)
    }
}
fn io(operation: &'static str, path: &Path, source: std::io::Error) -> SaveError {
    SaveError::Io {
        operation,
        path: path.into(),
        source,
    }
}
fn revision(bytes: &[u8], m: &Metadata) -> FileRevision {
    FileRevision {
        bytes: bytes.into(),
        modified: m.modified().ok(),
        #[cfg(unix)]
        identity: {
            use std::os::unix::fs::MetadataExt;
            (
                m.dev(),
                m.ino(),
                m.ctime(),
                m.ctime_nsec(),
                m.mode(),
                m.uid(),
                m.gid(),
                m.nlink(),
            )
        },
    }
}
fn reject_symlinks(path: &Path) -> Result<(), SaveError> {
    let absolute = if path.is_absolute() {
        path.into()
    } else {
        std::env::current_dir()
            .map_err(|e| io("resolve", path, e))?
            .join(path)
    };
    let mut prefix = PathBuf::new();
    for part in absolute.components() {
        prefix.push(part);
        match fs::symlink_metadata(&prefix) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(SaveError::UnsafeTarget {
                    path: path.into(),
                    reason: "symlinks are not supported",
                })
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(io("inspect", path, e)),
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn require_no_extended_attributes(file: &File, path: &Path) -> Result<(), SaveError> {
    use std::os::fd::AsRawFd;
    // SAFETY: The descriptor is owned by a live File. A null buffer with size zero
    // queries the required attribute-list length without accessing memory.
    #[cfg(target_os = "linux")]
    let count = unsafe { libc::flistxattr(file.as_raw_fd(), std::ptr::null_mut(), 0) };
    #[cfg(target_os = "macos")]
    let count = unsafe { libc::flistxattr(file.as_raw_fd(), std::ptr::null_mut(), 0, 0) };
    if count < 0 {
        return Err(io(
            "inspect extended permissions/attributes",
            path,
            std::io::Error::last_os_error(),
        ));
    }
    if count > 0 {
        return Err(SaveError::UnsafeTarget {
            path: path.into(),
            reason: "cannot preserve extended permissions/attributes; use Save As",
        });
    }
    Ok(())
}
/// Reads bytes and revision from the same open file under the default finite
/// legacy budget, validating the pathname. Callers with a known loaded revision
/// size should use [`load_document_bounded`] instead.
pub fn load_document(path: &Path) -> Result<(Vec<u8>, FileRevision), SaveError> {
    load_document_with_bound(path, DEFAULT_LEGACY_LOAD_BUDGET_BYTES)
}

/// Read at most the byte limit plus one detection byte while retaining the
/// same descriptor, symlink checks, and pathname/revision validation.
#[allow(dead_code)]
pub fn load_document_bounded(
    path: &Path,
    max_bytes: usize,
) -> Result<(Vec<u8>, FileRevision), SaveError> {
    load_document_with_bound(path, max_bytes)
}

fn load_document_with_bound(
    path: &Path,
    max_bytes: usize,
) -> Result<(Vec<u8>, FileRevision), SaveError> {
    reject_symlinks(path)?;
    if !fs::symlink_metadata(path)
        .map_err(|e| io("inspect", path, e))?
        .is_file()
    {
        return Err(SaveError::UnsafeTarget {
            path: path.into(),
            reason: "not a regular file",
        });
    }
    let mut f = File::open(path).map_err(|e| io("open", path, e))?;
    let before = f.metadata().map_err(|e| io("metadata", path, e))?;
    if !before.is_file() {
        return Err(SaveError::UnsafeTarget {
            path: path.into(),
            reason: "not a regular file",
        });
    }
    if before.len() > max_bytes as u64 {
        return Err(SaveError::TooLarge {
            path: path.into(),
            max_bytes,
        });
    }
    let bytes = read_document_bytes(&mut f, path, max_bytes)?;
    let after = f.metadata().map_err(|e| io("metadata", path, e))?;
    reject_symlinks(path)?;
    let named = fs::metadata(path).map_err(|e| io("metadata", path, e))?;
    let r = revision(&bytes, &before);
    if r != revision(&bytes, &after) || r != revision(&bytes, &named) {
        return Err(SaveError::Conflict(path.into()));
    }
    Ok((bytes, r))
}

fn read_document_bytes(
    reader: impl Read,
    path: &Path,
    max_bytes: usize,
) -> Result<Vec<u8>, SaveError> {
    let mut bytes = Vec::new();
    // A single detection byte makes growth during the read an explicit refusal
    // instead of a silently clipped revision.
    reader
        .take((max_bytes as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| io("read", path, error))?;
    if bytes.len() > max_bytes {
        return Err(SaveError::TooLarge {
            path: path.into(),
            max_bytes,
        });
    }
    Ok(bytes)
}

struct Temp(PathBuf, bool);
impl Drop for Temp {
    fn drop(&mut self) {
        if self.1 {
            let _ = fs::remove_file(&self.0);
        }
    }
}
static NEXT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Write,
    Flush,
    Sync,
    Permissions,
    Validate,
    Replace,
}
#[cfg(test)]
thread_local! { static FAIL: std::cell::Cell<Option<Stage>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
thread_local! {
    static VALIDATION_CHANGE: std::cell::RefCell<Option<Vec<u8>>> = const {
        std::cell::RefCell::new(None)
    };
}
#[cfg(test)]
pub(crate) fn inject_failure(stage: Stage) {
    FAIL.with(|f| f.set(Some(stage)));
}
fn checkpoint(stage: Stage, path: &Path) -> Result<(), SaveError> {
    #[cfg(test)]
    if stage == Stage::Validate {
        VALIDATION_CHANGE.with(|change| {
            if let Some(bytes) = change.borrow_mut().take() {
                fs::write(path, bytes).map_err(|error| io("external test edit", path, error))?;
            }
            Ok::<(), SaveError>(())
        })?;
    }
    #[cfg(test)]
    if FAIL.with(|f| {
        if f.get() == Some(stage) {
            f.set(None);
            true
        } else {
            false
        }
    }) {
        return Err(io(
            "injected save failure",
            path,
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "injected"),
        ));
    }
    let _ = (stage, path);
    Ok(())
}
/// None means create-only, never overwrite. Some means revision-checked replace.
pub fn save_document(
    path: &Path,
    bytes: &[u8],
    expected_revision: Option<&FileRevision>,
) -> Result<FileRevision, SaveError> {
    let limit = expected_revision.map_or(0, |revision| revision.bytes.len());
    save_document_bounded(path, bytes, expected_revision, limit)
}

fn validation_revision(
    path: &Path,
    expected: &FileRevision,
    max_validation_bytes: usize,
) -> Result<FileRevision, SaveError> {
    if expected.bytes.len() > max_validation_bytes {
        return Err(SaveError::Conflict(path.into()));
    }
    load_document_bounded(path, expected.bytes.len())
        .map(|(_, revision)| revision)
        .map_err(|error| match error {
            SaveError::TooLarge { .. } => SaveError::Conflict(path.into()),
            SaveError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
                SaveError::Conflict(path.into())
            }
            other => other,
        })
}

/// Shared safe publication with an explicit budget for target validation reads.
/// Both reads are also capped at the known revision size; growth is a conflict.
/// A read may consume one extra byte to detect growth during reading.
/// The budget does not limit output bytes, which callers must admit separately.
pub fn save_document_bounded(
    path: &Path,
    bytes: &[u8],
    expected_revision: Option<&FileRevision>,
    max_validation_bytes: usize,
) -> Result<FileRevision, SaveError> {
    reject_symlinks(path)?;
    let original: Option<Metadata> = if let Some(expected) = expected_revision {
        let actual = validation_revision(path, expected, max_validation_bytes)?;
        if &actual != expected {
            return Err(SaveError::Conflict(path.into()));
        }
        let m = fs::metadata(path).map_err(|e| io("metadata", path, e))?;
        if m.permissions().readonly() {
            return Err(SaveError::UnsafeTarget {
                path: path.into(),
                reason: "read-only file",
            });
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if m.mode() & 0o200 == 0 {
                return Err(SaveError::UnsafeTarget {
                    path: path.into(),
                    reason: "owner-read-only file",
                });
            }
            if m.nlink() != 1 {
                return Err(SaveError::UnsafeTarget {
                    path: path.into(),
                    reason: "hard-linked file",
                });
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        return Err(SaveError::UnsupportedReplacement);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let original = File::open(path).map_err(|error| io("open", path, error))?;
            require_no_extended_attributes(&original, path)?;
            Some(m)
        }
    } else {
        if fs::symlink_metadata(path).is_ok() {
            return Err(SaveError::AlreadyExists(path.into()));
        }
        None
    };
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let (mut temp, mut file) = loop {
        let name = parent.join(format!(
            ".fm-save-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&name) {
            Ok(f) => break (Temp(name, true), f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io("create temporary", path, e)),
        }
    };
    checkpoint(Stage::Write, path)?;
    file.write_all(bytes)
        .map_err(|e| io("write temporary", path, e))?;
    checkpoint(Stage::Flush, path)?;
    file.flush().map_err(|e| io("flush temporary", path, e))?;
    checkpoint(Stage::Permissions, path)?;
    if let Some(m) = original {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        require_no_extended_attributes(&file, path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let t = file
                .metadata()
                .map_err(|e| io("metadata temporary", path, e))?;
            if (m.uid(), m.gid()) != (t.uid(), t.gid()) {
                return Err(SaveError::UnsafeTarget {
                    path: path.into(),
                    reason: "cannot preserve ownership",
                });
            }
        }
        file.set_permissions(m.permissions())
            .map_err(|e| io("permissions temporary", path, e))?;
    }
    checkpoint(Stage::Sync, path)?;
    file.sync_all().map_err(|e| io("sync temporary", path, e))?;
    let saved = revision(
        bytes,
        &file
            .metadata()
            .map_err(|e| io("metadata temporary", path, e))?,
    );
    checkpoint(Stage::Validate, path)?;
    reject_symlinks(path)?;
    if let Some(expected) = expected_revision {
        let actual = validation_revision(path, expected, max_validation_bytes)?;
        if &actual != expected {
            return Err(SaveError::Conflict(path.into()));
        }
    }
    checkpoint(Stage::Replace, path)?;
    if expected_revision.is_some() {
        fs::rename(&temp.0, path).map_err(|e| io("replace", path, e))?;
        // The temporary name is no longer ours after successful rename.
        temp.1 = false;
    } else {
        // Atomic no-clobber publication; temp and destination share an inode until cleanup.
        fs::hard_link(&temp.0, path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                SaveError::AlreadyExists(path.into())
            } else {
                io("publish new file", path, e)
            }
        })?;
    }
    drop(temp);
    // Rename/link changes ctime; capture resulting identity without re-reading bytes.
    // A post-commit metadata failure must not turn a committed save into a failure.
    Ok(file
        .metadata()
        .map(|m| revision(bytes, &m))
        .unwrap_or(saved))
}
/// Default finite byte budget for legacy load/overwrite paths that cannot know
/// the configured admission limit. It mirrors the editor admission default so an
/// ordinary explicit overwrite stays useful; a target grown beyond it is refused
/// rather than read without bound.
pub const DEFAULT_LEGACY_LOAD_BUDGET_BYTES: usize =
    crate::config::DEFAULT_MAX_EDITOR_BYTES as usize;

/// Explicit user confirmation bypasses old revision, but never unsafe-target policy.
#[allow(dead_code)]
pub fn overwrite_document(path: &Path, bytes: &[u8]) -> Result<FileRevision, SaveError> {
    overwrite_document_bounded(path, bytes, DEFAULT_LEGACY_LOAD_BUDGET_BYTES)
}

/// Explicit user confirmation that captures the target revision under the
/// explicit `max_bytes` budget. A target grown beyond the budget must surface as
/// a conflict: it is never clipped, truncated, or partially published.
#[allow(dead_code)]
pub fn overwrite_document_bounded(
    path: &Path,
    bytes: &[u8],
    max_bytes: usize,
) -> Result<FileRevision, SaveError> {
    match load_document_bounded(path, max_bytes) {
        Ok((_, revision)) => save_document_bounded(path, bytes, Some(&revision), max_bytes),
        // Growth beyond the capture budget is a conflict the user can act on,
        // never a clipped read used as an overwrite revision.
        Err(SaveError::TooLarge { .. }) => Err(SaveError::Conflict(path.into())),
        Err(SaveError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            save_document(path, bytes, None)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_save_refuses_a_baseline_beyond_its_validation_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, b"original").unwrap();
        let expected = load_document_bounded(&path, 8).unwrap().1;
        assert!(matches!(
            save_document_bounded(&path, b"new", Some(&expected), 7),
            Err(SaveError::Conflict(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn bounded_save_validation_budget_is_independent_of_output_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, b"x").unwrap();
        let expected = load_document_bounded(&path, 1).unwrap().1;
        let saved = save_document_bounded(&path, b"larger", Some(&expected), 1).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"larger");
        assert!(matches!(
            save_document_bounded(&path, b"x", Some(&saved), 1),
            Err(SaveError::Conflict(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), b"larger");
    }
    #[test]
    fn bounded_save_initial_target_growth_is_conflict_not_truncated_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, b"x").unwrap();
        let expected = load_document_bounded(&path, 1).unwrap().1;
        fs::write(&path, b"externally grown").unwrap();
        assert!(matches!(
            save_document_bounded(&path, b"ours", Some(&expected), 1),
            Err(SaveError::Conflict(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), b"externally grown");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn bounded_save_final_target_growth_retains_external_bytes_and_cleans_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, b"x").unwrap();
        let expected = load_document_bounded(&path, 1).unwrap().1;
        VALIDATION_CHANGE.with(|change| {
            *change.borrow_mut() = Some(b"externally grown".to_vec());
        });
        assert!(matches!(
            save_document_bounded(&path, b"ours", Some(&expected), 1),
            Err(SaveError::Conflict(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), b"externally grown");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn legacy_load_document_refuses_a_target_beyond_the_default_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grown");
        // Sparse file: exceeds the default budget without writing 10 MiB.
        let file = File::create(&path).unwrap();
        file.set_len(DEFAULT_LEGACY_LOAD_BUDGET_BYTES as u64 + 1)
            .unwrap();
        drop(file);
        assert!(matches!(
            load_document(&path),
            Err(SaveError::TooLarge { max_bytes, .. })
                if max_bytes == DEFAULT_LEGACY_LOAD_BUDGET_BYTES
        ));
        assert_eq!(
            fs::metadata(&path).unwrap().len(),
            DEFAULT_LEGACY_LOAD_BUDGET_BYTES as u64 + 1
        );
    }
    #[test]
    fn bounded_overwrite_growth_beyond_budget_is_conflict_without_publication() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, b"small").unwrap();
        fs::write(&path, b"grown well beyond the tiny budget").unwrap();
        let original = fs::read(&path).unwrap();
        assert!(matches!(
            overwrite_document_bounded(&path, b"ours", 8),
            Err(SaveError::Conflict(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn bounded_overwrite_exact_budget_and_shrunk_target_publish_new_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, b"12345678").unwrap();
        overwrite_document_bounded(&path, b"new", 8).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        // A target that shrank after capture is replaced by an explicit overwrite.
        fs::write(&path, b"1").unwrap();
        overwrite_document_bounded(&path, b"replacement", 8).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn bounded_overwrite_refuses_directory_replacement_without_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("target");
        fs::write(&path, b"original").unwrap();
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(matches!(
            overwrite_document_bounded(&path, b"ours", 8),
            Err(SaveError::UnsafeTarget { .. })
        ));
        assert!(path.is_dir());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[cfg(unix)]
    #[test]
    fn bounded_overwrite_refuses_symlink_replacement_and_preserves_target() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        let link = dir.path().join("link");
        fs::write(&real, b"original").unwrap();
        symlink(&real, &link).unwrap();
        assert!(matches!(
            overwrite_document_bounded(&link, b"ours", 8),
            Err(SaveError::UnsafeTarget { .. })
        ));
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(&real).unwrap(), b"original");
    }
    #[test]
    fn bounded_reader_surfaces_unreadable_target_without_partial_bytes() {
        struct Unreadable;
        impl Read for Unreadable {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected unreadable target",
                ))
            }
        }
        let result = read_document_bytes(Unreadable, Path::new("unreadable"), 16);
        assert!(matches!(
            result,
            Err(SaveError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::PermissionDenied
        ));
    }
    #[test]
    fn bounded_save_shrunk_target_after_capture_is_conflict_without_clipping() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, b"original").unwrap();
        let expected = load_document_bounded(&path, 8).unwrap().1;
        VALIDATION_CHANGE.with(|change| {
            *change.borrow_mut() = Some(b"short".to_vec());
        });
        assert!(matches!(
            save_document_bounded(&path, b"ours", Some(&expected), 8),
            Err(SaveError::Conflict(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), b"short");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn bounded_document_reader_stops_after_limit_plus_detection_byte() {
        let mut reader = std::io::Cursor::new(b"1234567890");
        let result = read_document_bytes(&mut reader, Path::new("growing"), 4);
        assert!(
            reader.position() <= 5,
            "Read past the bounded detection byte"
        );
        assert!(matches!(
            result,
            Err(SaveError::TooLarge { max_bytes: 4, .. })
        ));
    }
    #[test]
    fn bounded_document_load_refuses_oversize_without_touching_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large");
        fs::write(&path, b"12345678").unwrap();
        assert!(matches!(
            load_document_bounded(&path, 4),
            Err(SaveError::TooLarge { max_bytes: 4, .. })
        ));
        assert_eq!(fs::read(&path).unwrap(), b"12345678");
    }

    #[test]
    fn bounded_document_load_preserves_exact_limit_revision_and_empty_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text");
        fs::write(&path, b"abcd").unwrap();
        let bounded = load_document_bounded(&path, 4).unwrap();
        assert_eq!(bounded, load_document(&path).unwrap());
        fs::write(&path, b"").unwrap();
        assert_eq!(
            load_document_bounded(&path, 0).unwrap(),
            load_document(&path).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn bounded_document_load_retains_symlink_and_non_file_refusals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text");
        let link = dir.path().join("link");
        fs::write(&path, b"abcd").unwrap();
        std::os::unix::fs::symlink(&path, &link).unwrap();
        for unsafe_path in [&link, dir.path()] {
            assert!(matches!(
                load_document_bounded(unsafe_path, 4),
                Err(SaveError::UnsafeTarget { .. })
            ));
        }
    }
    #[test]
    fn exact_fingerprint_detects_same_metadata_different_bytes() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("file");
        fs::write(&p, b"aa").unwrap();
        let m = fs::metadata(&p).unwrap();
        assert_ne!(revision(b"aa", &m), revision(b"bb", &m));
    }
    #[cfg(unix)]
    #[test]
    fn known_rename_revision_ignores_only_ctime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, b"aa").unwrap();
        let original = load_document_bounded(&path, 2).unwrap().1;
        let mut renamed = original.clone();
        // Deterministic rename metadata, independent of filesystem clock precision.
        renamed.identity.2 = renamed.identity.2.wrapping_add(1);
        renamed.identity.3 = (renamed.identity.3 + 1) % 1_000_000_000;
        assert_ne!(original, renamed);
        assert!(original.matches_after_known_rename(&renamed));
        assert!(renamed.matches_after_known_rename(&original));
    }
    #[cfg(unix)]
    #[test]
    fn known_rename_revision_preserves_bytes_and_all_other_metadata_checks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, b"aa").unwrap();
        let original = load_document_bounded(&path, 2).unwrap().1;
        let mut bytes_changed = original.clone();
        bytes_changed.bytes = Arc::from(&b"bb"[..]);
        let mut time_changed = original.clone();
        time_changed.modified = Some(SystemTime::UNIX_EPOCH);
        let mut device_changed = original.clone();
        device_changed.identity.0 = device_changed.identity.0.wrapping_add(1);
        let mut inode_changed = original.clone();
        inode_changed.identity.1 = inode_changed.identity.1.wrapping_add(1);
        let mut mode_changed = original.clone();
        mode_changed.identity.4 ^= 0o100;
        let mut owner_changed = original.clone();
        owner_changed.identity.5 = owner_changed.identity.5.wrapping_add(1);
        let mut group_changed = original.clone();
        group_changed.identity.6 = group_changed.identity.6.wrapping_add(1);
        let mut links_changed = original.clone();
        links_changed.identity.7 = links_changed.identity.7.wrapping_add(1);
        for changed in [
            bytes_changed,
            time_changed,
            device_changed,
            inode_changed,
            mode_changed,
            owner_changed,
            group_changed,
            links_changed,
        ] {
            assert!(!original.matches_after_known_rename(&changed));
        }
    }
    #[cfg(unix)]
    #[test]
    fn known_rename_revision_matches_real_rename_and_rejects_inode_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old");
        let new = dir.path().join("new");
        fs::write(&old, b"aa").unwrap();
        let original = load_document_bounded(&old, 2).unwrap().1;
        fs::rename(&old, &new).unwrap();
        let renamed = load_document_bounded(&new, 2).unwrap().1;
        assert!(original.matches_after_known_rename(&renamed));
        let replacement = dir.path().join("replacement");
        fs::write(&replacement, b"aa").unwrap();
        fs::rename(&replacement, &new).unwrap();
        let replaced = load_document_bounded(&new, 2).unwrap().1;
        assert!(!original.matches_after_known_rename(&replaced));
        assert_eq!(fs::read(&new).unwrap(), b"aa");
    }
    #[cfg(not(unix))]
    #[test]
    fn known_rename_revision_refuses_to_infer_missing_filesystem_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, b"aa").unwrap();
        let original = load_document_bounded(&path, 2).unwrap().1;
        assert!(!original.matches_after_known_rename(&original));
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn replacement_refuses_to_drop_extended_permissions_or_attributes() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("protected");
        fs::write(&path, b"original").unwrap();
        let name = CString::new(path.as_os_str().as_bytes()).unwrap();
        let attribute = c"user.fm-save-test";
        // SAFETY: C strings and the value buffer remain valid for this syscall.
        let result = unsafe {
            libc::setxattr(
                name.as_ptr(),
                attribute.as_ptr(),
                b"keep".as_ptr().cast(),
                4,
                0,
            )
        };
        assert_eq!(result, 0, "{}", std::io::Error::last_os_error());
        let expected = load_document(&path).unwrap().1;
        assert!(matches!(
            save_document(&path, b"changed", Some(&expected)),
            Err(SaveError::UnsafeTarget { .. })
        ));
        assert_eq!(fs::read(&path).unwrap(), b"original");
        let mut value = [0u8; 4];
        // SAFETY: The C strings and the writable buffer have valid lengths.
        let count = unsafe {
            libc::getxattr(
                name.as_ptr(),
                attribute.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
            )
        };
        assert_eq!(count, 4);
        assert_eq!(&value, b"keep");
    }
    #[test]
    fn exclusive_creation_and_second_save() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("file");
        let r = save_document(&p, b"a", None).unwrap();
        assert!(matches!(
            save_document(&p, b"b", None),
            Err(SaveError::AlreadyExists(_))
        ));
        #[cfg(unix)]
        {
            let r = save_document(&p, b"b", Some(&r)).unwrap();
            save_document(&p, b"c", Some(&r)).unwrap();
            assert_eq!(fs::read(&p).unwrap(), b"c");
        }
        #[cfg(not(unix))]
        assert!(matches!(
            save_document(&p, b"b", Some(&r)),
            Err(SaveError::UnsupportedReplacement)
        ));
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1);
    }
    #[test]
    fn deletion_and_external_edit_are_conflicts() {
        for delete in [false, true] {
            let d = tempfile::tempdir().unwrap();
            let p = d.path().join("file");
            fs::write(&p, b"aa").unwrap();
            let r = load_document(&p).unwrap().1;
            if delete {
                fs::remove_file(&p).unwrap();
            } else {
                fs::write(&p, b"bb").unwrap();
            }
            assert!(save_document(&p, b"cc", Some(&r))
                .unwrap_err()
                .is_conflict());
            if delete {
                assert!(!p.exists());
            } else {
                assert_eq!(fs::read(&p).unwrap(), b"bb");
            }
        }
    }
    #[cfg(unix)]
    #[test]
    fn identical_bytes_inode_replacement_is_conflict() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("file");
        let q = d.path().join("other");
        fs::write(&p, b"a").unwrap();
        let r = load_document(&p).unwrap().1;
        fs::write(&q, b"a").unwrap();
        fs::rename(q, &p).unwrap();
        assert!(save_document(&p, b"b", Some(&r)).unwrap_err().is_conflict());
        assert_eq!(fs::read(&p).unwrap(), b"a");
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_retargeting_and_hardlinks_are_refused_even_when_confirmed() {
        use std::os::unix::fs::symlink;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("file");
        let q = d.path().join("link");
        fs::write(&p, b"a").unwrap();
        symlink(&p, &q).unwrap();
        assert!(matches!(
            overwrite_document(&q, b"x"),
            Err(SaveError::UnsafeTarget { .. })
        ));
        let r = load_document(&p).unwrap().1;
        fs::remove_file(&p).unwrap();
        symlink(&q, &p).unwrap();
        assert!(matches!(
            save_document(&p, b"x", Some(&r)),
            Err(SaveError::UnsafeTarget { .. })
        ));
        fs::remove_file(&p).unwrap();
        fs::remove_file(&q).unwrap();
        fs::write(&p, b"a").unwrap();
        fs::hard_link(&p, &q).unwrap();
        assert!(matches!(
            overwrite_document(&p, b"x"),
            Err(SaveError::UnsafeTarget { .. })
        ));
        assert_eq!(fs::read(q).unwrap(), b"a");
    }
    #[cfg(unix)]
    #[test]
    fn preserves_modes_and_refuses_readonly_even_as_root() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("file");
        fs::write(&p, b"a").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o751)).unwrap();
        let r = load_document(&p).unwrap().1;
        save_document(&p, b"b", Some(&r)).unwrap();
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o751
        );
        fs::set_permissions(&p, fs::Permissions::from_mode(0o444)).unwrap();
        assert!(matches!(
            overwrite_document(&p, b"x"),
            Err(SaveError::UnsafeTarget { .. })
        ));
        assert_eq!(fs::read(&p).unwrap(), b"b");
    }
    #[cfg(unix)]
    #[test]
    fn owner_readonly_is_refused_despite_group_write_permission() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner-readonly");
        fs::write(&path, b"original").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o460)).unwrap();
        let expected = load_document(&path).unwrap().1;
        assert!(matches!(
            save_document(&path, b"changed", Some(&expected)),
            Err(SaveError::UnsafeTarget { .. })
        ));
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o460
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn refuses_inherited_acl_on_temporary_before_restoring_mode() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("protected");
        fs::write(&path, b"original").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let expected = load_document(&path).unwrap().1;
        let mut acl = 2u32.to_le_bytes().to_vec();
        for (tag, permission, id) in [
            (1u16, 7u16, u32::MAX),
            (2, 4, 42424),
            (4, 5, u32::MAX),
            (16, 5, u32::MAX),
            (32, 0, u32::MAX),
        ] {
            acl.extend(tag.to_le_bytes());
            acl.extend(permission.to_le_bytes());
            acl.extend(id.to_le_bytes());
        }
        let directory = CString::new(dir.path().as_os_str().as_bytes()).unwrap();
        // SAFETY: The C strings and serialized Linux POSIX ACL buffer are valid.
        let result = unsafe {
            libc::setxattr(
                directory.as_ptr(),
                c"system.posix_acl_default".as_ptr(),
                acl.as_ptr().cast(),
                acl.len(),
                0,
            )
        };
        assert_eq!(result, 0, "{}", std::io::Error::last_os_error());
        assert!(matches!(
            save_document(&path, b"changed", Some(&expected)),
            Err(SaveError::UnsafeTarget { .. })
        ));
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[cfg(unix)]
    #[test]
    fn injected_failures_keep_original_and_cleanup_only_own_temp() {
        for stage in [
            Stage::Write,
            Stage::Flush,
            Stage::Sync,
            Stage::Permissions,
            Stage::Validate,
            Stage::Replace,
        ] {
            let d = tempfile::tempdir().unwrap();
            let p = d.path().join("file");
            fs::write(&p, b"original").unwrap();
            fs::write(d.path().join(".fm-save-unrelated"), b"keep").unwrap();
            let r = load_document(&p).unwrap().1;
            inject_failure(stage);
            assert!(matches!(
                save_document(&p, b"changed", Some(&r)),
                Err(SaveError::Io { .. })
            ));
            assert_eq!(fs::read(&p).unwrap(), b"original");
            assert_eq!(fs::read_dir(d.path()).unwrap().count(), 2);
        }
    }
}
