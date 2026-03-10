//! Async S3 backend that shells out to the `aws` CLI.
//!
//! No AWS SDK dependency — all operations are performed by spawning
//! `aws s3 ls` / `aws s3 cp` subprocesses via `tokio::process::Command`.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio::process::Command;

use super::parser;
use super::types::{S3Config, S3Entry, S3Path};

/// The S3 backend that manages CLI interactions and caching.
#[derive(Debug, Clone)]
pub struct S3Backend {
    /// Optional AWS profile name.
    profile: Option<String>,
    /// Cache directory for downloaded files.
    #[allow(dead_code)]
    cache_dir: PathBuf,
    /// Map of S3 key → local cache path for already-downloaded files.
    #[allow(dead_code)]
    download_cache: HashMap<String, PathBuf>,
}

impl S3Backend {
    /// Create a new S3Backend from config.
    pub fn new(config: &S3Config) -> Self {
        let pid = std::process::id();
        let cache_dir = PathBuf::from(format!("/tmp/fm-s3-cache-{}", pid));
        Self {
            profile: config.profile.clone(),
            cache_dir,
            download_cache: HashMap::new(),
        }
    }

    /// Check if the `aws` CLI is available on $PATH.
    ///
    /// Returns `Ok(())` if found, `Err` with actionable message if not.
    pub async fn check_cli() -> Result<(), String> {
        match Command::new("which").arg("aws").output().await {
            Ok(output) if output.status.success() => Ok(()),
            _ => Err("AWS CLI (`aws`) not found on $PATH.\n\
                 Install it: https://docs.aws.amazon.com/cli/latest/userguide/install-cliv2.html\n\
                 Or: pip install awscli"
                .to_string()),
        }
    }

    /// List the contents of an S3 prefix.
    ///
    /// Spawns `aws s3 ls s3://bucket/prefix/` and parses the output.
    pub async fn list_prefix(&self, s3_path: &S3Path) -> Result<Vec<S3Entry>, String> {
        let uri = if s3_path.key.is_empty() {
            format!("s3://{}/", s3_path.bucket)
        } else if s3_path.key.ends_with('/') {
            format!("s3://{}/{}", s3_path.bucket, s3_path.key)
        } else {
            format!("s3://{}/{}/", s3_path.bucket, s3_path.key)
        };

        let mut cmd = Command::new("aws");
        if let Some(ref profile) = self.profile {
            cmd.arg("--profile").arg(profile);
        }
        cmd.arg("s3").arg("ls").arg(&uri);

        let output = cmd
            .output()
            .await
            .map_err(|e| format!("Failed to run `aws s3 ls`: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(parser::parse_error_output(&stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(parser::parse_ls_output(&stdout))
    }

    /// Download an S3 object to the local cache directory.
    ///
    /// Returns the local path to the downloaded file.
    /// Skips download if already cached for this session.
    #[allow(dead_code)]
    pub async fn download_to_cache(&mut self, s3_path: &S3Path) -> Result<PathBuf, String> {
        let cache_key = format!("{}/{}", s3_path.bucket, s3_path.key);

        // Check if already cached
        if let Some(cached) = self.download_cache.get(&cache_key) {
            if cached.exists() {
                return Ok(cached.clone());
            }
        }

        // Ensure cache directory exists
        if let Err(e) = std::fs::create_dir_all(&self.cache_dir) {
            return Err(format!("Failed to create cache dir: {}", e));
        }

        // Create local path preserving S3 key structure
        let local_path = self.cache_dir.join(&s3_path.bucket).join(&s3_path.key);
        if let Some(parent) = local_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return Err(format!("Failed to create cache subdirectory: {}", e));
            }
        }

        let s3_uri = s3_path.to_uri();
        let mut cmd = Command::new("aws");
        if let Some(ref profile) = self.profile {
            cmd.arg("--profile").arg(profile);
        }
        cmd.arg("s3")
            .arg("cp")
            .arg(&s3_uri)
            .arg(local_path.to_string_lossy().as_ref());

        let output = cmd
            .output()
            .await
            .map_err(|e| format!("Failed to run `aws s3 cp`: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(parser::parse_error_output(&stderr));
        }

        self.download_cache.insert(cache_key, local_path.clone());
        Ok(local_path)
    }

    /// Check if an S3 object is already cached locally.
    #[allow(dead_code)]
    pub fn is_cached(&self, s3_path: &S3Path) -> Option<&PathBuf> {
        let cache_key = format!("{}/{}", s3_path.bucket, s3_path.key);
        self.download_cache.get(&cache_key).filter(|p| p.exists())
    }

    /// Get the cache directory path.
    #[allow(dead_code)]
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Clean up the cache directory.
    pub fn cleanup_cache(&self) {
        let _ = std::fs::remove_dir_all(&self.cache_dir);
    }

    /// Get the configured AWS profile.
    #[allow(dead_code)]
    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    /// Build the base aws command with profile flag if set.
    #[allow(dead_code)]
    fn base_command(&self) -> Command {
        let mut cmd = Command::new("aws");
        if let Some(ref profile) = self.profile {
            cmd.arg("--profile").arg(profile);
        }
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_creation() {
        let config = S3Config {
            path: S3Path::parse("s3://test-bucket/prefix/").unwrap(),
            profile: Some("mfa".to_string()),
        };
        let backend = S3Backend::new(&config);
        assert_eq!(backend.profile(), Some("mfa"));
        assert!(backend
            .cache_dir()
            .to_string_lossy()
            .contains("fm-s3-cache-"));
    }

    #[test]
    fn test_backend_no_profile() {
        let config = S3Config {
            path: S3Path::parse("s3://bucket").unwrap(),
            profile: None,
        };
        let backend = S3Backend::new(&config);
        assert_eq!(backend.profile(), None);
    }

    #[test]
    fn test_cache_key_lookup() {
        let config = S3Config {
            path: S3Path::parse("s3://bucket").unwrap(),
            profile: None,
        };
        let backend = S3Backend::new(&config);
        let path = S3Path::parse("s3://bucket/key.txt").unwrap();
        assert!(backend.is_cached(&path).is_none());
    }
}
