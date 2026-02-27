use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

use crate::app::ViewMode;

/// Default max file size before switching to head+tail mode (1 MB).
pub const DEFAULT_MAX_FULL_PREVIEW_BYTES: u64 = 1_048_576;
/// Default number of head lines in head+tail mode.
pub const DEFAULT_HEAD_LINES: usize = 50;
/// Default number of tail lines in head+tail mode.
pub const DEFAULT_TAIL_LINES: usize = 20;
/// Line count adjustment step for +/- keys.
pub const LINE_COUNT_STEP: usize = 10;

/// Detect the syntax name for a file based on its extension.
pub fn detect_syntax_name(path: &Path) -> &str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("py") => "Python",
        Some("rs") => "Rust",
        Some("yaml" | "yml") => "YAML",
        Some("json") => "JSON",
        Some("toml") => "TOML",
        Some("sh" | "bash" | "zsh") => "Bash",
        Some("sql") => "SQL",
        Some("md" | "markdown") => "Markdown",
        Some("html" | "htm") => "HTML",
        Some("css") => "CSS",
        Some("js" | "jsx") => "JavaScript",
        Some("ts" | "tsx") => "TypeScript",
        Some("c" | "h") => "C",
        Some("cpp" | "hpp" | "cc") => "C++",
        Some("java") => "Java",
        Some("go") => "Go",
        Some("rb") => "Ruby",
        Some("txt" | "log" | "csv" | "cfg" | "conf" | "ini") => "Plain Text",
        Some("ipynb") => "Python",
        None => detect_from_shebang(path),
        _ => "Plain Text",
    }
}

/// Detect syntax from shebang line for extensionless files.
fn detect_from_shebang(path: &Path) -> &str {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return "Plain Text",
    };
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    if reader.read_line(&mut first_line).is_err() {
        return "Plain Text";
    }
    if !first_line.starts_with("#!") {
        return "Plain Text";
    }
    let line = first_line.to_lowercase();
    if line.contains("python") {
        "Python"
    } else if line.contains("bash") || line.contains("/sh") {
        "Bash"
    } else if line.contains("ruby") {
        "Ruby"
    } else if line.contains("node") || line.contains("deno") {
        "JavaScript"
    } else if line.contains("perl") {
        "Perl"
    } else {
        "Plain Text"
    }
}

/// Load a theme from the built-in theme set by name, with fallback.
pub fn load_theme(theme_name: Option<&str>) -> Theme {
    let ts = ThemeSet::load_defaults();
    let name = theme_name.unwrap_or("base16-ocean.dark");
    ts.themes
        .get(name)
        .cloned()
        .unwrap_or_else(|| ts.themes["base16-ocean.dark"].clone())
}

/// Convert syntect color to ratatui Color.
fn syntect_color_to_ratatui(c: syntect::highlighting::Color) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

/// Load and syntax-highlight a file's content, returning styled lines for ratatui.
///
/// Returns `(lines, total_line_count)`. On error, returns a single error-message line.
pub fn load_highlighted_content(
    path: &Path,
    ss: &SyntaxSet,
    theme: &Theme,
) -> (Vec<Line<'static>>, usize) {
    let content = match fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => {
                let lossy = String::from_utf8_lossy(e.as_bytes()).to_string();
                lossy
            }
        },
        Err(e) => {
            let msg = format!("Error reading file: {}", e);
            return (
                vec![Line::from(Span::styled(
                    msg,
                    Style::default().fg(Color::Red),
                ))],
                1,
            );
        }
    };

    let syntax_name = detect_syntax_name(path);
    let syntax = ss
        .find_syntax_by_name(syntax_name)
        .or_else(|| ss.find_syntax_by_extension(path.extension()?.to_str()?))
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    let mut highlighter = syntect::easy::HighlightLines::new(syntax, theme);
    let lines_text: Vec<&str> = content.lines().collect();
    let total = lines_text.len().max(1);
    let line_num_width = total.to_string().len();

    let mut result_lines = Vec::with_capacity(total);
    for (i, line_str) in lines_text.iter().enumerate() {
        let mut spans: Vec<Span<'static>> = Vec::new();

        // Line number
        let num = format!("{:>width$} │ ", i + 1, width = line_num_width);
        spans.push(Span::styled(num, Style::default().fg(Color::DarkGray)));

        // Highlighted content
        match highlighter.highlight_line(line_str, ss) {
            Ok(ranges) => {
                for (style, text) in ranges {
                    let fg = syntect_color_to_ratatui(style.foreground);
                    spans.push(Span::styled(text.to_string(), Style::default().fg(fg)));
                }
            }
            Err(_) => {
                spans.push(Span::raw(line_str.to_string()));
            }
        }

        result_lines.push(Line::from(spans));
    }

    if result_lines.is_empty() {
        result_lines.push(Line::from(Span::styled(
            "(empty file)",
            Style::default().fg(Color::DarkGray),
        )));
    }

    (result_lines, total)
}

/// Count lines in a file using fast byte scanning (64KB chunks).
pub fn fast_line_count(path: &Path) -> std::io::Result<usize> {
    let mut file = fs::File::open(path)?;
    let mut buf = [0u8; 65536];
    let mut count = 0usize;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        count += buf[..n].iter().filter(|&&b| b == b'\n').count();
    }
    // If file doesn't end with newline, the last line still counts
    if count == 0 {
        // Check if file has any content
        file.seek(SeekFrom::Start(0))?;
        let mut check = [0u8; 1];
        if file.read(&mut check)? > 0 {
            count = 1;
        }
    }
    Ok(count)
}

/// Load head+tail content from a large file.
///
/// Returns styled lines with head section, separator, and tail section.
pub fn load_head_tail_content(
    path: &Path,
    ss: &SyntaxSet,
    theme: &Theme,
    head_lines: usize,
    tail_lines: usize,
    view_mode: ViewMode,
) -> (Vec<Line<'static>>, usize) {
    let total_lines = match fast_line_count(path) {
        Ok(n) => n,
        Err(e) => {
            return (
                vec![Line::from(Span::styled(
                    format!("Error counting lines: {}", e),
                    Style::default().fg(Color::Red),
                ))],
                1,
            );
        }
    };

    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            return (
                vec![Line::from(Span::styled(
                    format!("Error reading file: {}", e),
                    Style::default().fg(Color::Red),
                ))],
                1,
            );
        }
    };
    let reader = BufReader::new(file);
    let all_lines: Vec<String> = reader.lines().map_while(Result::ok).collect();

    let syntax_name = detect_syntax_name(path);
    let syntax = ss
        .find_syntax_by_name(syntax_name)
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let mut highlighter = syntect::easy::HighlightLines::new(syntax, theme);

    let line_num_width = total_lines.to_string().len();
    let mut result_lines: Vec<Line<'static>> = Vec::new();

    let effective_head = head_lines.min(all_lines.len());
    let effective_tail = tail_lines.min(all_lines.len().saturating_sub(effective_head));
    let tail_start = all_lines.len().saturating_sub(effective_tail);

    match view_mode {
        ViewMode::HeadAndTail => {
            // Head section
            for (i, line_str) in all_lines[..effective_head].iter().enumerate() {
                result_lines.push(highlight_single_line(
                    line_str,
                    i + 1,
                    line_num_width,
                    &mut highlighter,
                    ss,
                ));
            }

            // Separator
            if tail_start > effective_head {
                let omitted = tail_start - effective_head;
                let sep = format!("  ──── {} lines omitted ────", omitted);
                result_lines.push(Line::from(Span::styled(
                    sep,
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::DIM),
                )));
            }

            // Tail section
            for (i, line_str) in all_lines[tail_start..].iter().enumerate() {
                result_lines.push(highlight_single_line(
                    line_str,
                    tail_start + i + 1,
                    line_num_width,
                    &mut highlighter,
                    ss,
                ));
            }
        }
        ViewMode::HeadOnly => {
            for (i, line_str) in all_lines[..effective_head].iter().enumerate() {
                result_lines.push(highlight_single_line(
                    line_str,
                    i + 1,
                    line_num_width,
                    &mut highlighter,
                    ss,
                ));
            }
        }
        ViewMode::TailOnly => {
            for (i, line_str) in all_lines[tail_start..].iter().enumerate() {
                result_lines.push(highlight_single_line(
                    line_str,
                    tail_start + i + 1,
                    line_num_width,
                    &mut highlighter,
                    ss,
                ));
            }
        }
    }

    let displayed = result_lines.len();
    (result_lines, displayed.max(1))
}

/// Highlight a single line with line number prefix.
fn highlight_single_line(
    line_str: &str,
    line_num: usize,
    line_num_width: usize,
    highlighter: &mut syntect::easy::HighlightLines,
    ss: &SyntaxSet,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();

    let num = format!("{:>width$} │ ", line_num, width = line_num_width);
    spans.push(Span::styled(num, Style::default().fg(Color::DarkGray)));

    match highlighter.highlight_line(line_str, ss) {
        Ok(ranges) => {
            for (style, text) in ranges {
                let fg = syntect_color_to_ratatui(style.foreground);
                spans.push(Span::styled(text.to_string(), Style::default().fg(fg)));
            }
        }
        Err(_) => {
            spans.push(Span::raw(line_str.to_string()));
        }
    }

    Line::from(spans)
}

/// Known binary file extensions.
const BINARY_EXTENSIONS: &[&str] = &[
    "pt", "pth", "h5", "hdf5", "pkl", "pickle", "onnx", "zip", "tar", "gz", "bz2", "xz", "so",
    "dylib", "exe", "bin", "img", "iso",
];

/// Check if a file is binary by extension or null-byte scan.
pub fn is_binary_file(path: &Path) -> bool {
    // Check known extensions first
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if BINARY_EXTENSIONS
            .iter()
            .any(|&b| b.eq_ignore_ascii_case(ext))
        {
            return true;
        }
    }

    // Fallback: scan first 8KB for null bytes
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut reader = BufReader::new(file);
    let mut buf = [0u8; 8192];
    let n = match reader.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };
    buf[..n].contains(&0)
}

/// Format bytes into human-readable size string.
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    const TB: u64 = 1024 * GB;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Format Unix permissions as rwxrwxrwx string.
fn format_permissions(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    let flags = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];
    for (bit, ch) in flags {
        if mode & bit != 0 {
            s.push(ch);
        } else {
            s.push('-');
        }
    }
    s
}

/// Generate metadata display lines for a binary file.
pub fn load_binary_metadata(path: &Path) -> (Vec<Line<'static>>, usize) {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return (
                vec![Line::from(Span::styled(
                    format!("Error reading metadata: {}", e),
                    Style::default().fg(Color::Red),
                ))],
                1,
            );
        }
    };

    let label_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let value_style = Style::default().fg(Color::White);
    let dim_style = Style::default().fg(Color::DarkGray);

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());

    let size_str = format_size(meta.len());

    let modified_str = meta
        .modified()
        .ok()
        .and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| {
                let secs = d.as_secs();
                let days = secs / 86400;
                let remaining = secs % 86400;
                let hours = remaining / 3600;
                let minutes = (remaining % 3600) / 60;
                // Simple date calculation from epoch days
                let (year, month, day) = epoch_days_to_date(days);
                format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}",
                    year, month, day, hours, minutes
                )
            })
        })
        .unwrap_or_else(|| "Unknown".to_string());

    let perms_str = format_permissions(meta.permissions().mode());

    let lines = vec![
        // Blank line
        Line::from(""),
        // File name
        Line::from(vec![
            Span::styled("  File: ", label_style),
            Span::styled(file_name, value_style),
        ]),
        // Size
        Line::from(vec![
            Span::styled("  Size: ", label_style),
            Span::styled(size_str, value_style),
        ]),
        // Modified
        Line::from(vec![
            Span::styled("  Modified: ", label_style),
            Span::styled(modified_str, value_style),
        ]),
        // Permissions
        Line::from(vec![
            Span::styled("  Permissions: ", label_style),
            Span::styled(perms_str, value_style),
        ]),
        // Blank line
        Line::from(""),
        // Binary message
        Line::from(Span::styled("  [Binary file — cannot preview]", dim_style)),
    ];

    let total = lines.len();
    (lines, total)
}

/// Convert days since Unix epoch to (year, month, day).
fn epoch_days_to_date(days: u64) -> (u64, u64, u64) {
    // Simple algorithm: iterate years/months
    let mut remaining = days as i64;
    let mut year = 1970u64;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }

    let days_in_months: [i64; 12] = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1u64;
    for &dm in &days_in_months {
        if remaining < dm {
            break;
        }
        remaining -= dm;
        month += 1;
    }

    (year, month, remaining as u64 + 1)
}

fn is_leap_year(year: u64) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn detect_syntax_by_extension() {
        assert_eq!(detect_syntax_name(Path::new("foo.rs")), "Rust");
        assert_eq!(detect_syntax_name(Path::new("bar.py")), "Python");
        assert_eq!(detect_syntax_name(Path::new("baz.yml")), "YAML");
        assert_eq!(detect_syntax_name(Path::new("config.toml")), "TOML");
        assert_eq!(detect_syntax_name(Path::new("style.css")), "CSS");
        assert_eq!(detect_syntax_name(Path::new("page.html")), "HTML");
        assert_eq!(detect_syntax_name(Path::new("app.tsx")), "TypeScript");
        assert_eq!(detect_syntax_name(Path::new("Makefile")), "Plain Text");
        assert_eq!(detect_syntax_name(Path::new("readme.md")), "Markdown");
    }

    #[test]
    fn detect_syntax_unknown_extension() {
        assert_eq!(detect_syntax_name(Path::new("file.xyz")), "Plain Text");
    }

    #[test]
    fn detect_shebang_python() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("script");
        let mut f = File::create(&path).unwrap();
        writeln!(f, "#!/usr/bin/env python3").unwrap();
        writeln!(f, "print('hello')").unwrap();
        assert_eq!(detect_syntax_name(&path), "Python");
    }

    #[test]
    fn detect_shebang_bash() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("run");
        let mut f = File::create(&path).unwrap();
        writeln!(f, "#!/bin/bash").unwrap();
        assert_eq!(detect_syntax_name(&path), "Bash");
    }

    #[test]
    fn detect_shebang_sh() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("run2");
        let mut f = File::create(&path).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        assert_eq!(detect_syntax_name(&path), "Bash");
    }

    #[test]
    fn detect_no_shebang() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data");
        let mut f = File::create(&path).unwrap();
        writeln!(f, "just some text").unwrap();
        assert_eq!(detect_syntax_name(&path), "Plain Text");
    }

    #[test]
    fn load_theme_default() {
        let theme = load_theme(None);
        // Just verify it doesn't panic and returns something
        assert!(!theme.scopes.is_empty() || theme.settings.background.is_some());
    }

    #[test]
    fn load_theme_invalid_falls_back() {
        let theme = load_theme(Some("nonexistent-theme"));
        // Should fall back to base16-ocean.dark
        assert!(!theme.scopes.is_empty() || theme.settings.background.is_some());
    }

    #[test]
    fn highlight_rust_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.rs");
        let mut f = File::create(&path).unwrap();
        writeln!(f, "fn main() {{").unwrap();
        writeln!(f, "    println!(\"hello\");").unwrap();
        writeln!(f, "}}").unwrap();

        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, total) = load_highlighted_content(&path, &ss, &theme);
        assert_eq!(total, 3);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn highlight_empty_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.txt");
        File::create(&path).unwrap();

        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, total) = load_highlighted_content(&path, &ss, &theme);
        assert_eq!(total, 1);
        assert!(!lines.is_empty());
    }

    #[test]
    fn highlight_nonexistent_file() {
        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, total) = load_highlighted_content(Path::new("/nonexistent"), &ss, &theme);
        assert_eq!(total, 1);
        // Should contain error message
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("Error"));
    }

    // === Fast line counting tests ===

    #[test]
    fn fast_line_count_small_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("small.txt");
        let mut f = File::create(&path).unwrap();
        writeln!(f, "line 1").unwrap();
        writeln!(f, "line 2").unwrap();
        writeln!(f, "line 3").unwrap();
        assert_eq!(fast_line_count(&path).unwrap(), 3);
    }

    #[test]
    fn fast_line_count_empty_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.txt");
        File::create(&path).unwrap();
        assert_eq!(fast_line_count(&path).unwrap(), 0);
    }

    #[test]
    fn fast_line_count_no_trailing_newline() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("no_nl.txt");
        let mut f = File::create(&path).unwrap();
        write!(f, "no newline").unwrap(); // no trailing \n
        assert_eq!(fast_line_count(&path).unwrap(), 1);
    }

    // === Head+tail tests ===

    #[test]
    fn head_tail_basic() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("big.txt");
        let mut f = File::create(&path).unwrap();
        for i in 1..=100 {
            writeln!(f, "line {}", i).unwrap();
        }
        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, _) = load_head_tail_content(&path, &ss, &theme, 10, 5, ViewMode::HeadAndTail);
        // Should have 10 head + 1 separator + 5 tail = 16 lines
        assert_eq!(lines.len(), 16);
    }

    #[test]
    fn head_only_mode() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("big2.txt");
        let mut f = File::create(&path).unwrap();
        for i in 1..=100 {
            writeln!(f, "line {}", i).unwrap();
        }
        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, _) = load_head_tail_content(&path, &ss, &theme, 10, 5, ViewMode::HeadOnly);
        assert_eq!(lines.len(), 10);
    }

    #[test]
    fn tail_only_mode() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("big3.txt");
        let mut f = File::create(&path).unwrap();
        for i in 1..=100 {
            writeln!(f, "line {}", i).unwrap();
        }
        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, _) = load_head_tail_content(&path, &ss, &theme, 10, 5, ViewMode::TailOnly);
        assert_eq!(lines.len(), 5);
    }

    // === Binary file detection tests ===

    #[test]
    fn binary_detection_by_known_extension() {
        assert!(is_binary_file(Path::new("model.pt")));
        assert!(is_binary_file(Path::new("model.pth")));
        assert!(is_binary_file(Path::new("data.h5")));
        assert!(is_binary_file(Path::new("data.hdf5")));
        assert!(is_binary_file(Path::new("model.pkl")));
        assert!(is_binary_file(Path::new("model.pickle")));
        assert!(is_binary_file(Path::new("model.onnx")));
        assert!(is_binary_file(Path::new("archive.zip")));
        assert!(is_binary_file(Path::new("archive.tar")));
        assert!(is_binary_file(Path::new("file.gz")));
        assert!(is_binary_file(Path::new("file.bz2")));
        assert!(is_binary_file(Path::new("file.xz")));
        assert!(is_binary_file(Path::new("lib.so")));
        assert!(is_binary_file(Path::new("lib.dylib")));
        assert!(is_binary_file(Path::new("app.exe")));
        assert!(is_binary_file(Path::new("data.bin")));
        assert!(is_binary_file(Path::new("disk.img")));
        assert!(is_binary_file(Path::new("disk.iso")));
    }

    #[test]
    fn binary_detection_text_file_not_binary() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("hello.txt");
        let mut f = File::create(&path).unwrap();
        writeln!(f, "This is plain text").unwrap();
        assert!(!is_binary_file(&path));
    }

    #[test]
    fn binary_detection_null_byte_scan() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("unknown.dat");
        let mut f = File::create(&path).unwrap();
        f.write_all(&[0x00, 0x01, 0x02, 0xFF]).unwrap();
        assert!(is_binary_file(&path));
    }

    #[test]
    fn binary_detection_nonexistent_file() {
        assert!(!is_binary_file(Path::new("/nonexistent/file.dat")));
    }

    // === Binary metadata display tests ===

    #[test]
    fn binary_metadata_shows_info() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.bin");
        let mut f = File::create(&path).unwrap();
        f.write_all(&[0u8; 1024]).unwrap();

        let (lines, total) = load_binary_metadata(&path);
        assert!(total >= 7); // blank, file, size, modified, permissions, blank, message
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("test.bin"));
        assert!(all_text.contains("1.00 KB"));
        assert!(all_text.contains("Binary file"));
    }

    #[test]
    fn binary_metadata_nonexistent_file() {
        let (lines, total) = load_binary_metadata(Path::new("/nonexistent/file"));
        assert_eq!(total, 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("Error"));
    }

    // === Format size tests ===

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1023), "1023 B");
    }

    #[test]
    fn format_size_kb() {
        assert_eq!(format_size(1024), "1.00 KB");
        assert_eq!(format_size(2048), "2.00 KB");
    }

    #[test]
    fn format_size_mb() {
        assert_eq!(format_size(1024 * 1024), "1.00 MB");
    }

    #[test]
    fn format_size_gb() {
        assert_eq!(format_size(1024 * 1024 * 1024), "1.00 GB");
    }

    // === Format permissions tests ===

    #[test]
    fn format_permissions_rwx() {
        assert_eq!(format_permissions(0o755), "rwxr-xr-x");
        assert_eq!(format_permissions(0o644), "rw-r--r--");
        assert_eq!(format_permissions(0o777), "rwxrwxrwx");
        assert_eq!(format_permissions(0o000), "---------");
    }
}
