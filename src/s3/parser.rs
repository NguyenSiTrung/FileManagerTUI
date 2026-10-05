//! Parser for `aws s3api list-objects-v2 --output json` responses.
//!
//! One response is one page: delimiter-grouped `CommonPrefixes` (virtual
//! directories), `Contents` (objects) and an optional `NextContinuationToken`.
use serde::Deserialize;

use super::types::{S3Entry, S3Page};

/// Continuation tokens are opaque and short; refuse anything implausible so a
/// hostile or corrupt response cannot inflate retained job envelopes.
pub const MAX_TOKEN_BYTES: usize = 4096;

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ListResponse {
    contents: Option<Vec<Object>>,
    common_prefixes: Option<Vec<CommonPrefix>>,
    next_continuation_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Object {
    key: String,
    #[serde(default)]
    size: u64,
    last_modified: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CommonPrefix {
    prefix: String,
}

/// Parse one `list-objects-v2` response listing `prefix` (empty or `/`-terminated).
///
/// Directories come first, then objects, each in API order. The folder-marker
/// object whose key equals `prefix` is skipped. Empty stdout (what the CLI
/// prints for an empty prefix) is an empty final page.
pub fn parse_list_objects(stdout: &[u8], prefix: &str) -> Result<S3Page, &'static str> {
    if stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(S3Page::default());
    }
    let response: ListResponse = serde_json::from_slice(stdout)
        .map_err(|_| "S3 failed: unreadable AWS CLI listing response")?;
    let directories = response.common_prefixes.unwrap_or_default();
    let objects = response.contents.unwrap_or_default();
    let mut entries = Vec::with_capacity(directories.len() + objects.len());
    for directory in directories {
        if let Some(name) = directory
            .prefix
            .strip_prefix(prefix)
            .filter(|n| !n.is_empty())
        {
            entries.push(S3Entry {
                name: name.to_owned(),
                is_dir: true,
                size: 0,
                modified: String::new(),
            });
        }
    }
    for object in objects {
        if let Some(name) = object.key.strip_prefix(prefix).filter(|n| !n.is_empty()) {
            entries.push(S3Entry {
                name: name.to_owned(),
                is_dir: false,
                size: object.size,
                modified: object
                    .last_modified
                    .as_deref()
                    .map_or_else(String::new, display_time),
            });
        }
    }
    let next_token = match response.next_continuation_token {
        Some(token) if token.len() > MAX_TOKEN_BYTES => {
            return Err("S3 failed: continuation token too large");
        }
        token => token.filter(|token| !token.is_empty()),
    };
    Ok(S3Page {
        entries,
        next_token,
    })
}

/// `2026-03-10T12:34:56+00:00` -> `2026-03-10 12:34:56`; anything else is kept.
fn display_time(raw: &str) -> String {
    match raw.get(..19) {
        Some(head) if head.as_bytes()[10] == b'T' => format!("{} {}", &head[..10], &head[11..]),
        _ => raw.to_owned(),
    }
}

/// Parse error output from `aws s3 ls` and convert to user-friendly message.
pub fn parse_error_output(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        return "Unknown AWS CLI error".to_string();
    }

    // Check for common error patterns
    if trimmed.contains("ExpiredToken") || trimmed.contains("expired") {
        return "AWS credentials have expired. Please refresh your credentials.".to_string();
    }
    if trimmed.contains("AccessDenied") || trimmed.contains("Access Denied") {
        return "Access denied. Check your AWS permissions.".to_string();
    }
    if trimmed.contains("NoSuchBucket") {
        return "Bucket does not exist. Check the bucket name.".to_string();
    }
    if trimmed.contains("NoSuchKey") {
        return "S3 key not found.".to_string();
    }
    if trimmed.contains("Unable to locate credentials") || trimmed.contains("NoCredentialProviders")
    {
        return "No AWS credentials found. Configure via `aws configure` or environment variables."
            .to_string();
    }
    if trimmed.contains("Could not connect") || trimmed.contains("ConnectTimeoutError") {
        return "Network error: could not connect to AWS. Check your internet connection."
            .to_string();
    }

    // Fall back to the raw error, trimmed to first line
    trimmed.lines().next().unwrap_or(trimmed).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_directories_then_objects_with_prefix_stripped() {
        let json = br#"{
            "Contents": [
                {"Key": "run/", "Size": 0, "LastModified": "2026-03-10T00:00:00+00:00"},
                {"Key": "run/my file.txt", "Size": 1000, "LastModified": "2026-03-10T12:34:56+00:00"}
            ],
            "CommonPrefixes": [{"Prefix": "run/logs/"}, {"Prefix": "run/ckpt/"}],
            "NextContinuationToken": "tok"
        }"#;
        let page = parse_list_objects(json, "run/").unwrap();
        let names: Vec<_> = page
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.is_dir))
            .collect();
        assert_eq!(
            names,
            [("logs/", true), ("ckpt/", true), ("my file.txt", false)]
        );
        assert_eq!(page.entries[2].size, 1000);
        assert_eq!(page.entries[2].modified, "2026-03-10 12:34:56");
        assert_eq!(page.next_token.as_deref(), Some("tok"));
    }

    #[test]
    fn test_parse_final_page_has_no_token() {
        let json = br#"{"Contents": [{"Key": "a", "Size": 1099511627776}]}"#;
        let page = parse_list_objects(json, "").unwrap();
        assert_eq!(page.entries[0].size, 1099511627776);
        assert!(page.next_token.is_none());
    }

    #[test]
    fn test_parse_empty_and_null_sections() {
        assert!(parse_list_objects(b"", "p/").unwrap().entries.is_empty());
        assert!(parse_list_objects(b"\n", "p/").unwrap().entries.is_empty());
        let page =
            parse_list_objects(br#"{"Contents": null, "CommonPrefixes": null}"#, "").unwrap();
        assert!(page.entries.is_empty() && page.next_token.is_none());
    }

    #[test]
    fn test_parse_rejects_garbage_and_huge_token() {
        assert!(parse_list_objects(b"PRE child/", "").is_err());
        let huge = format!(
            r#"{{"NextContinuationToken": "{}"}}"#,
            "t".repeat(MAX_TOKEN_BYTES + 1)
        );
        assert!(parse_list_objects(huge.as_bytes(), "").is_err());
    }

    #[test]
    fn test_parse_error_common_patterns() {
        assert!(parse_error_output("ExpiredToken: ...").contains("expired"));
        assert!(parse_error_output("AccessDenied").contains("Access denied"));
        assert!(parse_error_output("NoSuchBucket").contains("does not exist"));
        assert!(parse_error_output("Unable to locate credentials").contains("No AWS credentials"));
        assert!(parse_error_output("Could not connect").contains("Network error"));
    }

    #[test]
    fn test_parse_error_empty() {
        assert_eq!(parse_error_output(""), "Unknown AWS CLI error");
        assert_eq!(parse_error_output("   "), "Unknown AWS CLI error");
    }

    #[test]
    fn test_parse_error_unknown() {
        assert_eq!(
            parse_error_output("Something went wrong\nMore details"),
            "Something went wrong"
        );
    }
}
