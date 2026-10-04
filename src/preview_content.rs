use std::fs;
use std::io::{BufRead, BufReader, Read};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

use crate::app::ViewMode;
use crate::theme::ThemeColors;

/// Line count adjustment step for +/- keys.
pub const LINE_COUNT_STEP: usize = 10;

/// Hard cap for full-file preview loading (5 MB).
///
/// Files larger than this are handled via head+tail mode instead; the notebook
/// path takes this as its explicit budget because it has no head/tail fallback.
pub const MAX_PREVIEW_SIZE: u64 = 5 * 1024 * 1024;

/// Detect the syntax name for a file based on its extension or filename.
pub fn detect_syntax_name(path: &Path) -> &str {
    // Filename-based detection (for files without meaningful extensions)
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        match name {
            "Dockerfile" | "dockerfile" => return "Dockerfile",
            "Makefile" | "makefile" | "GNUmakefile" => return "Makefile",
            ".env" => return "Bash",
            ".gitignore" | ".dockerignore" | ".hgignore" => return "Plain Text",
            _ => {}
        }
    }

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
        // Expanded coverage (FR-5)
        Some("lua") => "Lua",
        Some("php") => "PHP",
        Some("swift") => "Swift",
        Some("kt" | "kts") => "Kotlin",
        Some("scala") => "Scala",
        Some("r" | "R") => "R",
        Some("tf") => "Terraform",
        Some("nix") => "Nix",
        Some("zig") => "Zig",
        Some("glsl") => "GLSL",
        Some("xml" | "xsl" | "xslt" | "svg") => "XML",
        Some("txt" | "log" | "csv" | "cfg" | "conf" | "ini") => "Plain Text",
        Some("ipynb") => "Python",
        None => detect_from_shebang(path),
        _ => "Plain Text",
    }
}

/// Detect syntax from shebang line for extensionless files.
fn detect_from_shebang(path: &Path) -> &str {
    /// Cap the shebang probe; a real `#!` line is far shorter than this.
    const MAX_SHEBANG_BYTES: u64 = 4096;
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return "Plain Text",
    };
    let mut reader = BufReader::new(file.take(MAX_SHEBANG_BYTES));
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

/// Read at most `max_bytes` bytes from `reader` in one bounded pass.
///
/// One extra detection byte distinguishes "fits the budget" from "grew past
/// it" without a metadata-then-read race: a source holding more than
/// `max_bytes` bytes yields `None` instead of a silently truncated buffer.
/// This mirrors the bounded descriptor read in `src/fs/save.rs`.
fn read_bounded(reader: impl Read, max_bytes: u64) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    reader
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        Ok(None)
    } else {
        Ok(Some(bytes))
    }
}

/// Result of a bounded whole-file preview read.
enum PreviewRead {
    /// Decoded content; `is_lossy` is set when the bytes were not valid UTF-8.
    Text { text: String, is_lossy: bool },
    /// The file held more than the byte budget (or grew past it while reading).
    TooLarge { size_bytes: u64 },
    /// The file could not be opened or read.
    Error(std::io::Error),
}

/// Open `path` once and read at most `max_bytes` bytes from that descriptor.
///
/// Opening once and reading from the same descriptor removes the
/// metadata-then-read TOCTOU window: a file that grows after the size check
/// is still refused by the `max_bytes + 1` detection byte rather than read
/// without bound.
fn read_preview_text(path: &Path, max_bytes: u64) -> PreviewRead {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) => return PreviewRead::Error(error),
    };
    // Size at open time, used only for the honest oversize message; the
    // bounded read still refuses a file that grows after this point.
    let observed = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    match read_bounded(&mut file, max_bytes) {
        Ok(Some(bytes)) => {
            let is_lossy = std::str::from_utf8(&bytes).is_err();
            let text = match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(error) => String::from_utf8_lossy(error.as_bytes()).to_string(),
            };
            PreviewRead::Text { text, is_lossy }
        }
        Ok(None) => PreviewRead::TooLarge {
            size_bytes: observed.max(max_bytes.saturating_add(1)),
        },
        Err(error) => PreviewRead::Error(error),
    }
}

/// The bounded oversize state shared by both whole-file preview paths.
fn too_large_lines(size_bytes: u64, colors: &ThemeColors) -> Vec<Line<'static>> {
    let size_mb = size_bytes as f64 / (1024.0 * 1024.0);
    let msg = format!("File too large for full preview ({:.1} MB)", size_mb);
    vec![Line::from(Span::styled(
        msg,
        Style::default().fg(colors.warning_fg),
    ))]
}

/// Load and syntax-highlight a file's content, returning styled lines for ratatui.
///
/// Returns `(lines, total_line_count)`. On error, returns a single error-message line.
pub fn load_highlighted_content(
    path: &Path,
    ss: &SyntaxSet,
    theme: &Theme,
    colors: &ThemeColors,
) -> (Vec<Line<'static>>, usize) {
    crate::highlighting::note_render_io_if_watching();
    // Defense-in-depth bounded read: open once and take at most the hard-cap
    // budget plus one detection byte from that descriptor. Callers gate via
    // update_preview(), but this protects future callers that skip the check
    // and refuses a file that grows after any earlier metadata read.
    let (content, is_lossy) = match read_preview_text(path, MAX_PREVIEW_SIZE) {
        PreviewRead::Text { text, is_lossy } => (text, is_lossy),
        PreviewRead::TooLarge { size_bytes } => {
            return (too_large_lines(size_bytes, colors), 1);
        }
        PreviewRead::Error(e) => {
            let msg = format!("Error reading file: {}", e);
            return (
                vec![Line::from(Span::styled(
                    msg,
                    Style::default().fg(colors.error_fg),
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

    // Prepend UTF-8 lossy warning if applicable
    if is_lossy {
        result_lines.push(Line::from(Span::styled(
            "⚠ File contains non-UTF-8 bytes (showing lossy conversion)".to_string(),
            Style::default().fg(colors.warning_fg),
        )));
    }

    for (i, line_str) in lines_text.iter().enumerate() {
        result_lines.push(highlight_single_line(
            line_str,
            i + 1,
            line_num_width,
            &mut highlighter,
            ss,
            colors,
        ));
    }

    if result_lines.is_empty() {
        result_lines.push(Line::from(Span::styled(
            "(empty file)",
            Style::default().fg(colors.dim_fg),
        )));
    }

    (result_lines, total)
}

/// Syntax-highlight a string of content, using the given filename for language detection.
///
/// Returns (highlighted lines, total line count). Used for S3 head preview
/// where content is already in memory (not on disk).
pub fn highlight_content_from_string(
    content: &str,
    filename: &str,
    ss: &SyntaxSet,
    theme: &Theme,
    colors: &ThemeColors,
) -> (Vec<Line<'static>>, usize) {
    let path = std::path::Path::new(filename);
    let syntax_name = detect_syntax_name(path);
    let syntax = ss
        .find_syntax_by_name(syntax_name)
        .or_else(|| {
            path.extension()
                .and_then(|e| e.to_str())
                .and_then(|ext| ss.find_syntax_by_extension(ext))
        })
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    let mut highlighter = syntect::easy::HighlightLines::new(syntax, theme);
    let lines_text: Vec<&str> = content.lines().collect();
    let total = lines_text.len().max(1);
    let line_num_width = total.to_string().len();

    let mut result_lines = Vec::with_capacity(total);

    for (i, line_str) in lines_text.iter().enumerate() {
        result_lines.push(highlight_single_line(
            line_str,
            i + 1,
            line_num_width,
            &mut highlighter,
            ss,
            colors,
        ));
    }

    if result_lines.is_empty() {
        result_lines.push(Line::from(Span::styled(
            "(empty file)",
            Style::default().fg(colors.dim_fg),
        )));
    }

    (result_lines, total)
}

/// Count lines in a file using fast byte scanning (64KB chunks).
pub fn fast_line_count(path: &Path) -> std::io::Result<usize> {
    let mut file = fs::File::open(path)?;
    let mut buf = [0u8; 65536];
    let mut count = 0usize;
    let mut saw_bytes = false;
    let mut last_byte = None;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        saw_bytes = true;
        last_byte = Some(buf[n - 1]);
        count += buf[..n].iter().filter(|&&b| b == b'\n').count();
    }
    // If file has content and doesn't end with newline, the last line still counts.
    if saw_bytes && last_byte != Some(b'\n') {
        count += 1;
    }
    Ok(count)
}

/// Read the first `n` lines from a file using streaming I/O.
///
/// Only reads as many bytes as needed — never loads the entire file.
/// On I/O error mid-read, returns the lines collected so far (partial success).
pub fn read_head_lines(path: &Path, n: usize) -> std::io::Result<Vec<String>> {
    let file = fs::File::open(path)?;
    // Cap total bytes read so a pathological single line cannot materialize the
    // whole file; the final line may be clipped at the cap.
    let reader = BufReader::new(file.take(MAX_PREVIEW_SIZE));
    let mut result = Vec::with_capacity(n);
    for line_result in reader.lines().take(n) {
        match line_result {
            Ok(line) => result.push(line),
            Err(_) => break, // return partial results on I/O error
        }
    }
    Ok(result)
}

/// Read the last `n` lines from a file by seeking to the end and scanning backward.
///
/// Memory usage is bounded to O(n) regardless of file size.
pub fn read_tail_lines(path: &Path, n: usize) -> std::io::Result<Vec<String>> {
    use std::io::Seek;

    if n == 0 {
        return Ok(Vec::new());
    }

    let mut file = fs::File::open(path)?;
    let file_len = file.metadata()?.len();
    if file_len == 0 {
        return Ok(Vec::new());
    }

    // Scan backward in chunks to find n newlines
    const CHUNK_SIZE: u64 = 65536;
    let mut newline_positions: Vec<u64> = Vec::new();
    // If file ends with \n, skip it — it doesn't start a new line
    let mut offset = file_len;
    {
        let mut last_byte = [0u8];
        file.seek(std::io::SeekFrom::Start(file_len - 1))?;
        file.read_exact(&mut last_byte)?;
        if last_byte[0] == b'\n' {
            offset = file_len - 1;
        }
    }

    // Find n newlines scanning backward. Each newline marks a line boundary,
    // so n newlines delineate n tail lines.
    loop {
        let read_start = offset.saturating_sub(CHUNK_SIZE);
        let read_len = (offset - read_start) as usize;
        if read_len == 0 {
            break;
        }

        file.seek(std::io::SeekFrom::Start(read_start))?;
        let mut buf = vec![0u8; read_len];
        file.read_exact(&mut buf)?;

        for i in (0..read_len).rev() {
            if buf[i] == b'\n' {
                newline_positions.push(read_start + i as u64);
                if newline_positions.len() >= n {
                    break;
                }
            }
        }

        if newline_positions.len() >= n || read_start == 0 {
            break;
        }
        offset = read_start;
    }

    // Determine the byte offset where our tail lines begin
    let start_offset = if newline_positions.len() >= n {
        // Start right after the n-th newline from the end
        newline_positions[n - 1] + 1
    } else {
        // Fewer than n lines in file — start from beginning
        0
    };

    // Read from start_offset to end, bounded so a tail region larger than the
    // preview cap (for example a single line with no newlines, where
    // `start_offset` is 0) cannot materialize the whole file.
    file.seek(std::io::SeekFrom::Start(start_offset))?;
    let mut bytes = Vec::new();
    file.take(MAX_PREVIEW_SIZE).read_to_end(&mut bytes)?;
    let mut content = String::from_utf8_lossy(&bytes).to_string();

    // Strip trailing newline to avoid empty last element
    if content.ends_with('\n') {
        content.pop();
    }

    Ok(content.lines().map(|l| l.to_string()).collect())
}

/// Load head+tail content from a large file.
///
/// Returns styled lines with head section, separator, and tail section.
/// Uses streaming I/O — memory usage is bounded to O(head_lines + tail_lines).
pub fn load_head_tail_content(
    path: &Path,
    ss: &SyntaxSet,
    theme: &Theme,
    colors: &ThemeColors,
    head_lines: usize,
    tail_lines: usize,
    view_mode: ViewMode,
) -> (Vec<Line<'static>>, usize) {
    crate::highlighting::note_render_io_if_watching();
    let total_lines = match fast_line_count(path) {
        Ok(n) => n,
        Err(e) => {
            return (
                vec![Line::from(Span::styled(
                    format!("Error counting lines: {}", e),
                    Style::default().fg(colors.error_fg),
                ))],
                1,
            );
        }
    };

    let syntax_name = detect_syntax_name(path);
    let syntax = ss
        .find_syntax_by_name(syntax_name)
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let mut highlighter = syntect::easy::HighlightLines::new(syntax, theme);

    let line_num_width = total_lines.to_string().len();
    let mut result_lines: Vec<Line<'static>> = Vec::new();

    match view_mode {
        ViewMode::HeadAndTail => {
            let head = match read_head_lines(path, head_lines) {
                Ok(lines) => lines,
                Err(e) => {
                    return (
                        vec![Line::from(Span::styled(
                            format!("Error reading file: {}", e),
                            Style::default().fg(colors.error_fg),
                        ))],
                        1,
                    );
                }
            };
            let tail = match read_tail_lines(path, tail_lines) {
                Ok(lines) => lines,
                Err(e) => {
                    return (
                        vec![Line::from(Span::styled(
                            format!("Error reading file: {}", e),
                            Style::default().fg(colors.error_fg),
                        ))],
                        1,
                    );
                }
            };

            let effective_head = head.len();
            // Calculate tail start line number
            let tail_start_line = total_lines.saturating_sub(tail.len());

            // Head section
            for (i, line_str) in head.iter().enumerate() {
                result_lines.push(highlight_single_line(
                    line_str,
                    i + 1,
                    line_num_width,
                    &mut highlighter,
                    ss,
                    colors,
                ));
            }

            // Separator (if there are omitted lines between head and tail)
            if tail_start_line > effective_head {
                let omitted = tail_start_line - effective_head;
                let sep = format!("  ──── {} lines omitted ────", omitted);
                result_lines.push(Line::from(Span::styled(
                    sep,
                    Style::default()
                        .fg(colors.warning_fg)
                        .add_modifier(Modifier::DIM),
                )));
            }

            // Tail section
            for (i, line_str) in tail.iter().enumerate() {
                result_lines.push(highlight_single_line(
                    line_str,
                    tail_start_line + i + 1,
                    line_num_width,
                    &mut highlighter,
                    ss,
                    colors,
                ));
            }
        }
        ViewMode::HeadOnly => {
            let head = match read_head_lines(path, head_lines) {
                Ok(lines) => lines,
                Err(e) => {
                    return (
                        vec![Line::from(Span::styled(
                            format!("Error reading file: {}", e),
                            Style::default().fg(colors.error_fg),
                        ))],
                        1,
                    );
                }
            };
            for (i, line_str) in head.iter().enumerate() {
                result_lines.push(highlight_single_line(
                    line_str,
                    i + 1,
                    line_num_width,
                    &mut highlighter,
                    ss,
                    colors,
                ));
            }
        }
        ViewMode::TailOnly => {
            let tail = match read_tail_lines(path, tail_lines) {
                Ok(lines) => lines,
                Err(e) => {
                    return (
                        vec![Line::from(Span::styled(
                            format!("Error reading file: {}", e),
                            Style::default().fg(colors.error_fg),
                        ))],
                        1,
                    );
                }
            };
            let tail_start_line = total_lines.saturating_sub(tail.len());
            for (i, line_str) in tail.iter().enumerate() {
                result_lines.push(highlight_single_line(
                    line_str,
                    tail_start_line + i + 1,
                    line_num_width,
                    &mut highlighter,
                    ss,
                    colors,
                ));
            }
        }
    }

    // Return actual file line count, not displayed line count.
    // PreviewState.total_lines should always mean "total lines in file".
    (result_lines, total_lines)
}

/// Highlight a single line with line number prefix.
fn highlight_single_line(
    line_str: &str,
    line_num: usize,
    line_num_width: usize,
    highlighter: &mut syntect::easy::HighlightLines,
    ss: &SyntaxSet,
    colors: &ThemeColors,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();

    let num = format!("{:>width$} │ ", line_num, width = line_num_width);
    spans.push(Span::styled(
        num,
        Style::default().fg(colors.preview_line_nr_fg),
    ));

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
    // ML model formats
    "pt",
    "pth",
    "h5",
    "hdf5",
    "pkl",
    "pickle",
    "onnx",
    "safetensors",
    "parquet",
    "arrow",
    "avro",
    // Archives
    "zip",
    "tar",
    "gz",
    "bz2",
    "xz",
    "7z",
    "rar",
    "lz4",
    "zst",
    // Shared libraries / executables
    "so",
    "dylib",
    "exe",
    "bin",
    "img",
    "iso",
    // Compiled / object files
    "wasm",
    "pyc",
    "pyo",
    "class",
    "o",
    "a",
    "lib",
    "dll",
    // System packages
    "deb",
    "rpm",
    // Images
    "png",
    "jpg",
    "jpeg",
    "gif",
    "bmp",
    "ico",
    "webp",
    // Audio / video
    "mp3",
    "mp4",
    "wav",
    "flac",
    // Documents (binary)
    "pdf",
    "doc",
    "docx",
    "xls",
    "xlsx",
    "ppt",
    "pptx",
];

/// Check if a file is binary by extension or null-byte scan.
pub fn is_binary_file(path: &Path) -> bool {
    crate::highlighting::note_render_io_if_watching();
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
pub fn load_binary_metadata(path: &Path, colors: &ThemeColors) -> (Vec<Line<'static>>, usize) {
    crate::highlighting::note_render_io_if_watching();
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return (
                vec![Line::from(Span::styled(
                    format!("Error reading metadata: {}", e),
                    Style::default().fg(colors.error_fg),
                ))],
                1,
            );
        }
    };

    let label_style = Style::default()
        .fg(colors.info_fg)
        .add_modifier(Modifier::BOLD);
    let value_style = Style::default().fg(colors.preview_fg);
    let dim_style = Style::default().fg(colors.dim_fg);

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

    #[cfg(unix)]
    let perms_str = format_permissions(meta.permissions().mode());
    #[cfg(not(unix))]
    let perms_str = if meta.permissions().readonly() {
        "Read-only".to_string()
    } else {
        "Read/Write".to_string()
    };

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
///
/// Guards against integer overflow when casting large `u64` values to `i64`.
fn epoch_days_to_date(days: u64) -> (u64, u64, u64) {
    // Guard against values that would overflow i64
    let mut remaining = match i64::try_from(days) {
        Ok(d) => d,
        Err(_) => return (9999, 12, 31), // fallback for astronomically large values
    };
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

/// Generate a summary display for a directory.
///
/// Shows: directory name, file count, subdirectory count, total size.
/// Caps recursive walk to avoid hanging on huge trees.
#[allow(dead_code)]
pub fn load_directory_summary(path: &Path, colors: &ThemeColors) -> (Vec<Line<'static>>, usize) {
    let label_style = Style::default()
        .fg(colors.info_fg)
        .add_modifier(Modifier::BOLD);
    let value_style = Style::default().fg(colors.preview_fg);

    let dir_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());

    let mut file_count: u64 = 0;
    let mut dir_count: u64 = 0;
    let mut total_size: u64 = 0;
    let mut entries_scanned: u64 = 0;
    const MAX_ENTRIES: u64 = 10_000;

    // Walk directory iteratively with a stack
    let mut stack = vec![path.to_path_buf()];
    let mut capped = false;
    // Symlink loop protection (same pattern as spawn_async_dir_summary)
    let mut visited = crate::fs::tree::VisitedDirs::new();

    while let Some(current) = stack.pop() {
        let entries = match fs::read_dir(&current) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            entries_scanned += 1;
            if entries_scanned > MAX_ENTRIES {
                capped = true;
                break;
            }

            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            if meta.is_dir() {
                dir_count += 1;
                // Only recurse if we haven't visited this directory before
                // (prevents infinite loops on circular symlinks)
                if visited.visit(&entry.path()) {
                    stack.push(entry.path());
                }
            } else {
                file_count += 1;
                total_size += meta.len();
            }
        }

        if capped {
            break;
        }
    }

    let size_str = format_size(total_size);

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Directory: ", label_style),
            Span::styled(dir_name, value_style),
        ]),
        Line::from(vec![
            Span::styled("  Files: ", label_style),
            Span::styled(file_count.to_string(), value_style),
        ]),
        Line::from(vec![
            Span::styled("  Subdirectories: ", label_style),
            Span::styled(dir_count.to_string(), value_style),
        ]),
        Line::from(vec![
            Span::styled("  Total Size: ", label_style),
            Span::styled(size_str, value_style),
        ]),
    ];

    if capped {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  (scan capped at {} entries)", MAX_ENTRIES),
            Style::default().fg(colors.warning_fg),
        )));
    }

    let total = lines.len();
    (lines, total)
}

/// Maximum number of child items to list in the shallow directory preview.
const SHALLOW_LISTING_LIMIT: usize = 20;

/// Generate a shallow (depth-1) summary display for a directory.
///
/// Only counts immediate children — no recursive walk. Shows:
/// - directory name, immediate file count, immediate subdirectory count
/// - first N child item names as a quick listing
/// - hint for deep scan
pub fn load_directory_summary_shallow(
    path: &Path,
    colors: &ThemeColors,
) -> (Vec<Line<'static>>, usize) {
    crate::highlighting::note_render_io_if_watching();
    let summary =
        load_directory_summary_shallow_budgeted(path, colors, usize::MAX, usize::MAX, || false);
    (summary.lines, summary.total)
}

pub(crate) struct ShallowSummary {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) total: usize,
    pub(crate) complete: bool,
}

pub(crate) fn summary_lines_bytes(lines: &Vec<Line<'static>>) -> usize {
    lines.iter().fold(
        lines
            .capacity()
            .saturating_mul(std::mem::size_of::<Line<'static>>()),
        |bytes, line| {
            line.spans.iter().fold(
                bytes.saturating_add(
                    line.spans
                        .capacity()
                        .saturating_mul(std::mem::size_of::<Span<'static>>()),
                ),
                |bytes, span| {
                    bytes.saturating_add(match &span.content {
                        std::borrow::Cow::Owned(text) => text.capacity(),
                        std::borrow::Cow::Borrowed(_) => 0,
                    })
                },
            )
        },
    )
}

/// Existing formatting policy with native worker cancellation/retention seams.
/// At most 20 names and 32 rendered lines; lossily decoded names <=12 KiB each.
pub(crate) fn load_directory_summary_shallow_budgeted(
    path: &Path,
    colors: &ThemeColors,
    max_entries: usize,
    bytes: usize,
    stopped: impl Fn() -> bool,
) -> ShallowSummary {
    // Reserve all fixed formatting vectors, numeric text and directory label
    // before collecting nested names or building rendered lines.
    const FORMATTING_BYTES: usize = 32 * 1024;
    if path.as_os_str().len() > 4096 || bytes < FORMATTING_BYTES {
        return ShallowSummary {
            lines: Vec::new(),
            total: 0,
            complete: false,
        };
    }
    let label_style = Style::default()
        .fg(colors.info_fg)
        .add_modifier(Modifier::BOLD);
    let value_style = Style::default().fg(colors.preview_fg);
    let dim_style = Style::default().fg(colors.dim_fg);
    let hint_style = Style::default().fg(colors.warning_fg);

    let dir_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());

    let mut file_count: u64 = 0;
    let mut dir_count: u64 = 0;
    let mut child_names: Vec<(String, bool)> = Vec::with_capacity(SHALLOW_LISTING_LIMIT);
    let mut names_bytes = 0usize;
    let mut complete = true;

    let entries = match if stopped() {
        None
    } else {
        Some(fs::read_dir(path))
    } {
        None => {
            complete = false;
            None
        }
        Some(Ok(entries)) => Some(entries),
        Some(Err(_)) => {
            return ShallowSummary {
                lines: vec![Line::from(Span::styled(
                    "  Incomplete: Error reading directory (I/O)",
                    Style::default().fg(colors.error_fg),
                ))],
                total: 1,
                complete: false,
            };
        }
    };

    for (scanned, entry) in entries.into_iter().flatten().enumerate() {
        if stopped() || scanned == max_entries {
            complete = false;
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        if stopped() {
            complete = false;
            break;
        }
        let is_dir = match entry.metadata() {
            Ok(metadata) => metadata.is_dir(),
            Err(_) => {
                complete = false;
                continue;
            }
        };

        if is_dir {
            dir_count += 1;
        } else {
            file_count += 1;
        }

        if child_names.len() < SHALLOW_LISTING_LIMIT {
            let raw = entry.file_name();
            let charge = raw.len().saturating_mul(6).saturating_add(32);
            if raw.capacity() > 4096
                || charge > bytes.saturating_sub(FORMATTING_BYTES + names_bytes)
            {
                complete = false;
                break;
            }
            let name = raw.to_string_lossy().to_string();
            names_bytes += charge;
            child_names.push((name, is_dir));
        }
    }

    // Sort child names: directories first, then alphabetically
    child_names.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  📁 Directory: ", label_style),
            Span::styled(dir_name, value_style),
        ]),
        Line::from(vec![
            Span::styled("  Files: ", label_style),
            Span::styled(format!("{} (direct)", file_count), value_style),
        ]),
        Line::from(vec![
            Span::styled("  Subdirectories: ", label_style),
            Span::styled(format!("{} (direct)", dir_count), value_style),
        ]),
        Line::from(""),
    ];

    // Child listing
    let total_children = file_count + dir_count;
    if total_children > 0 {
        lines.push(Line::from(Span::styled("  Contents:", label_style)));

        for (name, is_dir) in &child_names {
            let icon = if *is_dir { "📂 " } else { "  📄 " };
            lines.push(Line::from(Span::styled(
                format!("    {}{}", icon, name),
                dim_style,
            )));
        }

        let remaining = total_children as usize - child_names.len();
        if remaining > 0 {
            lines.push(Line::from(Span::styled(
                format!("    ... and {} more", remaining),
                dim_style,
            )));
        }
    }

    // Deep scan hint
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  [Press D for deep scan]",
        hint_style,
    )));

    if !complete {
        lines.push(Line::raw(
            "  Incomplete (cap/deadline/I/O); totals are partial",
        ));
    }
    let total = lines.len();
    ShallowSummary {
        lines,
        total,
        complete,
    }
}

/// Load and render a Jupyter notebook (.ipynb) file.
///
/// Parses the JSON structure and renders cells with headers, source code
/// (syntax-highlighted for code cells), and text outputs.
pub fn load_notebook_content(
    path: &Path,
    ss: &SyntaxSet,
    theme: &Theme,
    colors: &ThemeColors,
    max_bytes: u64,
) -> (Vec<Line<'static>>, usize) {
    crate::highlighting::note_render_io_if_watching();
    // The notebook path takes an explicit budget so a large `.ipynb` can never
    // be read whole. The read is bounded before parsing, so the JSON parser
    // can never observe bytes beyond `max_bytes`.
    let content = match read_preview_text(path, max_bytes) {
        PreviewRead::Text { text, .. } => text,
        PreviewRead::TooLarge { size_bytes } => {
            return (too_large_lines(size_bytes, colors), 1);
        }
        PreviewRead::Error(e) => {
            return (
                vec![Line::from(Span::styled(
                    format!("Error reading notebook: {}", e),
                    Style::default().fg(colors.error_fg),
                ))],
                1,
            );
        }
    };

    let notebook: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            return (
                vec![Line::from(Span::styled(
                    format!("Error parsing notebook JSON: {}", e),
                    Style::default().fg(colors.error_fg),
                ))],
                1,
            );
        }
    };

    let cells = match notebook.get("cells").and_then(|c| c.as_array()) {
        Some(c) => c,
        None => {
            return (
                vec![Line::from(Span::styled(
                    "Invalid notebook: no cells array found",
                    Style::default().fg(colors.error_fg),
                ))],
                1,
            );
        }
    };

    // Detect kernel language for code cell highlighting
    let kernel_lang = notebook
        .pointer("/metadata/kernelspec/language")
        .and_then(|v| v.as_str())
        .unwrap_or("python");
    let kernel_ext = format!("_.{}", kernel_lang);
    let kernel_syntax_name = detect_syntax_name(Path::new(&kernel_ext));

    let header_style = Style::default()
        .fg(colors.warning_fg)
        .add_modifier(Modifier::BOLD);
    let output_prefix_style = Style::default()
        .fg(colors.success_fg)
        .add_modifier(Modifier::BOLD);
    let dim_style = Style::default().fg(colors.dim_fg);

    let mut lines: Vec<Line<'static>> = Vec::new();

    for (i, cell) in cells.iter().enumerate() {
        let cell_type = cell
            .get("cell_type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        // Cell header
        lines.push(Line::from(Span::styled(
            format!("━━━ Cell {} [{}] ━━━", i + 1, cell_type),
            header_style,
        )));

        // Cell source
        let source = extract_notebook_text(cell.get("source"));
        if !source.is_empty() {
            if cell_type == "code" {
                // Syntax-highlight code cells
                let syntax = ss
                    .find_syntax_by_name(kernel_syntax_name)
                    .unwrap_or_else(|| ss.find_syntax_plain_text());
                let mut highlighter = syntect::easy::HighlightLines::new(syntax, theme);

                for line_str in source.lines() {
                    let mut spans: Vec<Span<'static>> = Vec::new();
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
                    lines.push(Line::from(spans));
                }
            } else {
                // Markdown/raw cells: plain text
                for line_str in source.lines() {
                    lines.push(Line::from(line_str.to_string()));
                }
            }
        }

        // Cell outputs (only for code cells)
        if cell_type == "code" {
            if let Some(outputs) = cell.get("outputs").and_then(|o| o.as_array()) {
                for output in outputs {
                    let output_type = output
                        .get("output_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    match output_type {
                        "stream" => {
                            let text = extract_notebook_text(output.get("text"));
                            if !text.is_empty() {
                                for line_str in text.lines() {
                                    lines.push(Line::from(vec![
                                        Span::styled("[Out] ", output_prefix_style),
                                        Span::raw(line_str.to_string()),
                                    ]));
                                }
                            }
                        }
                        "execute_result" | "display_data" => {
                            // Only render text/plain from data
                            if let Some(data) = output.get("data") {
                                let text = extract_notebook_text(data.get("text/plain"));
                                if !text.is_empty() {
                                    for line_str in text.lines() {
                                        lines.push(Line::from(vec![
                                            Span::styled("[Out] ", output_prefix_style),
                                            Span::raw(line_str.to_string()),
                                        ]));
                                    }
                                }
                            }
                        }
                        "error" => {
                            if let Some(traceback) =
                                output.get("traceback").and_then(|t| t.as_array())
                            {
                                for tb_line in traceback {
                                    if let Some(s) = tb_line.as_str() {
                                        // Strip ANSI escape codes
                                        let clean = strip_ansi(s);
                                        lines.push(Line::from(Span::styled(
                                            clean,
                                            Style::default().fg(colors.error_fg),
                                        )));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Separator between cells
        lines.push(Line::from(Span::styled("", dim_style)));
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "(empty notebook)",
            Style::default().fg(colors.dim_fg),
        )));
    }

    let total = lines.len();
    (lines, total)
}

/// Extract text from a notebook source/text field.
///
/// Notebook fields can be either a string or an array of strings.
fn extract_notebook_text(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Strip ANSI escape codes from a string.
fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip until we find a letter (end of escape sequence)
            while let Some(&next) = chars.peek() {
                chars.next();
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    fn test_colors() -> crate::theme::ThemeColors {
        crate::theme::dark_theme()
    }

    #[test]
    fn detect_syntax_by_extension() {
        assert_eq!(detect_syntax_name(Path::new("foo.rs")), "Rust");
        assert_eq!(detect_syntax_name(Path::new("bar.py")), "Python");
        assert_eq!(detect_syntax_name(Path::new("baz.yml")), "YAML");
        assert_eq!(detect_syntax_name(Path::new("config.toml")), "TOML");
        assert_eq!(detect_syntax_name(Path::new("style.css")), "CSS");
        assert_eq!(detect_syntax_name(Path::new("page.html")), "HTML");
        assert_eq!(detect_syntax_name(Path::new("app.tsx")), "TypeScript");
        assert_eq!(detect_syntax_name(Path::new("Makefile")), "Makefile");
        assert_eq!(detect_syntax_name(Path::new("readme.md")), "Markdown");
    }

    #[test]
    fn detect_syntax_new_extensions() {
        // FR-5: expanded coverage
        assert_eq!(detect_syntax_name(Path::new("script.lua")), "Lua");
        assert_eq!(detect_syntax_name(Path::new("app.php")), "PHP");
        assert_eq!(detect_syntax_name(Path::new("main.swift")), "Swift");
        assert_eq!(detect_syntax_name(Path::new("Main.kt")), "Kotlin");
        assert_eq!(detect_syntax_name(Path::new("build.kts")), "Kotlin");
        assert_eq!(detect_syntax_name(Path::new("App.scala")), "Scala");
        assert_eq!(detect_syntax_name(Path::new("script.r")), "R");
        assert_eq!(detect_syntax_name(Path::new("main.tf")), "Terraform");
        assert_eq!(detect_syntax_name(Path::new("flake.nix")), "Nix");
        assert_eq!(detect_syntax_name(Path::new("main.zig")), "Zig");
        assert_eq!(detect_syntax_name(Path::new("shader.glsl")), "GLSL");
        assert_eq!(detect_syntax_name(Path::new("data.xml")), "XML");
        assert_eq!(detect_syntax_name(Path::new("icon.svg")), "XML");
    }

    #[test]
    fn detect_syntax_by_filename() {
        // FR-5: filename-based detection
        assert_eq!(detect_syntax_name(Path::new("Dockerfile")), "Dockerfile");
        assert_eq!(detect_syntax_name(Path::new("makefile")), "Makefile");
        assert_eq!(detect_syntax_name(Path::new(".env")), "Bash");
        assert_eq!(detect_syntax_name(Path::new(".gitignore")), "Plain Text");
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
        let (lines, total) = load_highlighted_content(&path, &ss, &theme, &test_colors());
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
        let (lines, total) = load_highlighted_content(&path, &ss, &theme, &test_colors());
        assert_eq!(total, 1);
        assert!(!lines.is_empty());
    }

    #[test]
    fn highlight_nonexistent_file() {
        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, total) =
            load_highlighted_content(Path::new("/nonexistent"), &ss, &theme, &test_colors());
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

    #[test]
    fn fast_line_count_multiline_no_trailing_newline() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("multiline_no_nl.txt");
        let mut f = File::create(&path).unwrap();
        write!(f, "line 1\nline 2\nline 3").unwrap();
        assert_eq!(fast_line_count(&path).unwrap(), 3);
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
        let (lines, _) = load_head_tail_content(
            &path,
            &ss,
            &theme,
            &test_colors(),
            10,
            5,
            ViewMode::HeadAndTail,
        );
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
        let (lines, _) = load_head_tail_content(
            &path,
            &ss,
            &theme,
            &test_colors(),
            10,
            5,
            ViewMode::HeadOnly,
        );
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
        let (lines, _) = load_head_tail_content(
            &path,
            &ss,
            &theme,
            &test_colors(),
            10,
            5,
            ViewMode::TailOnly,
        );
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
    fn binary_detection_new_extensions() {
        // FR-6: expanded coverage
        // ML formats
        assert!(is_binary_file(Path::new("model.safetensors")));
        assert!(is_binary_file(Path::new("data.parquet")));
        assert!(is_binary_file(Path::new("data.arrow")));
        assert!(is_binary_file(Path::new("data.avro")));
        // Compiled
        assert!(is_binary_file(Path::new("module.wasm")));
        assert!(is_binary_file(Path::new("cache.pyc")));
        assert!(is_binary_file(Path::new("App.class")));
        assert!(is_binary_file(Path::new("main.o")));
        assert!(is_binary_file(Path::new("lib.dll")));
        // Packages
        assert!(is_binary_file(Path::new("pkg.deb")));
        assert!(is_binary_file(Path::new("pkg.rpm")));
        assert!(is_binary_file(Path::new("file.7z")));
        assert!(is_binary_file(Path::new("file.rar")));
        // Images
        assert!(is_binary_file(Path::new("photo.png")));
        assert!(is_binary_file(Path::new("photo.jpg")));
        assert!(is_binary_file(Path::new("icon.ico")));
        // Media
        assert!(is_binary_file(Path::new("song.mp3")));
        assert!(is_binary_file(Path::new("video.mp4")));
        // Documents
        assert!(is_binary_file(Path::new("doc.pdf")));
        assert!(is_binary_file(Path::new("doc.docx")));
        assert!(is_binary_file(Path::new("sheet.xlsx")));
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

        let (lines, total) = load_binary_metadata(&path, &test_colors());
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
        let (lines, total) = load_binary_metadata(Path::new("/nonexistent/file"), &test_colors());
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

    // === Directory summary tests ===

    #[test]
    fn directory_summary_basic() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();
        let mut f = File::create(dir.path().join("file.txt")).unwrap();
        writeln!(f, "hello world").unwrap();
        File::create(dir.path().join("file2.txt")).unwrap();

        let (lines, total) = load_directory_summary(dir.path(), &test_colors());
        assert!(total >= 5);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("Files:"));
        assert!(all_text.contains("2"));
        assert!(all_text.contains("Subdirectories:"));
        assert!(all_text.contains("1"));
    }

    #[test]
    fn directory_summary_empty_dir() {
        let dir = TempDir::new().unwrap();
        let (lines, total) = load_directory_summary(dir.path(), &test_colors());
        assert!(total >= 5);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("Files:"));
        assert!(all_text.contains("0"));
        assert!(all_text.contains("0 B"));
    }

    #[test]
    fn directory_summary_nested() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("a/b")).unwrap();
        File::create(dir.path().join("a/b/deep.txt")).unwrap();

        let (lines, _) = load_directory_summary(dir.path(), &test_colors());
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        // Should count nested file and both subdirs
        assert!(all_text.contains("1")); // 1 file
        assert!(all_text.contains("2")); // 2 subdirs (a, b)
    }

    // === Shallow directory summary tests ===

    #[test]
    fn shallow_summary_empty_dir() {
        let dir = TempDir::new().unwrap();
        let (lines, total) = load_directory_summary_shallow(dir.path(), &test_colors());
        assert!(total >= 3);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("0 (direct)"));
        assert!(all_text.contains("[Press D for deep scan]"));
    }

    #[test]
    fn shallow_summary_basic() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();
        File::create(dir.path().join("file.txt")).unwrap();
        File::create(dir.path().join("file2.txt")).unwrap();

        let (lines, _) = load_directory_summary_shallow(dir.path(), &test_colors());
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("2 (direct)")); // 2 files
        assert!(all_text.contains("1 (direct)")); // 1 subdir
        assert!(all_text.contains("Contents:"));
        assert!(all_text.contains("file.txt"));
        assert!(all_text.contains("subdir"));
    }

    #[test]
    fn shallow_summary_does_not_recurse() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("a/b")).unwrap();
        File::create(dir.path().join("a/b/deep.txt")).unwrap();

        let (lines, _) = load_directory_summary_shallow(dir.path(), &test_colors());
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        // Should only see 1 subdir (a), NOT 2 (a + b)
        // Should see 0 files (deep.txt is nested)
        assert!(all_text.contains("0 (direct)")); // 0 direct files
        assert!(all_text.contains("1 (direct)")); // 1 direct subdir
        assert!(!all_text.contains("deep.txt"));
    }

    #[test]
    fn shallow_summary_child_listing_limit() {
        let dir = TempDir::new().unwrap();
        for i in 0..25 {
            File::create(dir.path().join(format!("file_{:02}.txt", i))).unwrap();
        }

        let (lines, _) = load_directory_summary_shallow(dir.path(), &test_colors());
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("25 (direct)"));
        assert!(all_text.contains("... and 5 more"));
    }

    #[test]
    fn shallow_summary_has_deep_scan_hint() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join("a.txt")).unwrap();

        let (lines, _) = load_directory_summary_shallow(dir.path(), &test_colors());
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("[Press D for deep scan]"));
    }

    // === Notebook rendering tests ===

    #[test]
    fn notebook_basic_rendering() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.ipynb");
        let notebook = r##"{
            "cells": [
                {
                    "cell_type": "code",
                    "source": ["print('hello')\n", "x = 1"],
                    "outputs": [
                        {
                            "output_type": "stream",
                            "text": ["hello\n"]
                        }
                    ]
                },
                {
                    "cell_type": "markdown",
                    "source": ["# Title"]
                }
            ],
            "metadata": {
                "kernelspec": {
                    "language": "python"
                }
            }
        }"##;
        let mut f = File::create(&path).unwrap();
        f.write_all(notebook.as_bytes()).unwrap();

        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, total) =
            load_notebook_content(&path, &ss, &theme, &test_colors(), MAX_PREVIEW_SIZE);
        assert!(total > 0);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("Cell 1"));
        assert!(all_text.contains("code"));
        assert!(all_text.contains("Cell 2"));
        assert!(all_text.contains("markdown"));
        assert!(all_text.contains("[Out]"));
        assert!(all_text.contains("hello"));
    }

    #[test]
    fn notebook_execute_result_output() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test2.ipynb");
        let notebook = r#"{
            "cells": [
                {
                    "cell_type": "code",
                    "source": ["42"],
                    "outputs": [
                        {
                            "output_type": "execute_result",
                            "data": {
                                "text/plain": ["42"]
                            }
                        }
                    ]
                }
            ],
            "metadata": {}
        }"#;
        let mut f = File::create(&path).unwrap();
        f.write_all(notebook.as_bytes()).unwrap();

        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, _) =
            load_notebook_content(&path, &ss, &theme, &test_colors(), MAX_PREVIEW_SIZE);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("[Out]"));
        assert!(all_text.contains("42"));
    }

    #[test]
    fn notebook_invalid_json() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.ipynb");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"not json").unwrap();

        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, total) =
            load_notebook_content(&path, &ss, &theme, &test_colors(), MAX_PREVIEW_SIZE);
        assert_eq!(total, 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("Error"));
    }

    #[test]
    fn notebook_no_cells() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.ipynb");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"{}").unwrap();

        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, total) =
            load_notebook_content(&path, &ss, &theme, &test_colors(), MAX_PREVIEW_SIZE);
        assert_eq!(total, 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("no cells"));
    }

    // === ANSI strip tests ===

    #[test]
    fn strip_ansi_removes_codes() {
        assert_eq!(strip_ansi("hello"), "hello");
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(strip_ansi("\x1b[1;32mbold green\x1b[0m"), "bold green");
    }

    // === Edge case tests ===

    #[test]
    fn highlight_empty_file_shows_placeholder() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.txt");
        File::create(&path).unwrap();

        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, total) = load_highlighted_content(&path, &ss, &theme, &test_colors());
        assert_eq!(total, 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("empty file"));
    }

    #[test]
    fn highlight_permission_denied_shows_error() {
        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        // Non-existent path simulates permission denied scenario
        let (lines, total) =
            load_highlighted_content(Path::new("/nonexistent/file"), &ss, &theme, &test_colors());
        assert_eq!(total, 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("Error"));
    }

    #[test]
    fn binary_detection_empty_file_not_binary() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.dat");
        File::create(&path).unwrap();
        // Empty file with unknown extension should not be binary (no null bytes)
        assert!(!is_binary_file(&path));
    }

    #[test]
    fn format_size_large_values() {
        assert_eq!(format_size(1024 * 1024 * 1024 * 1024), "1.00 TB");
    }

    #[test]
    fn notebook_source_as_string() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("str_source.ipynb");
        let notebook =
            r#"{"cells":[{"cell_type":"code","source":"x=1","outputs":[]}],"metadata":{}}"#;
        let mut f = File::create(&path).unwrap();
        f.write_all(notebook.as_bytes()).unwrap();

        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, total) =
            load_notebook_content(&path, &ss, &theme, &test_colors(), MAX_PREVIEW_SIZE);
        assert!(total > 0);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("Cell 1"));
    }

    #[test]
    fn zero_byte_file_line_count() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("zero.txt");
        File::create(&path).unwrap();
        assert_eq!(fast_line_count(&path).unwrap(), 0);
    }

    // === Streaming read_head_lines / read_tail_lines tests ===

    #[test]
    fn read_head_lines_returns_n_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("big.txt");
        let mut f = File::create(&path).unwrap();
        for i in 0..10_000 {
            writeln!(f, "line {}", i).unwrap();
        }

        let head = read_head_lines(&path, 50).unwrap();
        assert_eq!(head.len(), 50);
        assert_eq!(head[0], "line 0");
        assert_eq!(head[49], "line 49");
    }

    #[test]
    fn read_head_lines_small_file_returns_all() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("small.txt");
        let mut f = File::create(&path).unwrap();
        for i in 0..5 {
            writeln!(f, "line {}", i).unwrap();
        }

        let head = read_head_lines(&path, 50).unwrap();
        assert_eq!(head.len(), 5);
        assert_eq!(head[4], "line 4");
    }

    #[test]
    fn read_tail_lines_returns_last_m() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("big.txt");
        let mut f = File::create(&path).unwrap();
        for i in 0..10_000 {
            writeln!(f, "line {}", i).unwrap();
        }

        let tail = read_tail_lines(&path, 20).unwrap();
        assert_eq!(tail.len(), 20);
        assert_eq!(tail[0], "line 9980");
        assert_eq!(tail[19], "line 9999");
    }

    #[test]
    fn read_tail_lines_small_file_returns_all() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("small.txt");
        let mut f = File::create(&path).unwrap();
        for i in 0..3 {
            writeln!(f, "line {}", i).unwrap();
        }

        let tail = read_tail_lines(&path, 20).unwrap();
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0], "line 0");
        assert_eq!(tail[2], "line 2");
    }

    #[test]
    fn read_tail_lines_no_trailing_newline() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("no_nl.txt");
        let mut f = File::create(&path).unwrap();
        write!(f, "line 0\nline 1\nline 2").unwrap();

        let tail = read_tail_lines(&path, 2).unwrap();
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0], "line 1");
        assert_eq!(tail[1], "line 2");
    }

    #[test]
    fn streaming_head_tail_integration() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("medium.txt");
        let mut f = File::create(&path).unwrap();
        for i in 0..1000 {
            writeln!(f, "content line {}", i).unwrap();
        }

        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, total) = load_head_tail_content(
            &path,
            &ss,
            &theme,
            &test_colors(),
            10,
            5,
            ViewMode::HeadAndTail,
        );
        // 10 head + 1 separator + 5 tail = 16 display lines
        // But total_lines should be the actual file line count
        assert_eq!(total, 1000);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("content line 0"));
        assert!(all_text.contains("content line 9"));
        assert!(all_text.contains("lines omitted"));
        assert!(all_text.contains("content line 999"));
    }

    // === FR-1: Defense-in-depth size guard tests ===

    #[test]
    fn size_guard_rejects_large_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("huge.txt");
        // Create a file slightly over 5MB
        let mut f = File::create(&path).unwrap();
        let chunk = vec![b'A'; 1024];
        for _ in 0..(5 * 1024 + 1) {
            f.write_all(&chunk).unwrap();
        }

        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, total) = load_highlighted_content(&path, &ss, &theme, &test_colors());
        assert_eq!(total, 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("too large"));
    }

    #[test]
    fn size_guard_allows_small_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("small.rs");
        let mut f = File::create(&path).unwrap();
        writeln!(f, "fn main() {{}}").unwrap();

        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, total) = load_highlighted_content(&path, &ss, &theme, &test_colors());
        assert_eq!(total, 1);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!text.contains("too large"));
    }

    // === FileManagerTUI-8u3: bounded preview reads ===

    #[test]
    fn bounded_reader_never_exceeds_limit_under_growth() {
        use std::io::Cursor;
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::{Arc, Barrier};

        struct GrowingReader {
            prefix: Cursor<Vec<u8>>,
            grown: bool,
            barrier: Arc<Barrier>,
            served: Arc<AtomicU64>,
        }

        impl Read for GrowingReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if !self.grown {
                    let n = self.prefix.read(buf)?;
                    if n > 0 {
                        self.served.fetch_add(n as u64, Ordering::SeqCst);
                        return Ok(n);
                    }
                    self.grown = true;
                    // Release the growth only once the test is also waiting:
                    // deterministic coordination instead of a sleep.
                    self.barrier.wait();
                }
                let chunk = [b'x'; 4096];
                let n = chunk.len().min(buf.len());
                buf[..n].copy_from_slice(&chunk[..n]);
                self.served.fetch_add(n as u64, Ordering::SeqCst);
                Ok(n)
            }
        }

        let limit = 64u64;
        let barrier = Arc::new(Barrier::new(2));
        let served = Arc::new(AtomicU64::new(0));
        let reader = GrowingReader {
            prefix: Cursor::new(vec![b'a'; limit as usize]),
            grown: false,
            barrier: barrier.clone(),
            served: served.clone(),
        };

        let handle = std::thread::spawn(move || {
            let mut reader = reader;
            read_bounded(&mut reader, limit)
        });
        // The reader blocks at the barrier only after serving the first
        // `limit` bytes; releasing it here forces the growth deterministically.
        barrier.wait();
        let result = handle.join().unwrap().unwrap();

        assert!(
            result.is_none(),
            "a source grown past the budget is refused"
        );
        let consumed = served.load(Ordering::SeqCst);
        assert!(
            consumed <= limit + 1,
            "bounded reader consumed {consumed} bytes for a limit of {limit}"
        );
    }

    #[test]
    fn read_bounded_file_consumes_at_most_budget_plus_one() {
        use std::io::Seek;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("oversize.txt");
        let budget = 16 * 1024u64;
        std::fs::write(&path, vec![b'z'; (budget * 8) as usize]).unwrap();

        let mut file = File::open(&path).unwrap();
        let result = read_bounded(&mut file, budget).unwrap();
        assert!(result.is_none(), "a file over budget must be refused");
        // The descriptor position proves only budget + 1 bytes were consumed,
        // so memory never scales with the file.
        assert_eq!(file.stream_position().unwrap(), budget + 1);
    }

    #[test]
    fn bounded_reader_propagates_read_errors() {
        struct FailingReader;
        impl Read for FailingReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("injected read failure"))
            }
        }

        let error = read_bounded(FailingReader, 16).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
    }

    #[test]
    fn unreadable_preview_read_reports_error() {
        // A directory opens as a descriptor on Unix but fails on read, which
        // exercises the read-error arm of the bounded preview reader.
        let dir = TempDir::new().unwrap();
        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, total) = load_highlighted_content(dir.path(), &ss, &theme, &test_colors());
        assert_eq!(total, 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("Error reading file"), "got: {text}");
    }

    #[test]
    fn notebook_nonexistent_file_reports_read_error() {
        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, total) = load_notebook_content(
            Path::new("/nonexistent/missing.ipynb"),
            &ss,
            &theme,
            &test_colors(),
            MAX_PREVIEW_SIZE,
        );
        assert_eq!(total, 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("Error reading notebook"), "got: {text}");
    }

    #[test]
    fn oversize_notebook_is_bounded_and_not_parsed() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("huge.ipynb");
        // A valid notebook prefix followed by whitespace: the whole buffer is
        // valid JSON and just above the budget. A parser that saw a truncated
        // buffer would instead return the "no cells" state.
        let mut body = String::from(r#"{"cells":[],"metadata":{}}"#);
        body.push_str(&" ".repeat(8192));
        std::fs::write(&path, body.as_bytes()).unwrap();

        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, total) = load_notebook_content(&path, &ss, &theme, &test_colors(), 512);
        assert_eq!(total, 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("too large"), "got: {text}");
        assert!(
            !text.contains("no cells"),
            "the JSON parse must not observe a truncated buffer: {text}"
        );
    }

    // === FR-2: UTF-8 lossy warning tests ===

    #[test]
    fn utf8_lossy_shows_warning() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("mixed.txt");
        let mut f = File::create(&path).unwrap();
        // Valid UTF-8 followed by invalid bytes followed by more valid text
        f.write_all(b"hello ").unwrap();
        f.write_all(&[0x80, 0x81, 0x82]).unwrap();
        f.write_all(b" world\n").unwrap();

        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, _total) = load_highlighted_content(&path, &ss, &theme, &test_colors());
        // First line should be the warning
        let first_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(first_text.contains("non-UTF-8"));
    }

    #[test]
    fn valid_utf8_no_warning() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("valid.txt");
        let mut f = File::create(&path).unwrap();
        writeln!(f, "This is valid UTF-8").unwrap();

        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (lines, _) = load_highlighted_content(&path, &ss, &theme, &test_colors());
        let first_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!first_text.contains("non-UTF-8"));
    }

    // === FR-3: total_lines semantics test ===

    #[test]
    fn head_tail_total_lines_is_actual_count() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("hundred.txt");
        let mut f = File::create(&path).unwrap();
        for i in 1..=100 {
            writeln!(f, "line {}", i).unwrap();
        }
        let ss = SyntaxSet::load_defaults_newlines();
        let theme = load_theme(None);
        let (_lines, total) = load_head_tail_content(
            &path,
            &ss,
            &theme,
            &test_colors(),
            10,
            5,
            ViewMode::HeadAndTail,
        );
        // total_lines should be the actual file line count, not displayed lines
        assert_eq!(total, 100);
    }

    // === FR-10: safe epoch date tests ===

    #[test]
    fn epoch_days_overflow_does_not_panic() {
        // u64::MAX is far beyond i64 range — should return fallback
        let (y, m, d) = epoch_days_to_date(u64::MAX);
        assert_eq!((y, m, d), (9999, 12, 31));
    }

    #[test]
    fn epoch_days_normal_dates() {
        // Day 0 = 1970-01-01
        assert_eq!(epoch_days_to_date(0), (1970, 1, 1));
        // Day 365 = 1971-01-01 (1970 is not a leap year)
        assert_eq!(epoch_days_to_date(365), (1971, 1, 1));
    }
}
