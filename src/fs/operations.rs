use std::fs;
use std::path::Path;

use crate::error::Result;

/// Create an empty file at the given path.
pub fn create_file(path: &Path) -> Result<()> {
    fs::File::create(path)?;
    Ok(())
}

/// Create a new directory at the given path.
pub fn create_dir(path: &Path) -> Result<()> {
    fs::create_dir(path)?;
    Ok(())
}

/// Rename (move) a file or directory from one path to another.
pub fn rename(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to)?;
    Ok(())
}

/// Delete a file or directory. Directories are removed recursively.
pub fn delete(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
