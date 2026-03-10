//! Parser for `aws s3 ls` CLI output.
//!
//! Converts the text output of `aws s3 ls s3://bucket/prefix/` into
//! structured `S3Entry` values.
use super::types::S3Entry;

/// Parse the stdout of `aws s3 ls` into a list of S3 entries.
///
/// The `aws s3 ls` command produces two kinds of lines:
/// - `PRE <name>/` — a common prefix (virtual directory)
/// - `<date> <time> <size> <name>` — an object (file)
///
/// Empty lines and unexpected formats are silently skipped.
pub fn parse_ls_output(output: &str) -> Vec<S3Entry> {
    let mut entries = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(entry) = parse_line(trimmed) {
            entries.push(entry);
        }
    }
    entries
}

/// Parse a single line of `aws s3 ls` output.
fn parse_line(line: &str) -> Option<S3Entry> {
    // Directory prefix: "                           PRE <name>/"
    if let Some(rest) = line.strip_prefix("PRE ") {
        let name = rest.trim();
        if !name.is_empty() {
            return Some(S3Entry {
                name: name.to_string(),
                is_dir: true,
                size: 0,
                modified: String::new(),
            });
        }
        return None;
    }

    // Also handle the common indented format
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("PRE ") {
        let name = rest.trim();
        if !name.is_empty() {
            return Some(S3Entry {
                name: name.to_string(),
                is_dir: true,
                size: 0,
                modified: String::new(),
            });
        }
        return None;
    }

    // File entry: "2026-03-10 12:34:56     12345 filename.txt"
    // Format: YYYY-MM-DD HH:MM:SS <whitespace> <size> <name>
    // Date is 10 chars, space, time is 8 chars = 19 chars for datetime
    if line.len() < 20 {
        return None;
    }

    // Try to find the date pattern (YYYY-MM-DD HH:MM:SS)
    let date_part = &line[..10];
    if !looks_like_date(date_part) {
        return None;
    }

    // After "YYYY-MM-DD " (11 chars), time is "HH:MM:SS" (8 chars)
    if line.len() < 20 {
        return None;
    }

    let datetime = &line[..19]; // "YYYY-MM-DD HH:MM:SS"

    // After datetime, there's whitespace, then size, then whitespace, then name
    let rest = &line[19..].trim_start();

    // Parse size (digits)
    let (size_str, name_part) = split_size_and_name(rest)?;
    let size = size_str.parse::<u64>().ok()?;
    let name = name_part.trim();

    if name.is_empty() {
        return None;
    }

    Some(S3Entry {
        name: name.to_string(),
        is_dir: false,
        size,
        modified: datetime.to_string(),
    })
}

/// Check if a string looks like a date (YYYY-MM-DD).
fn looks_like_date(s: &str) -> bool {
    if s.len() != 10 {
        return false;
    }
    let bytes = s.as_bytes();
    // Check pattern: \d{4}-\d{2}-\d{2}
    bytes[4] == b'-' && bytes[7] == b'-' && bytes[..4].iter().all(|b| b.is_ascii_digit())
}

/// Split a string like "12345 filename.txt" into ("12345", "filename.txt").
fn split_size_and_name(s: &str) -> Option<(&str, &str)> {
    let idx = s.find(|c: char| !c.is_ascii_digit())?;
    let size_str = &s[..idx];
    if size_str.is_empty() {
        return None;
    }
    let name = s[idx..].trim_start();
    Some((size_str, name))
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
    fn test_parse_directory_entries() {
        let output = "                           PRE checkpoints/\n\
                       \n                           PRE logs/\n";
        let entries = parse_ls_output(output);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "checkpoints/");
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].name, "logs/");
        assert!(entries[1].is_dir);
    }

    #[test]
    fn test_parse_file_entries() {
        let output = "2026-03-10 12:34:56      12345 model.pt\n\
                       2026-03-09 08:00:00        567 config.yaml\n";
        let entries = parse_ls_output(output);
        assert_eq!(entries.len(), 2);

        assert_eq!(entries[0].name, "model.pt");
        assert!(!entries[0].is_dir);
        assert_eq!(entries[0].size, 12345);
        assert_eq!(entries[0].modified, "2026-03-10 12:34:56");

        assert_eq!(entries[1].name, "config.yaml");
        assert!(!entries[1].is_dir);
        assert_eq!(entries[1].size, 567);
    }

    #[test]
    fn test_parse_mixed_output() {
        let output = "                           PRE experiments/\n\
                       2026-03-10 12:34:56      12345 README.md\n\
                       \n\
                       2026-03-10 12:35:00   98765432 data.parquet\n";
        let entries = parse_ls_output(output);
        assert_eq!(entries.len(), 3);
        assert!(entries[0].is_dir);
        assert!(!entries[1].is_dir);
        assert!(!entries[2].is_dir);
        assert_eq!(entries[2].size, 98765432);
    }

    #[test]
    fn test_parse_empty_output() {
        let entries = parse_ls_output("");
        assert!(entries.is_empty());

        let entries = parse_ls_output("\n\n\n");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_filename_with_spaces() {
        let output = "2026-01-01 00:00:00       1000 my file name.txt\n";
        let entries = parse_ls_output(output);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "my file name.txt");
        assert_eq!(entries[0].size, 1000);
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

    #[test]
    fn test_parse_zero_size_file() {
        let output = "2026-03-10 12:00:00          0 empty-file.txt\n";
        let entries = parse_ls_output(output);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "empty-file.txt");
        assert_eq!(entries[0].size, 0);
    }

    #[test]
    fn test_parse_large_file_size() {
        let output = "2026-03-10 12:00:00 1099511627776 huge-model.bin\n";
        let entries = parse_ls_output(output);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].size, 1099511627776); // 1 TiB
    }
}
