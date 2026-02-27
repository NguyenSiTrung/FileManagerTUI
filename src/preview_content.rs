use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

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
}
