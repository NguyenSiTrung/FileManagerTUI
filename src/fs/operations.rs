use std::fs;
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};

use crate::error::Result;

/// Create an empty file exclusively, refusing any existing directory entry.
#[allow(dead_code)]
pub fn create_file(path: &Path) -> Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|cause| {
            Error::new(
                cause.kind(),
                format!("create file '{}': {cause}", path.display()),
            )
        })?;
    Ok(())
}

/// Create a new directory at the given path.
#[allow(dead_code)]
pub fn create_dir(path: &Path) -> Result<()> {
    fs::create_dir(path).map_err(|cause| {
        Error::new(
            cause.kind(),
            format!("create directory '{}': {cause}", path.display()),
        )
    })?;
    Ok(())
}

/// Rename a directory entry without replacing any existing destination.
///
/// Linux and macOS use native atomic no-replace primitives. Other platforms,
/// or filesystems/kernels that do not support the primitive, fail safely;
/// there is deliberately no check-then-rename or copy/delete fallback.
#[allow(dead_code)]
pub fn rename(from: &Path, to: &Path) -> Result<()> {
    rename_no_replace(from, to).map_err(|cause| {
        Error::new(
            cause.kind(),
            format!("rename '{}' to '{}': {cause}", from.display(), to.display()),
        )
    })?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let from = CString::new(from.as_os_str().as_bytes())
        .map_err(|cause| Error::new(ErrorKind::InvalidInput, cause))?;
    let to = CString::new(to.as_os_str().as_bytes())
        .map_err(|cause| Error::new(ErrorKind::InvalidInput, cause))?;

    // SAFETY: both arguments are live NUL-terminated strings. No pointers
    // escape the call. The native flag makes destination validation and the
    // rename one atomic operation, including dangling symlink destinations.
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };

    if result == 0 {
        Ok(())
    } else {
        Err(Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn rename_no_replace(_from: &Path, _to: &Path) -> std::io::Result<()> {
    Err(Error::new(
        ErrorKind::Unsupported,
        "atomic no-replace rename is not supported on this platform",
    ))
}

/// Delete a file or directory. Directories are removed recursively.
#[allow(dead_code)]
pub fn delete(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// Progress callback for recursive delete operations.
pub type DeleteProgressFn = Box<dyn Fn(&str, usize) + Send>;

/// Recursively delete a file or directory with progress reporting and cancellation.
///
/// For files: simply deletes the file.
/// For directories: walks the tree, collecting all files first, then deletes
/// bottom-up (files, then empty dirs).
///
/// - `progress_fn`: called with `(current_file_name, items_deleted_so_far)`
/// - `cancel`: checked between each file deletion; if set, stops early
///
/// Returns `(deleted_count, errors)`.
#[allow(dead_code)]
pub fn delete_recursive_with_progress(
    path: &Path,
    progress_fn: &DeleteProgressFn,
    cancel: &std::sync::atomic::AtomicBool,
) -> (usize, Vec<String>) {
    use std::sync::atomic::Ordering;

    let mut deleted = 0;
    let mut errors = Vec::new();

    if cancel.load(Ordering::Relaxed) {
        return (deleted, errors);
    }

    let root_meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) => {
            errors.push(format!("{}: {}", path.display(), e));
            return (deleted, errors);
        }
    };

    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        // Simple file delete
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        progress_fn(&name, 0);
        match fs::remove_file(path) {
            Ok(()) => deleted += 1,
            Err(e) => errors.push(format!("{}: {}", path.display(), e)),
        }
        return (deleted, errors);
    }

    // Collect all entries bottom-up (files first, then dirs)
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut stack = vec![path.to_path_buf()];

    let mut visited = crate::fs::tree::VisitedDirs::new();
    visited.visit(path);

    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return (deleted, errors);
        }
        dirs.push(dir.clone());
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                errors.push(format!("{}: {}", dir.display(), e));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    errors.push(format!("read_dir entry: {}", e));
                    continue;
                }
            };
            let entry_path = entry.path();
            let meta = match fs::symlink_metadata(&entry_path) {
                Ok(m) => m,
                Err(e) => {
                    errors.push(format!("{}: {}", entry_path.display(), e));
                    continue;
                }
            };
            if meta.file_type().is_symlink() {
                // Never recurse into symlinks; treat them as leaf delete targets.
                files.push(entry_path);
            } else if meta.is_dir() {
                // Skip symlink loops
                if visited.visit(&entry_path) {
                    stack.push(entry_path);
                }
            } else {
                files.push(entry_path);
            }
        }
    }

    // Delete files first
    for file in &files {
        if cancel.load(Ordering::Relaxed) {
            return (deleted, errors);
        }
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        progress_fn(&name, deleted);
        match fs::remove_file(file) {
            Ok(()) => deleted += 1,
            Err(e) => errors.push(format!("{}: {}", file.display(), e)),
        }
    }

    // Delete directories bottom-up (deepest first)
    dirs.reverse();
    for dir in &dirs {
        if cancel.load(Ordering::Relaxed) {
            return (deleted, errors);
        }
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        progress_fn(&name, deleted);
        match fs::remove_dir(dir) {
            Ok(()) => deleted += 1,
            Err(e) => errors.push(format!("{}: {}", dir.display(), e)),
        }
    }

    (deleted, errors)
}

/// Resolve a name collision by appending `_copy`, `_copy2`, etc.
///
/// Returns a path that does not exist yet in the destination directory.
pub fn resolve_collision(dest: &Path) -> PathBuf {
    if !dest.exists() {
        return dest.to_path_buf();
    }

    let parent = dest.parent().unwrap_or(Path::new("."));
    let stem = dest
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = dest.extension().map(|e| e.to_string_lossy().to_string());

    // Try _copy, _copy2, _copy3, ...
    for i in 1..=1000 {
        let suffix = if i == 1 {
            "_copy".to_string()
        } else {
            format!("_copy{}", i)
        };
        let new_name = match &ext {
            Some(e) => format!("{}{}.{}", stem, suffix, e),
            None => format!("{}{}", stem, suffix),
        };
        let candidate = parent.join(&new_name);
        if !candidate.exists() {
            return candidate;
        }
    }

    // Fallback: should not happen in practice
    dest.to_path_buf()
}

/// Recursively copy a file or directory from `src` to `dest_dir`.
///
/// Returns the final path of the copied item (with collision resolution).
pub(crate) struct TransferPolicy<'a> {
    pub(crate) stopped: &'a dyn Fn() -> bool,
    pub(crate) max_entries: usize,
}

pub(crate) struct TransferReceipt {
    pub(crate) destination: Option<PathBuf>,
    pub(crate) completed_entries: usize,
    /// True only after the source was actually removed by a move.
    pub(crate) moved: bool,
    pub(crate) result: Result<()>,
}

/// Cooperative boundaries: metadata, each entry, file chunks and source removal.
/// Native syscalls themselves cannot be safely preempted.
pub(crate) fn transfer_with_policy(
    src: &Path,
    dest_dir: &Path,
    cut: bool,
    policy: TransferPolicy<'_>,
) -> TransferReceipt {
    let mut receipt = TransferReceipt {
        destination: None,
        completed_entries: 0,
        moved: false,
        result: Ok(()),
    };
    let mut visited = 0;
    let result = (|| -> Result<()> {
        check_transfer(&policy)?;
        if policy.max_entries == 0 {
            return Err(Error::other("operation entry budget").into());
        }
        let name = src
            .file_name()
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "no filename"))?;
        let dest = resolve_collision(&dest_dir.join(name));
        let meta = fs::symlink_metadata(src)?;
        if meta.file_type().is_symlink() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!("Refusing to copy/move symlink source: {}", src.display()),
            )
            .into());
        }
        if meta.is_dir() {
            ensure_not_descendant(src, &dest)?;
        }
        check_transfer(&policy)?;
        if cut && fs::rename(src, &dest).is_ok() {
            receipt.destination = Some(dest);
            receipt.completed_entries = 1;
            receipt.moved = true;
            return Ok(());
        }
        copy_transfer(src, &dest, &policy, &mut visited, &mut receipt, 0)?;
        if cut {
            check_transfer(&policy)?;
            if meta.is_dir() {
                fs::remove_dir_all(src)?;
            } else {
                fs::remove_file(src)?;
            }
            receipt.moved = true;
        }
        Ok(())
    })();
    receipt.result = result;
    receipt
}

fn check_transfer(policy: &TransferPolicy<'_>) -> Result<()> {
    if (policy.stopped)() {
        Err(Error::new(ErrorKind::Interrupted, "operation interrupted/deadline").into())
    } else {
        Ok(())
    }
}

fn copy_transfer(
    src: &Path,
    dest: &Path,
    policy: &TransferPolicy<'_>,
    visited: &mut usize,
    receipt: &mut TransferReceipt,
    depth: usize,
) -> Result<()> {
    use std::io::{Read, Write};
    check_transfer(policy)?;
    let bounded = policy.max_entries != usize::MAX;
    if *visited == policy.max_entries
        || (bounded
            && (depth > 128 || src.as_os_str().len() > 4096 || dest.as_os_str().len() > 4096))
    {
        return Err(Error::other("operation entry/depth/path budget").into());
    }
    *visited += 1;
    let meta = fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("Refusing to copy symlink entry: {}", src.display()),
        )
        .into());
    }
    check_transfer(policy)?;
    if meta.is_dir() {
        fs::create_dir_all(dest)?;
        if depth == 0 {
            receipt.destination = Some(dest.to_path_buf());
        }
        for entry in fs::read_dir(src)? {
            check_transfer(policy)?;
            let entry = entry?;
            // Check before allocating child paths. No unbounded traversal stack.
            if bounded
                && (src
                    .as_os_str()
                    .len()
                    .saturating_add(entry.file_name().len())
                    > 4094
                    || dest
                        .as_os_str()
                        .len()
                        .saturating_add(entry.file_name().len())
                        > 4094)
            {
                return Err(Error::other("operation path budget").into());
            }
            copy_transfer(
                &entry.path(),
                &dest.join(entry.file_name()),
                policy,
                visited,
                receipt,
                depth + 1,
            )?;
        }
    } else if !bounded {
        fs::copy(src, dest)?;
        if depth == 0 {
            receipt.destination = Some(dest.to_path_buf());
        }
    } else {
        if !meta.is_file() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "bounded transfer requires a regular file",
            )
            .into());
        }
        let mut input = fs::File::open(src)?;
        let mut output = fs::File::create(dest)?;
        if depth == 0 {
            receipt.destination = Some(dest.to_path_buf());
        }
        // Heap-only leaf buffer: recursive directory frames must not reserve
        // 64 KiB each on the blocking worker's finite stack.
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            check_transfer(policy)?;
            let len = input.read(&mut buffer)?;
            if len == 0 {
                break;
            }
            output.write_all(&buffer[..len])?;
        }
        fs::set_permissions(dest, meta.permissions())?;
    }
    receipt.completed_entries = receipt.completed_entries.saturating_add(1);
    Ok(())
}

/// Existing callers delegate through an unbounded no-op policy.
#[allow(dead_code)]
pub fn copy_recursive(src: &Path, dest_dir: &Path) -> Result<PathBuf> {
    let receipt = transfer_with_policy(
        src,
        dest_dir,
        false,
        TransferPolicy {
            stopped: &|| false,
            max_entries: usize::MAX,
        },
    );
    receipt.result?;
    Ok(receipt
        .destination
        .expect("successful transfer destination"))
}

fn normalize_for_prefix_check(path: &Path) -> std::io::Result<PathBuf> {
    if path.exists() {
        return fs::canonicalize(path);
    }
    let parent = path.parent().ok_or_else(|| {
        Error::new(
            ErrorKind::InvalidInput,
            format!("Path has no parent: {}", path.display()),
        )
    })?;
    let parent_canonical = fs::canonicalize(parent)?;
    Ok(match path.file_name() {
        Some(name) => parent_canonical.join(name),
        None => parent_canonical,
    })
}

fn ensure_not_descendant(src_dir: &Path, dest_path: &Path) -> Result<()> {
    let src_canonical = fs::canonicalize(src_dir)?;
    let dest_normalized = normalize_for_prefix_check(dest_path)?;
    if dest_normalized.starts_with(&src_canonical) {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!(
                "Refusing to copy/move directory '{}' into itself ('{}')",
                src_dir.display(),
                dest_path.display()
            ),
        )
        .into());
    }
    Ok(())
}

/// Move a file or directory from `src` to `dest_dir`.
///
/// Uses `fs::rename` first (fast, same-device). Falls back to copy+delete
/// if rename fails (cross-device). Returns the final path.
#[allow(dead_code)]
pub fn move_item(src: &Path, dest_dir: &Path) -> Result<PathBuf> {
    let receipt = transfer_with_policy(
        src,
        dest_dir,
        true,
        TransferPolicy {
            stopped: &|| false,
            max_entries: usize::MAX,
        },
    );
    receipt.result?;
    Ok(receipt
        .destination
        .expect("successful transfer destination"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs as unix_fs;
    use tempfile::TempDir;

    #[test]
    fn app_jobs_transfer_chunks_entry_and_depth_caps_preserve_honest_receipts() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("large");
        let dest = tmp.path().join("dest");
        fs::create_dir(&dest).unwrap();
        fs::write(&src, vec![b'x'; 192 * 1024]).unwrap();
        let output = dest.join("large");
        let receipt = transfer_with_policy(
            &src,
            &dest,
            false,
            TransferPolicy {
                stopped: &|| fs::metadata(&output).is_ok_and(|m| m.len() >= 64 * 1024),
                max_entries: 10,
            },
        );
        assert!(receipt.result.is_err());
        assert_eq!(receipt.completed_entries, 0);
        assert_eq!(receipt.destination, Some(output.clone()));
        assert_eq!(fs::metadata(&output).unwrap().len(), 64 * 1024);
        let root = tmp.path().join("root");
        fs::create_dir(&root).unwrap();
        for i in 0..3 {
            fs::write(root.join(i.to_string()), b"data").unwrap();
        }
        let receipt = transfer_with_policy(
            &root,
            &dest,
            false,
            TransferPolicy {
                stopped: &|| false,
                max_entries: 2,
            },
        );
        assert!(receipt.result.is_err());
        assert_eq!(receipt.completed_entries, 1);
        assert_eq!(fs::read_dir(dest.join("root")).unwrap().count(), 1);
        let deep = tmp.path().join("deep");
        fs::create_dir(&deep).unwrap();
        let mut leaf = deep.clone();
        for _ in 0..130 {
            leaf = leaf.join("d");
            fs::create_dir(&leaf).unwrap();
        }
        let receipt = transfer_with_policy(
            &deep,
            &dest,
            false,
            TransferPolicy {
                stopped: &|| false,
                max_entries: 1000,
            },
        );
        assert!(receipt.result.is_err());
        assert_eq!(receipt.destination, Some(dest.join("deep")));
        assert_eq!(receipt.completed_entries, 0);
    }

    #[cfg(unix)]
    #[test]
    fn app_jobs_transfer_bounded_policy_refuses_fifo_before_opening_or_creating() {
        use std::os::unix::ffi::OsStrExt;
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("fifo");
        let dest = tmp.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let cpath = std::ffi::CString::new(src.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        // This test-owned FIFO endpoint is released on every path. The legacy
        // chunk reader gets real EOF, so its behavioral red cannot hang.
        let endpoint = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&src)
            .unwrap();
        let source = src.clone();
        let destination = dest.clone();
        let worker = std::thread::spawn(move || {
            transfer_with_policy(
                &source,
                &destination,
                false,
                TransferPolicy {
                    stopped: &|| false,
                    max_entries: 10,
                },
            )
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !worker.is_finished()
            && !dest.join("fifo").exists()
            && std::time::Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        drop(endpoint);
        let receipt = worker.join().unwrap();
        assert!(
            receipt.result.is_err(),
            "bounded transfer opened a potentially indefinite FIFO read"
        );
        assert!(receipt.destination.is_none());
        assert!(!dest.join("fifo").exists());
    }

    #[test]
    fn app_jobs_transfer_stop_and_budget_precede_filesystem_mutation() {
        for stopped in [true, false] {
            let tmp = TempDir::new().unwrap();
            let src = tmp.path().join("file");
            let dest = tmp.path().join("dest");
            fs::write(&src, b"data").unwrap();
            fs::create_dir(&dest).unwrap();
            let receipt = transfer_with_policy(
                &src,
                &dest,
                false,
                TransferPolicy {
                    stopped: &|| stopped,
                    max_entries: if stopped { 10 } else { 0 },
                },
            );
            assert!(receipt.result.is_err(), "policy refusal was ignored");
            assert!(receipt.destination.is_none());
            assert!(!dest.join("file").exists());
            assert_eq!(receipt.completed_entries, 0);
        }
    }

    #[test]
    fn app_jobs_transfer_interrupt_retains_created_root_and_actual_entries() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("source");
        let dest = tmp.path().join("dest");
        fs::create_dir(&src).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(src.join("file"), b"data").unwrap();
        let receipt = transfer_with_policy(
            &src,
            &dest,
            false,
            TransferPolicy {
                stopped: &|| dest.join("source").exists(),
                max_entries: 10,
            },
        );
        assert!(
            receipt.result.is_err(),
            "interrupt after root creation was ignored"
        );
        assert_eq!(receipt.destination, Some(dest.join("source")));
        assert_eq!(receipt.completed_entries, 0);
        assert!(!receipt.moved);
        assert!(!dest.join("source/file").exists());
    }

    #[test]
    fn test_create_file() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.txt");
        create_file(&file_path).unwrap();
        assert!(file_path.exists());
    }

    #[test]
    fn test_create_dir() {
        let tmp = TempDir::new().unwrap();
        let dir_path = tmp.path().join("subdir");
        create_dir(&dir_path).unwrap();
        assert!(dir_path.exists());
        assert!(dir_path.is_dir());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn test_rename() {
        let tmp = TempDir::new().unwrap();
        let old_path = tmp.path().join("old.txt");
        let new_path = tmp.path().join("new.txt");
        create_file(&old_path).unwrap();
        rename(&old_path, &new_path).unwrap();
        assert!(!old_path.exists());
        assert!(new_path.exists());
        rename(&new_path, &old_path).unwrap();
        assert!(old_path.exists());
        assert!(!new_path.exists());
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn unsupported_rename_preserves_source() {
        let dir = TempDir::new().unwrap();
        let from = dir.path().join("source");
        let to = dir.path().join("destination");
        fs::write(&from, "keep").unwrap();
        let error = rename(&from, &to).unwrap_err();
        assert!(matches!(
            error,
            crate::error::AppError::Io(ref cause) if cause.kind() == ErrorKind::Unsupported
        ));
        assert_eq!(fs::read(&from).unwrap(), b"keep");
        assert!(!to.exists());
    }

    #[test]
    fn test_delete_file() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("delete_me.txt");
        create_file(&file_path).unwrap();
        assert!(file_path.exists());
        delete(&file_path).unwrap();
        assert!(!file_path.exists());
    }

    #[test]
    fn test_delete_directory_recursively() {
        let tmp = TempDir::new().unwrap();
        let dir_path = tmp.path().join("parent");
        let nested_dir = dir_path.join("child");
        fs::create_dir_all(&nested_dir).unwrap();
        fs::File::create(nested_dir.join("file.txt")).unwrap();
        fs::File::create(dir_path.join("root_file.txt")).unwrap();

        assert!(dir_path.exists());
        delete(&dir_path).unwrap();
        assert!(!dir_path.exists());
    }

    #[test]
    fn create_existing_file_preserves_bytes() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("existing.txt");
        fs::write(&file_path, "keep: true\n").unwrap();
        assert!(create_file(&file_path).is_err());
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "keep: true\n");
    }

    #[test]
    fn test_create_dir_already_exists_fails() {
        let tmp = TempDir::new().unwrap();
        let dir_path = tmp.path().join("dup");
        create_dir(&dir_path).unwrap();
        assert!(create_dir(&dir_path).is_err());
    }

    #[test]
    fn test_rename_nonexistent_fails() {
        let tmp = TempDir::new().unwrap();
        let from = tmp.path().join("no_such_file.txt");
        let to = tmp.path().join("dest.txt");
        assert!(rename(&from, &to).is_err());
    }

    #[test]
    fn test_delete_nonexistent_fails() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("no_such_file.txt");
        assert!(delete(&path).is_err());
    }

    #[test]
    fn create_existing_directory_is_rejected() {
        let dir = TempDir::new().unwrap();
        assert!(create_file(dir.path()).is_err());
        assert!(dir.path().is_dir());
    }

    #[test]
    fn rename_existing_file_preserves_both() {
        let dir = TempDir::new().unwrap();
        let from = dir.path().join("from");
        let to = dir.path().join("to");
        fs::write(&from, "source").unwrap();
        fs::write(&to, "destination").unwrap();
        assert!(rename(&from, &to).is_err());
        assert_eq!(fs::read(&from).unwrap(), b"source");
        assert_eq!(fs::read(&to).unwrap(), b"destination");
    }

    #[test]
    fn rename_existing_directory_preserves_both() {
        let dir = TempDir::new().unwrap();
        let from = dir.path().join("from");
        let to = dir.path().join("to");
        fs::create_dir(&from).unwrap();
        fs::write(from.join("keep"), "source").unwrap();
        fs::create_dir(&to).unwrap();
        assert!(rename(&from, &to).is_err());
        assert_eq!(fs::read(from.join("keep")).unwrap(), b"source");
        assert!(to.is_dir());
    }

    #[test]
    fn rename_same_path_is_rejected() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("same");
        fs::write(&path, "keep").unwrap();
        assert!(rename(&path, &path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn create_symlink_destinations_are_rejected() {
        for dangling in [false, true] {
            let dir = TempDir::new().unwrap();
            let target = dir.path().join("target");
            if !dangling {
                fs::write(&target, "keep").unwrap();
            }
            let link = dir.path().join("link");
            unix_fs::symlink(&target, &link).unwrap();
            assert!(create_file(&link).is_err());
            assert_eq!(fs::read_link(&link).unwrap(), target);
            if dangling {
                assert!(!target.exists());
            } else {
                assert_eq!(fs::read(&target).unwrap(), b"keep");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn rename_symlink_destinations_are_rejected() {
        for dangling in [false, true] {
            let dir = TempDir::new().unwrap();
            let target = dir.path().join("target");
            if !dangling {
                fs::write(&target, "keep").unwrap();
            }
            let link = dir.path().join("link");
            unix_fs::symlink(&target, &link).unwrap();
            let from = dir.path().join("source");
            fs::write(&from, "source").unwrap();
            assert!(rename(&from, &link).is_err());
            assert_eq!(fs::read(&from).unwrap(), b"source");
            assert_eq!(fs::read_link(&link).unwrap(), target);
            if !dangling {
                assert_eq!(fs::read(&target).unwrap(), b"keep");
            }
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn rename_symlink_source_moves_link_not_target() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("missing");
        let from = dir.path().join("link");
        let to = dir.path().join("moved");
        unix_fs::symlink(&target, &from).unwrap();
        rename(&from, &to).unwrap();
        assert!(fs::symlink_metadata(&from).is_err());
        assert_eq!(fs::read_link(&to).unwrap(), target);
        assert!(!target.exists());
    }

    #[test]
    fn operation_errors_include_paths_and_cause() {
        let dir = TempDir::new().unwrap();
        let from = dir.path().join("missing/source");
        let to = dir.path().join("missing/destination");
        let create_error = create_file(&to).unwrap_err().to_string();
        assert!(create_error.contains("create file"), "{create_error}");
        assert!(create_error.contains(&to.display().to_string()));
        assert!(create_error.contains(": "));
        let rename_error = rename(&from, &to).unwrap_err().to_string();
        assert!(rename_error.contains("rename"), "{rename_error}");
        assert!(rename_error.contains(&from.display().to_string()));
        assert!(rename_error.contains(&to.display().to_string()));
        assert!(rename_error.contains(": "));
    }

    #[test]
    fn racing_file_creators_have_one_winner() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("contested");
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                barrier.wait();
                create_file(&path)
            });
            let second = scope.spawn(|| {
                barrier.wait();
                create_file(&path)
            });
            let results = [first.join().unwrap(), second.join().unwrap()];
            assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        });
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn racing_rename_and_exclusive_creator_never_replace() {
        let dir = TempDir::new().unwrap();
        let from = dir.path().join("source");
        let to = dir.path().join("contested");
        fs::write(&from, "source").unwrap();
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let mover = scope.spawn(|| {
                barrier.wait();
                rename(&from, &to)
            });
            let creator = scope.spawn(|| {
                barrier.wait();
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&to)
            });
            let moved = mover.join().unwrap();
            let created = creator.join().unwrap();
            assert_ne!(moved.is_ok(), created.is_ok());
            if moved.is_ok() {
                assert_eq!(fs::read(&to).unwrap(), b"source");
                assert!(!from.exists());
            } else {
                assert_eq!(fs::read(&from).unwrap(), b"source");
                assert!(fs::read(&to).unwrap().is_empty());
            }
        });
    }

    // === copy_recursive tests ===

    #[test]
    fn test_copy_file_to_new_dest() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src.txt");
        fs::write(&src, "hello").unwrap();
        let dest_dir = tmp.path().join("dest");
        fs::create_dir(&dest_dir).unwrap();

        let result = copy_recursive(&src, &dest_dir).unwrap();
        assert_eq!(result, dest_dir.join("src.txt"));
        assert!(result.exists());
        assert_eq!(fs::read_to_string(&result).unwrap(), "hello");
        // Original still exists
        assert!(src.exists());
    }

    #[test]
    fn test_copy_file_collision_appends_suffix() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("file.txt");
        fs::write(&src, "original").unwrap();
        let dest_dir = tmp.path();
        // file.txt already exists at dest
        let result = copy_recursive(&src, dest_dir).unwrap();
        assert_eq!(result, tmp.path().join("file_copy.txt"));
        assert!(result.exists());
    }

    #[test]
    fn test_copy_file_double_collision() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("file.txt");
        fs::write(&src, "data").unwrap();
        fs::write(tmp.path().join("file_copy.txt"), "existing").unwrap();
        let result = copy_recursive(&src, tmp.path()).unwrap();
        assert_eq!(result, tmp.path().join("file_copy2.txt"));
    }

    #[test]
    fn test_copy_directory_recursive() {
        let tmp = TempDir::new().unwrap();
        let src_dir = tmp.path().join("src_dir");
        fs::create_dir(&src_dir).unwrap();
        fs::write(src_dir.join("a.txt"), "aaa").unwrap();
        fs::create_dir(src_dir.join("sub")).unwrap();
        fs::write(src_dir.join("sub").join("b.txt"), "bbb").unwrap();

        let dest_dir = tmp.path().join("dest");
        fs::create_dir(&dest_dir).unwrap();

        let result = copy_recursive(&src_dir, &dest_dir).unwrap();
        assert_eq!(result, dest_dir.join("src_dir"));
        assert!(result.join("a.txt").exists());
        assert!(result.join("sub").join("b.txt").exists());
        assert_eq!(fs::read_to_string(result.join("a.txt")).unwrap(), "aaa");
        assert_eq!(
            fs::read_to_string(result.join("sub").join("b.txt")).unwrap(),
            "bbb"
        );
    }

    #[test]
    fn test_copy_directory_into_descendant_rejected() {
        let tmp = TempDir::new().unwrap();
        let src_dir = tmp.path().join("src");
        fs::create_dir(&src_dir).unwrap();
        fs::create_dir(src_dir.join("nested")).unwrap();
        fs::write(src_dir.join("a.txt"), "aaa").unwrap();

        let result = copy_recursive(&src_dir, &src_dir);
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_copy_symlink_source_rejected() {
        let tmp = TempDir::new().unwrap();
        let real_dir = tmp.path().join("real");
        fs::create_dir(&real_dir).unwrap();
        fs::write(real_dir.join("a.txt"), "aaa").unwrap();

        let link = tmp.path().join("link_to_real");
        unix_fs::symlink(&real_dir, &link).unwrap();
        let dest_dir = tmp.path().join("dest");
        fs::create_dir(&dest_dir).unwrap();

        let result = copy_recursive(&link, &dest_dir);
        assert!(result.is_err());
    }

    // === move_item tests ===

    #[test]
    fn test_move_file() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("move_me.txt");
        fs::write(&src, "content").unwrap();
        let dest_dir = tmp.path().join("dest");
        fs::create_dir(&dest_dir).unwrap();

        let result = move_item(&src, &dest_dir).unwrap();
        assert_eq!(result, dest_dir.join("move_me.txt"));
        assert!(result.exists());
        assert!(!src.exists()); // Source removed
        assert_eq!(fs::read_to_string(&result).unwrap(), "content");
    }

    #[test]
    fn test_move_directory() {
        let tmp = TempDir::new().unwrap();
        let src_dir = tmp.path().join("move_dir");
        fs::create_dir(&src_dir).unwrap();
        fs::write(src_dir.join("inner.txt"), "data").unwrap();
        let dest_dir = tmp.path().join("dest");
        fs::create_dir(&dest_dir).unwrap();

        let result = move_item(&src_dir, &dest_dir).unwrap();
        assert_eq!(result, dest_dir.join("move_dir"));
        assert!(result.join("inner.txt").exists());
        assert!(!src_dir.exists());
    }

    #[test]
    fn test_move_directory_into_descendant_rejected() {
        let tmp = TempDir::new().unwrap();
        let src_dir = tmp.path().join("move_dir");
        fs::create_dir(&src_dir).unwrap();
        fs::create_dir(src_dir.join("child")).unwrap();
        fs::write(src_dir.join("inner.txt"), "data").unwrap();

        let result = move_item(&src_dir, &src_dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_move_with_collision() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("file.txt");
        fs::write(&src, "new").unwrap();
        let dest_dir = tmp.path().join("dest");
        fs::create_dir(&dest_dir).unwrap();
        fs::write(dest_dir.join("file.txt"), "existing").unwrap();

        let result = move_item(&src, &dest_dir).unwrap();
        assert_eq!(result, dest_dir.join("file_copy.txt"));
        assert!(!src.exists());
        // Original at dest untouched
        assert_eq!(
            fs::read_to_string(dest_dir.join("file.txt")).unwrap(),
            "existing"
        );
    }

    // === resolve_collision tests ===

    #[test]
    fn test_resolve_collision_no_conflict() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("new.txt");
        assert_eq!(resolve_collision(&path), path);
    }

    #[test]
    fn test_resolve_collision_no_extension() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("Makefile");
        fs::write(&path, "").unwrap();
        let resolved = resolve_collision(&path);
        assert_eq!(resolved, tmp.path().join("Makefile_copy"));
    }

    // === delete_recursive_with_progress tests ===

    #[test]
    fn test_delete_recursive_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.txt");
        fs::write(&path, "data").unwrap();

        let cancel = std::sync::atomic::AtomicBool::new(false);
        let progress: DeleteProgressFn = Box::new(|_, _| {});
        let (deleted, errors) = delete_recursive_with_progress(&path, &progress, &cancel);

        assert_eq!(deleted, 1);
        assert!(errors.is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn test_delete_recursive_directory() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("parent");
        fs::create_dir_all(dir.join("child")).unwrap();
        fs::write(dir.join("a.txt"), "a").unwrap();
        fs::write(dir.join("child").join("b.txt"), "b").unwrap();

        let cancel = std::sync::atomic::AtomicBool::new(false);
        let names = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let names_clone = names.clone();
        let progress: DeleteProgressFn = Box::new(move |name, _count| {
            names_clone.lock().unwrap().push(name.to_string());
        });

        let (deleted, errors) = delete_recursive_with_progress(&dir, &progress, &cancel);

        assert!(errors.is_empty());
        // 2 files + 2 dirs (child + parent) = 4
        assert_eq!(deleted, 4);
        assert!(!dir.exists());
        // Progress was reported for each item
        assert_eq!(names.lock().unwrap().len(), 4);
    }

    #[test]
    fn test_delete_recursive_cancelled() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("cancel_test");
        fs::create_dir(&dir).unwrap();
        for i in 0..10 {
            fs::write(dir.join(format!("file_{}.txt", i)), "data").unwrap();
        }

        let cancel = std::sync::atomic::AtomicBool::new(false);
        // Cancel after first file
        let progress: DeleteProgressFn = Box::new(|_name, count| {
            if count >= 1 {
                // We can't directly set cancel from here, but the test
                // verifies the cancel mechanism works
            }
        });

        // Set cancel immediately
        cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        let (deleted, _errors) = delete_recursive_with_progress(&dir, &progress, &cancel);

        // Cancelled before deleting any files
        assert_eq!(deleted, 0);
        // Directory and files still exist
        assert!(dir.exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_delete_recursive_does_not_follow_symlink_dirs() {
        let tmp = TempDir::new().unwrap();
        let outside = tmp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let outside_file = outside.join("keep_me.txt");
        fs::write(&outside_file, "safe").unwrap();

        let root = tmp.path().join("root");
        fs::create_dir(&root).unwrap();
        unix_fs::symlink(&outside, root.join("outside_link")).unwrap();

        let cancel = std::sync::atomic::AtomicBool::new(false);
        let progress: DeleteProgressFn = Box::new(|_, _| {});
        let (_deleted, errors) = delete_recursive_with_progress(&root, &progress, &cancel);

        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert!(!root.exists(), "root directory should be removed");
        assert!(
            outside_file.exists(),
            "outside target must remain after deleting symlinked root"
        );
    }
}
