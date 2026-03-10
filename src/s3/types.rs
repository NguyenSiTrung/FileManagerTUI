/// S3 path and entry types for the S3 browse mode.
///
/// These types represent parsed S3 URIs and directory listing entries
/// without any AWS SDK dependency — the backend shells out to the `aws` CLI.

use std::fmt;

/// A parsed S3 URI, split into bucket name and optional key prefix.
///
/// # Examples
/// ```
/// let path = S3Path::parse("s3://my-bucket/experiments/run-1/").unwrap();
/// assert_eq!(path.bucket, "my-bucket");
/// assert_eq!(path.key, "experiments/run-1/");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Path {
    /// The bucket name (e.g., "my-bucket").
    pub bucket: String,
    /// The key or prefix within the bucket (e.g., "experiments/run-1/").
    /// Empty string for bucket root.
    pub key: String,
}

impl S3Path {
    /// Parse an `s3://bucket/key` URI into an `S3Path`.
    ///
    /// Returns `None` if the URI doesn't start with `s3://` or has no bucket name.
    pub fn parse(uri: &str) -> Option<Self> {
        let stripped = uri.strip_prefix("s3://")?;
        if stripped.is_empty() {
            return None; // No bucket name
        }

        let (bucket, key) = match stripped.find('/') {
            Some(idx) => (&stripped[..idx], &stripped[idx + 1..]),
            None => (stripped, ""),
        };

        if bucket.is_empty() {
            return None; // Empty bucket name
        }

        Some(Self {
            bucket: bucket.to_string(),
            key: key.to_string(),
        })
    }

    /// Returns the full S3 URI string (e.g., `s3://bucket/key`).
    pub fn to_uri(&self) -> String {
        if self.key.is_empty() {
            format!("s3://{}", self.bucket)
        } else {
            format!("s3://{}/{}", self.bucket, self.key)
        }
    }

    /// Returns whether this path represents a "directory" (prefix).
    ///
    /// In S3 terms, a prefix ends with `/` or is empty (bucket root).
    pub fn is_prefix(&self) -> bool {
        self.key.is_empty() || self.key.ends_with('/')
    }

    /// Returns the display name for this path (last component or bucket name).
    pub fn display_name(&self) -> &str {
        if self.key.is_empty() {
            &self.bucket
        } else {
            // Remove trailing slash, then take last component
            let trimmed = self.key.trim_end_matches('/');
            trimmed.rsplit('/').next().unwrap_or(trimmed)
        }
    }

    /// Returns a child path by appending a name to the current key.
    pub fn child(&self, name: &str) -> Self {
        let new_key = if self.key.is_empty() {
            name.to_string()
        } else if self.key.ends_with('/') {
            format!("{}{}", self.key, name)
        } else {
            format!("{}/{}", self.key, name)
        };
        Self {
            bucket: self.bucket.clone(),
            key: new_key,
        }
    }

    /// Returns the parent prefix, or `None` if already at bucket root.
    pub fn parent(&self) -> Option<Self> {
        if self.key.is_empty() {
            return None;
        }
        let trimmed = self.key.trim_end_matches('/');
        match trimmed.rfind('/') {
            Some(idx) => Some(Self {
                bucket: self.bucket.clone(),
                key: format!("{}/", &trimmed[..idx]),
            }),
            None => Some(Self {
                bucket: self.bucket.clone(),
                key: String::new(),
            }),
        }
    }
}

impl fmt::Display for S3Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_uri())
    }
}

/// An entry from an S3 directory listing.
///
/// Represents either a "directory" (common prefix) or a file (object)
/// as returned by `aws s3 ls`.
#[derive(Debug, Clone)]
pub struct S3Entry {
    /// The entry name (e.g., "model.pt" or "checkpoints/").
    pub name: String,
    /// Whether this entry is a directory (common prefix).
    pub is_dir: bool,
    /// File size in bytes (0 for directories).
    pub size: u64,
    /// Last modified date/time as a string (e.g., "2026-03-10 12:34:56").
    /// Empty for directories (S3 prefixes have no modification time).
    pub modified: String,
}

/// Configuration for the S3 backend.
#[derive(Debug, Clone)]
pub struct S3Config {
    /// The starting S3 path.
    pub path: S3Path,
    /// Optional AWS profile name (--aws-profile flag).
    pub profile: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_full_path() {
        let path = S3Path::parse("s3://my-bucket/experiments/run-1/").unwrap();
        assert_eq!(path.bucket, "my-bucket");
        assert_eq!(path.key, "experiments/run-1/");
        assert!(path.is_prefix());
    }

    #[test]
    fn test_parse_bucket_only() {
        let path = S3Path::parse("s3://my-bucket").unwrap();
        assert_eq!(path.bucket, "my-bucket");
        assert_eq!(path.key, "");
        assert!(path.is_prefix());
    }

    #[test]
    fn test_parse_bucket_with_trailing_slash() {
        let path = S3Path::parse("s3://my-bucket/").unwrap();
        assert_eq!(path.bucket, "my-bucket");
        assert_eq!(path.key, "");
        assert!(path.is_prefix());
    }

    #[test]
    fn test_parse_file_path() {
        let path = S3Path::parse("s3://bucket/path/to/file.txt").unwrap();
        assert_eq!(path.bucket, "bucket");
        assert_eq!(path.key, "path/to/file.txt");
        assert!(!path.is_prefix());
    }

    #[test]
    fn test_parse_no_prefix() {
        assert!(S3Path::parse("not-s3://bucket").is_none());
        assert!(S3Path::parse("http://bucket").is_none());
        assert!(S3Path::parse("bucket/key").is_none());
    }

    #[test]
    fn test_parse_empty_bucket() {
        assert!(S3Path::parse("s3://").is_none());
        assert!(S3Path::parse("s3:///key").is_none());
    }

    #[test]
    fn test_to_uri() {
        let path = S3Path::parse("s3://bucket/key/file.txt").unwrap();
        assert_eq!(path.to_uri(), "s3://bucket/key/file.txt");

        let root = S3Path::parse("s3://bucket").unwrap();
        assert_eq!(root.to_uri(), "s3://bucket");
    }

    #[test]
    fn test_display_name() {
        assert_eq!(
            S3Path::parse("s3://my-bucket").unwrap().display_name(),
            "my-bucket"
        );
        assert_eq!(
            S3Path::parse("s3://bucket/experiments/").unwrap().display_name(),
            "experiments"
        );
        assert_eq!(
            S3Path::parse("s3://bucket/path/to/file.txt")
                .unwrap()
                .display_name(),
            "file.txt"
        );
    }

    #[test]
    fn test_child() {
        let root = S3Path::parse("s3://bucket").unwrap();
        let child = root.child("experiments/");
        assert_eq!(child.to_uri(), "s3://bucket/experiments/");

        let prefix = S3Path::parse("s3://bucket/experiments/").unwrap();
        let child = prefix.child("model.pt");
        assert_eq!(child.to_uri(), "s3://bucket/experiments/model.pt");
    }

    #[test]
    fn test_parent() {
        let path = S3Path::parse("s3://bucket/a/b/c/").unwrap();
        let parent = path.parent().unwrap();
        assert_eq!(parent.to_uri(), "s3://bucket/a/b/");

        let top = S3Path::parse("s3://bucket/a/").unwrap();
        let parent = top.parent().unwrap();
        assert_eq!(parent.to_uri(), "s3://bucket");

        let root = S3Path::parse("s3://bucket").unwrap();
        assert!(root.parent().is_none());
    }

    #[test]
    fn test_display_trait() {
        let path = S3Path::parse("s3://bucket/key").unwrap();
        assert_eq!(format!("{}", path), "s3://bucket/key");
    }
}
