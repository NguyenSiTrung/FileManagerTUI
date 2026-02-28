//! Application configuration: TOML file loading, CLI overrides, and defaults.
//!
//! Resolution order (first found wins, values merge/override):
//! 1. CLI flags (`--config`, `--no-preview`, `--theme`, etc.)
//! 2. `$FM_TUI_CONFIG` environment variable (path to config file)
//! 3. Project-local `.fm-tui.toml` in the current working directory
//! 4. Global `~/.config/fm-tui/config.toml`
//! 5. Built-in defaults

use std::path::{Path, PathBuf};

use serde::Deserialize;

// ── Section configs ──────────────────────────────────────────────────────────

/// General application settings.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct GeneralConfig {
    /// Starting directory (overridden by CLI positional arg).
    pub default_path: Option<String>,
    /// Show hidden files by default.
    pub show_hidden: Option<bool>,
    /// Confirm before delete operations.
    pub confirm_delete: Option<bool>,
    /// Enable mouse support.
    pub mouse: Option<bool>,
}

/// Preview panel settings.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct PreviewConfig {
    /// Maximum file size (bytes) for full preview; above this, use head+tail mode.
    pub max_full_preview_bytes: Option<u64>,
    /// Number of lines from the top of large files.
    pub head_lines: Option<usize>,
    /// Number of lines from the bottom of large files.
    pub tail_lines: Option<usize>,
    /// Default view mode for large files: "head_and_tail", "head_only", "tail_only".
    pub default_view_mode: Option<String>,
    /// Tab rendering width.
    pub tab_width: Option<usize>,
    /// Enable line wrapping.
    pub line_wrap: Option<bool>,
    /// Syntax highlighting theme (syntect theme name).
    pub syntax_theme: Option<String>,
    /// Whether the preview panel is enabled.
    pub enabled: Option<bool>,
}

/// Tree panel settings.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct TreeConfig {
    /// Sort order: "name", "size", "modified".
    pub sort_by: Option<String>,
    /// Directories always listed first.
    pub dirs_first: Option<bool>,
    /// Use nerd font icons (false = ASCII fallback).
    pub use_icons: Option<bool>,
}

/// Filesystem watcher settings.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct WatcherConfig {
    /// Enable filesystem watcher for auto-refresh.
    pub enabled: Option<bool>,
    /// Debounce interval in milliseconds.
    pub debounce_ms: Option<u64>,
}

/// Color settings for a single theme palette.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ThemeColorsConfig {
    pub tree_bg: Option<String>,
    pub tree_fg: Option<String>,
    pub tree_selected_bg: Option<String>,
    pub tree_selected_fg: Option<String>,
    pub tree_dir_fg: Option<String>,
    pub tree_file_fg: Option<String>,
    pub tree_hidden_fg: Option<String>,
    pub preview_bg: Option<String>,
    pub preview_fg: Option<String>,
    pub preview_line_nr_fg: Option<String>,
    pub status_bg: Option<String>,
    pub status_fg: Option<String>,
    pub border_fg: Option<String>,
    pub dialog_bg: Option<String>,
    pub dialog_border_fg: Option<String>,
}

/// Theme configuration section.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ThemeConfig {
    /// Color scheme: "dark", "light", "custom".
    pub scheme: Option<String>,
    /// Custom color overrides.
    pub custom: Option<ThemeColorsConfig>,
}

// ── Top-level config ─────────────────────────────────────────────────────────

/// Top-level application configuration.
///
/// All fields are optional so that partial configs from different sources
/// can be merged together (CLI overrides file, file overrides defaults).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct AppConfig {
    pub general: GeneralConfig,
    pub preview: PreviewConfig,
    pub tree: TreeConfig,
    pub watcher: WatcherConfig,
    pub theme: ThemeConfig,
}

// ── Default constants ────────────────────────────────────────────────────────

/// Default max file size for full preview (1 MiB).
pub const DEFAULT_MAX_FULL_PREVIEW_BYTES: u64 = 1_048_576;
/// Default head lines for large file preview.
pub const DEFAULT_HEAD_LINES: usize = 50;
/// Default tail lines for large file preview.
pub const DEFAULT_TAIL_LINES: usize = 20;
/// Default debounce interval in milliseconds.
pub const DEFAULT_DEBOUNCE_MS: u64 = 300;

// ── Config file locator ──────────────────────────────────────────────────────

/// Return the list of candidate config file paths in priority order.
///
/// Does NOT include the CLI `--config` path — that is handled separately.
fn candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // 1. $FM_TUI_CONFIG environment variable
    if let Ok(env_path) = std::env::var("FM_TUI_CONFIG") {
        paths.push(PathBuf::from(env_path));
    }

    // 2. Project-local `.fm-tui.toml` in CWD
    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join(".fm-tui.toml"));
    }

    // 3. Global `~/.config/fm-tui/config.toml`
    if let Some(config_dir) = dirs::config_dir() {
        paths.push(config_dir.join("fm-tui").join("config.toml"));
    }

    paths
}

/// Try to read and parse a TOML config file. Returns `None` if the file
/// doesn't exist or can't be parsed (with a warning printed to stderr).
fn load_file(path: &Path) -> Option<AppConfig> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return None,
    };
    match toml::from_str::<AppConfig>(&content) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            eprintln!(
                "Warning: failed to parse config file {}: {}",
                path.display(),
                e
            );
            None
        }
    }
}

// ── Merge logic ──────────────────────────────────────────────────────────────

/// Merge helper: `base` provides defaults; `over` overrides `base`.
/// For each `Option` field, if `over` has `Some`, use it; otherwise keep `base`.
#[allow(dead_code)]
impl AppConfig {
    /// Merge `other` on top of `self` — `other`'s `Some` values win.
    pub fn merge(self, other: &AppConfig) -> AppConfig {
        AppConfig {
            general: GeneralConfig {
                default_path: other
                    .general
                    .default_path
                    .clone()
                    .or(self.general.default_path),
                show_hidden: other.general.show_hidden.or(self.general.show_hidden),
                confirm_delete: other.general.confirm_delete.or(self.general.confirm_delete),
                mouse: other.general.mouse.or(self.general.mouse),
            },
            preview: PreviewConfig {
                max_full_preview_bytes: other
                    .preview
                    .max_full_preview_bytes
                    .or(self.preview.max_full_preview_bytes),
                head_lines: other.preview.head_lines.or(self.preview.head_lines),
                tail_lines: other.preview.tail_lines.or(self.preview.tail_lines),
                default_view_mode: other
                    .preview
                    .default_view_mode
                    .clone()
                    .or(self.preview.default_view_mode),
                tab_width: other.preview.tab_width.or(self.preview.tab_width),
                line_wrap: other.preview.line_wrap.or(self.preview.line_wrap),
                syntax_theme: other
                    .preview
                    .syntax_theme
                    .clone()
                    .or(self.preview.syntax_theme),
                enabled: other.preview.enabled.or(self.preview.enabled),
            },
            tree: TreeConfig {
                sort_by: other.tree.sort_by.clone().or(self.tree.sort_by),
                dirs_first: other.tree.dirs_first.or(self.tree.dirs_first),
                use_icons: other.tree.use_icons.or(self.tree.use_icons),
            },
            watcher: WatcherConfig {
                enabled: other.watcher.enabled.or(self.watcher.enabled),
                debounce_ms: other.watcher.debounce_ms.or(self.watcher.debounce_ms),
            },
            theme: ThemeConfig {
                scheme: other.theme.scheme.clone().or(self.theme.scheme),
                custom: match (&self.theme.custom, &other.theme.custom) {
                    (_, Some(o)) => Some(o.clone()),
                    (Some(s), None) => Some(s.clone()),
                    (None, None) => None,
                },
            },
        }
    }

    /// Load the final merged configuration.
    ///
    /// `cli_config_path` is an explicit config file path from `--config`.
    /// `cli_overrides` are partial overrides derived from CLI flags.
    pub fn load(cli_config_path: Option<&Path>, cli_overrides: Option<&AppConfig>) -> AppConfig {
        // Start with built-in defaults (all None — the struct Default).
        let mut config = AppConfig::default();

        // Load from candidate files (lowest priority first so higher overwrites).
        let paths = candidate_paths();
        // Walk in reverse so that highest-priority (env var) overwrites lower.
        for path in paths.iter().rev() {
            if let Some(file_cfg) = load_file(path) {
                config = config.merge(&file_cfg);
            }
        }

        // Explicit --config file has higher priority than candidates.
        if let Some(cli_path) = cli_config_path {
            if let Some(file_cfg) = load_file(cli_path) {
                config = config.merge(&file_cfg);
            }
        }

        // CLI flag overrides are highest priority.
        if let Some(overrides) = cli_overrides {
            config = config.merge(overrides);
        }

        config
    }

    // ── Convenience getters with built-in defaults ──────────────────────────

    /// Whether to show hidden files by default.
    pub fn show_hidden(&self) -> bool {
        self.general.show_hidden.unwrap_or(false)
    }

    /// Whether to confirm before delete.
    pub fn confirm_delete(&self) -> bool {
        self.general.confirm_delete.unwrap_or(true)
    }

    /// Whether mouse support is enabled.
    pub fn mouse_enabled(&self) -> bool {
        self.general.mouse.unwrap_or(true)
    }

    /// Whether the preview panel is enabled.
    pub fn preview_enabled(&self) -> bool {
        self.preview.enabled.unwrap_or(true)
    }

    /// Max file size in bytes for full preview.
    pub fn max_full_preview_bytes(&self) -> u64 {
        self.preview
            .max_full_preview_bytes
            .unwrap_or(DEFAULT_MAX_FULL_PREVIEW_BYTES)
    }

    /// Head lines for large file preview.
    pub fn head_lines(&self) -> usize {
        self.preview.head_lines.unwrap_or(DEFAULT_HEAD_LINES)
    }

    /// Tail lines for large file preview.
    pub fn tail_lines(&self) -> usize {
        self.preview.tail_lines.unwrap_or(DEFAULT_TAIL_LINES)
    }

    /// Syntax highlighting theme name.
    pub fn syntax_theme_name(&self) -> &str {
        self.preview
            .syntax_theme
            .as_deref()
            .unwrap_or("base16-ocean.dark")
    }

    /// Whether the watcher is enabled.
    pub fn watcher_enabled(&self) -> bool {
        self.watcher.enabled.unwrap_or(true)
    }

    /// Watcher debounce interval in milliseconds.
    pub fn debounce_ms(&self) -> u64 {
        self.watcher.debounce_ms.unwrap_or(DEFAULT_DEBOUNCE_MS)
    }

    /// Sort mode: "name", "size", or "modified".
    pub fn sort_by(&self) -> &str {
        self.tree.sort_by.as_deref().unwrap_or("name")
    }

    /// Whether directories are listed before files.
    pub fn dirs_first(&self) -> bool {
        self.tree.dirs_first.unwrap_or(true)
    }

    /// Whether to use nerd font icons.
    pub fn use_icons(&self) -> bool {
        self.tree.use_icons.unwrap_or(true)
    }

    /// Theme scheme: "dark", "light", or "custom".
    pub fn theme_scheme(&self) -> &str {
        self.theme.scheme.as_deref().unwrap_or("dark")
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_default_values() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.show_hidden(), false);
        assert_eq!(cfg.confirm_delete(), true);
        assert_eq!(cfg.mouse_enabled(), true);
        assert_eq!(cfg.preview_enabled(), true);
        assert_eq!(cfg.max_full_preview_bytes(), 1_048_576);
        assert_eq!(cfg.head_lines(), 50);
        assert_eq!(cfg.tail_lines(), 20);
        assert_eq!(cfg.syntax_theme_name(), "base16-ocean.dark");
        assert_eq!(cfg.watcher_enabled(), true);
        assert_eq!(cfg.debounce_ms(), 300);
        assert_eq!(cfg.sort_by(), "name");
        assert_eq!(cfg.dirs_first(), true);
        assert_eq!(cfg.use_icons(), true);
        assert_eq!(cfg.theme_scheme(), "dark");
    }

    #[test]
    fn test_toml_parsing_full() {
        let toml = r#"
[general]
show_hidden = true
confirm_delete = false
mouse = false

[preview]
max_full_preview_bytes = 2_000_000
head_lines = 100
tail_lines = 40
syntax_theme = "Solarized (dark)"
enabled = false

[tree]
sort_by = "size"
dirs_first = false
use_icons = false

[watcher]
enabled = false
debounce_ms = 500

[theme]
scheme = "light"
"#;
        let cfg: AppConfig = toml::from_str(toml).expect("parse failed");
        assert_eq!(cfg.show_hidden(), true);
        assert_eq!(cfg.confirm_delete(), false);
        assert_eq!(cfg.mouse_enabled(), false);
        assert_eq!(cfg.preview_enabled(), false);
        assert_eq!(cfg.max_full_preview_bytes(), 2_000_000);
        assert_eq!(cfg.head_lines(), 100);
        assert_eq!(cfg.tail_lines(), 40);
        assert_eq!(cfg.syntax_theme_name(), "Solarized (dark)");
        assert_eq!(cfg.watcher_enabled(), false);
        assert_eq!(cfg.debounce_ms(), 500);
        assert_eq!(cfg.sort_by(), "size");
        assert_eq!(cfg.dirs_first(), false);
        assert_eq!(cfg.use_icons(), false);
        assert_eq!(cfg.theme_scheme(), "light");
    }

    #[test]
    fn test_toml_parsing_partial() {
        let toml = r#"
[general]
show_hidden = true
"#;
        let cfg: AppConfig = toml::from_str(toml).expect("parse failed");
        assert_eq!(cfg.show_hidden(), true);
        // Everything else should be defaults
        assert_eq!(cfg.confirm_delete(), true);
        assert_eq!(cfg.max_full_preview_bytes(), 1_048_576);
        assert_eq!(cfg.sort_by(), "name");
    }

    #[test]
    fn test_toml_parsing_empty() {
        let cfg: AppConfig = toml::from_str("").expect("parse failed");
        assert_eq!(cfg.show_hidden(), false);
        assert_eq!(cfg.confirm_delete(), true);
    }

    #[test]
    fn test_merge_overrides() {
        let base = AppConfig {
            general: GeneralConfig {
                show_hidden: Some(false),
                confirm_delete: Some(true),
                ..Default::default()
            },
            preview: PreviewConfig {
                head_lines: Some(50),
                tail_lines: Some(20),
                ..Default::default()
            },
            ..Default::default()
        };

        let over = AppConfig {
            general: GeneralConfig {
                show_hidden: Some(true),
                // confirm_delete not set — should keep base
                ..Default::default()
            },
            preview: PreviewConfig {
                head_lines: Some(100),
                // tail_lines not set — should keep base
                ..Default::default()
            },
            ..Default::default()
        };

        let merged = base.merge(&over);
        assert_eq!(merged.show_hidden(), true); // overridden
        assert_eq!(merged.confirm_delete(), true); // from base
        assert_eq!(merged.head_lines(), 100); // overridden
        assert_eq!(merged.tail_lines(), 20); // from base
    }

    #[test]
    fn test_merge_none_does_not_clear_some() {
        let base = AppConfig {
            watcher: WatcherConfig {
                enabled: Some(false),
                debounce_ms: Some(500),
            },
            ..Default::default()
        };
        let over = AppConfig::default(); // all None

        let merged = base.merge(&over);
        assert_eq!(merged.watcher_enabled(), false); // base preserved
        assert_eq!(merged.debounce_ms(), 500); // base preserved
    }

    #[test]
    fn test_load_from_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg_path = dir.path().join("test-config.toml");
        let mut f = std::fs::File::create(&cfg_path).expect("create");
        writeln!(
            f,
            r#"
[general]
show_hidden = true

[preview]
head_lines = 75

[tree]
sort_by = "modified"
"#
        )
        .expect("write");

        let cfg = load_file(&cfg_path).expect("load");
        assert_eq!(cfg.show_hidden(), true);
        assert_eq!(cfg.head_lines(), 75);
        assert_eq!(cfg.sort_by(), "modified");
        // Unset fields fall through to defaults
        assert_eq!(cfg.tail_lines(), 20);
    }

    #[test]
    fn test_load_missing_file() {
        let result = load_file(Path::new("/nonexistent/config.toml"));
        assert!(result.is_none());
    }

    #[test]
    fn test_load_invalid_toml_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg_path = dir.path().join("bad.toml");
        std::fs::write(&cfg_path, "this is { not valid toml").expect("write");
        let result = load_file(&cfg_path);
        assert!(result.is_none());
    }

    #[test]
    fn test_load_with_cli_overrides() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg_path = dir.path().join("config.toml");
        std::fs::write(
            &cfg_path,
            r#"
[general]
show_hidden = true

[preview]
head_lines = 75
"#,
        )
        .expect("write");

        let cli_overrides = AppConfig {
            preview: PreviewConfig {
                head_lines: Some(200),
                ..Default::default()
            },
            ..Default::default()
        };

        let cfg = AppConfig::load(Some(&cfg_path), Some(&cli_overrides));
        // CLI override wins
        assert_eq!(cfg.head_lines(), 200);
        // File value preserved (not overridden by CLI)
        assert_eq!(cfg.show_hidden(), true);
    }

    #[test]
    fn test_load_with_no_files_returns_defaults() {
        // When no files found (env vars not set, no CWD config, no global config),
        // we should get all defaults.
        let cfg = AppConfig::load(None, None);
        assert_eq!(cfg.show_hidden(), false);
        assert_eq!(cfg.confirm_delete(), true);
        assert_eq!(cfg.head_lines(), 50);
        assert_eq!(cfg.tail_lines(), 20);
    }

    #[test]
    fn test_theme_custom_colors() {
        let toml = r##"
[theme]
scheme = "custom"

[theme.custom]
tree_bg = "#1a1b26"
tree_fg = "#c0caf5"
border_fg = "#565f89"
"##;
        let cfg: AppConfig = toml::from_str(toml).expect("parse");
        assert_eq!(cfg.theme_scheme(), "custom");
        let custom = cfg.theme.custom.as_ref().expect("custom present");
        assert_eq!(custom.tree_bg.as_deref(), Some("#1a1b26"));
        assert_eq!(custom.tree_fg.as_deref(), Some("#c0caf5"));
        assert_eq!(custom.border_fg.as_deref(), Some("#565f89"));
        // Unset custom colors are None
        assert!(custom.dialog_bg.is_none());
    }
}
