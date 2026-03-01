use std::fs;
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};

use crate::error::Result;

/// Create an empty file at the given path.
#[allow(dead_code)]
pub fn create_file(path: &Path) -> Result<()> {
    fs::File::create(path)?;
    Ok(())
}

/// Create a new directory at the given path.
#[allow(dead_code)]
pub fn create_dir(path: &Path) -> Result<()> {
    fs::create_dir(path)?;
    Ok(())
}

/// Rename (move) a file or directory from one path to another.
#[allow(dead_code)]
pub fn rename(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to)?;
    Ok(())
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
#[allow(dead_code)]
pub fn copy_recursive(src: &Path, dest_dir: &Path) -> Result<PathBuf> {
    let name = src
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no filename"))?;
    let dest = resolve_collision(&dest_dir.join(name));
    let src_meta = fs::symlink_metadata(src)?;

    if src_meta.file_type().is_symlink() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("Refusing to copy symlink source: {}", src.display()),
        )
        .into());
    }

    if src_meta.is_dir() {
        ensure_not_descendant(src, &dest)?;
        copy_dir_recursive(src, &dest)?;
    } else {
        fs::copy(src, &dest)?;
    }
    Ok(dest)
}

/// Internal recursive directory copy.
fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        let meta = fs::symlink_metadata(&src_path)?;
        if meta.file_type().is_symlink() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!("Refusing to copy symlink entry: {}", src_path.display()),
            )
            .into());
        }
        if meta.is_dir() {
            copy_dir_recursive(&src_path, &dest_path)?;
        } else {
            fs::copy(&src_path, &dest_path)?;
        }
    }
    Ok(())
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
    let name = src
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no filename"))?;
    let dest = resolve_collision(&dest_dir.join(name));
    let src_meta = fs::symlink_metadata(src)?;
    let src_is_dir = src_meta.is_dir();

    if src_meta.file_type().is_symlink() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("Refusing to move symlink source: {}", src.display()),
        )
        .into());
    }

    if src_is_dir {
        ensure_not_descendant(src, &dest)?;
    }

    // Try rename first (same filesystem, instant)
    match fs::rename(src, &dest) {
        Ok(()) => Ok(dest),
        Err(_) => {
            // Fallback: copy then delete (cross-device)
            if src_is_dir {
                copy_dir_recursive(src, &dest)?;
                fs::remove_dir_all(src)?;
            } else {
                fs::copy(src, &dest)?;
                fs::remove_file(src)?;
            }
            Ok(dest)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs as unix_fs;
    use tempfile::TempDir;

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

    #[test]
    fn test_rename() {
        let tmp = TempDir::new().unwrap();
        let old_path = tmp.path().join("old.txt");
        let new_path = tmp.path().join("new.txt");
        create_file(&old_path).unwrap();
        rename(&old_path, &new_path).unwrap();
        assert!(!old_path.exists());
        assert!(new_path.exists());
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
    fn test_create_file_overwrites_existing() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("existing.txt");
        create_file(&file_path).unwrap();
        // File::create overwrites — should succeed again
        assert!(create_file(&file_path).is_ok());
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
