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
    /// Maximum entries to load per page when expanding a directory (default: 1000).
    /// Clamped to 100..50000.
    pub max_entries_per_page: Option<u32>,
    /// Maximum entries for deep search filesystem walk (default: 10000).
    pub search_max_entries: Option<u32>,
    /// Maximum entries in a DirSnapshot (default: 500000).
    /// Limits memory usage for very large directories. Clamped to 10000..5000000.
    pub snapshot_max_entries: Option<u32>,
    /// Maximum file size in bytes that the editor will open (default: 10MB).
    pub max_editor_bytes: Option<u64>,
    /// Maximum line count that the editor will open (default: 100000).
    pub max_editor_lines: Option<u64>,
    /// Directory names excluded from content search and filename indexing.
    /// Empty or absent uses built-in defaults (`.git`, `target`, `.venv`, ...).
    pub search_exclude_dirs: Option<Vec<String>>,
    /// Maximum bytes read from a single file during content search.
    pub search_max_file_bytes: Option<u64>,
    /// Maximum number of files scanned for one content search.
    pub search_max_files: Option<u32>,
    /// Maximum number of hits retained for one content search.
    pub search_max_hits: Option<u32>,
    /// Maximum cumulative bytes scanned for one content search.
    pub search_max_bytes_scanned: Option<u64>,
    /// Maximum files scanned per resumable content-search batch.
    pub search_batch_files: Option<u32>,
    /// Maximum hits emitted per resumable content-search batch.
    pub search_batch_hits: Option<u32>,
    /// Maximum excerpt bytes retained per content-search hit.
    pub search_max_excerpt_bytes: Option<u32>,
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
    /// Timeout in milliseconds for directory scans (default: 2000ms).
    pub preview_timeout_ms: Option<u64>,
    /// Number of lines to stream for S3 head preview (default: 100).
    pub s3_head_lines: Option<usize>,
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
    /// Lines to scroll per mouse wheel tick (default: 3). Clamped to 1..=10.
    pub scroll_lines: Option<u16>,
}

/// Which watcher backend monitors the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WatcherMode {
    /// Native OS notifications (inotify/FSEvents/ReadDirectoryChangesW).
    #[default]
    Event,
    /// Periodic bounded re-scan; repairs lost/coalesced events on mounted
    /// storage that does not deliver reliable notifications.
    Polling,
}

impl WatcherMode {
    /// Parse the canonical `[watcher].mode` spelling; unknown values fall back
    /// to the event backend rather than silently disabling watching.
    pub fn from_str(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "polling" | "poll" => WatcherMode::Polling,
            _ => WatcherMode::Event,
        }
    }

    /// Canonical setting/config spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            WatcherMode::Event => "event",
            WatcherMode::Polling => "polling",
        }
    }
}

/// Filesystem watcher settings.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct WatcherConfig {
    /// Enable filesystem watcher (inotify backend). Defaults to false.
    pub enabled: Option<bool>,
    /// Debounce interval in milliseconds.
    pub debounce_ms: Option<u64>,
    /// Automatically apply filesystem changes to the tree (auto-refresh).
    /// When false (default), the watcher runs but changes only apply on
    /// manual refresh (F5 / Ctrl+R toggle). Set to true to restore the old
    /// interval-driven auto-refresh behaviour.
    pub auto_refresh: Option<bool>,
    /// Backend selection: `"event"` (default) or `"polling"`. Applies at
    /// backend start; changing it requires a restart.
    pub mode: Option<String>,
    /// Polling interval in milliseconds (default 2000, clamped to 250..=60000).
    /// Only consulted by the polling backend.
    pub poll_interval_ms: Option<u64>,
}

/// Embedded terminal settings.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct TerminalConfig {
    /// Enable the embedded terminal feature (default: true).
    pub enabled: Option<bool>,
    /// Default shell to use (default: $SHELL or /bin/sh).
    pub default_shell: Option<String>,
    /// Number of scrollback lines (default: 1000).
    pub scrollback_lines: Option<usize>,
}

/// Private workspace-session persistence settings.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct SessionConfig {
    /// Persist and restore workspace sessions (default true).
    pub enabled: Option<bool>,
    /// Override the private session state directory. Empty/absent means the
    /// platform state directory (`fm-tui/sessions` under it).
    pub state_dir: Option<String>,
}

/// Bounded private recovery-snapshot persistence settings.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct RecoveryConfig {
    /// Persist bounded recovery snapshots for dirty documents (default true).
    pub enabled: Option<bool>,
    /// Override the private recovery state directory. Empty/absent means the
    /// platform state directory (`fm-tui/recovery` under it).
    pub state_dir: Option<String>,
    /// Maximum number of retained snapshots per workspace (clamped 1..=1024).
    pub max_records: Option<usize>,
    /// Maximum snapshot age in seconds (clamped to at least one hour).
    pub max_age_secs: Option<u64>,
    /// Minimum interval between throttled snapshot writes in milliseconds.
    pub min_interval_ms: Option<u64>,
}

/// Optional read-only Git indicator settings.
///
/// The backend (`src/git.rs`) is strictly read-only; this only controls whether
/// the optional `git` executable is queried for branch and file decorations.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct GitConfig {
    /// Show optional read-only Git indicators (default true). Disabling removes
    /// every decoration and stops further `git` queries.
    pub enabled: Option<bool>,
}

/// Private per-user session state directory used when no override is set.
/// Never the shared fixed paths used for transient clipboard data.
pub fn default_session_state_dir() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_dir)
        .map(|base| base.join("fm-tui").join("sessions"))
}

/// Private per-user recovery state directory used when no override is set.
/// Distinct from the session directory so clearing one never touches the other.
pub fn default_recovery_state_dir() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_dir)
        .map(|base| base.join("fm-tui").join("recovery"))
}

/// Saved absolute pane preferences; temporary maximize/compact geometry is not persisted.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct LayoutConfig {
    pub explorer_width: Option<u16>,
    pub explorer_visible: Option<bool>,
    pub terminal_height: Option<u16>,
    pub terminal_visible: Option<bool>,
}

impl LayoutConfig {
    pub fn state(&self) -> crate::workspace::layout::LayoutState {
        let mut state = crate::workspace::layout::LayoutState::default();
        state.set_explorer_width(self.explorer_width.unwrap_or(24));
        state.set_terminal_height(self.terminal_height.unwrap_or(7));
        if !self.explorer_visible.unwrap_or(true) {
            state.toggle_explorer();
        }
        if !self.terminal_visible.unwrap_or(false) {
            state.toggle_terminal();
        }
        state
    }
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
    pub scrollbar_track_fg: Option<String>,
    pub scrollbar_thumb_fg: Option<String>,
    pub s3_dir_fg: Option<String>,
    pub s3_file_fg: Option<String>,
    pub s3_border_fg: Option<String>,
    pub git_modified_fg: Option<String>,
    pub git_staged_fg: Option<String>,
    pub git_untracked_fg: Option<String>,
    pub git_conflicted_fg: Option<String>,
    pub git_branch_fg: Option<String>,
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
    pub terminal: TerminalConfig,
    pub layout: LayoutConfig,
    pub session: SessionConfig,
    pub recovery: RecoveryConfig,
    pub git: GitConfig,
    /// Installed-server LSP configuration (FR-10). Optional by design.
    pub lsp: crate::lsp::config::LspConfig,
    /// The `[lsp]` section as loaded from trusted layers only (global file,
    /// `$FM_TUI_CONFIG`, `--config`, CLI overrides) — the source of durable
    /// `[[lsp.trust]]` grants. Never deserialized; populated by
    /// `load_checked`.
    #[serde(skip)]
    pub lsp_global: crate::lsp::config::LspConfig,
    /// The `[lsp]` section from the project-local `.fm-tui.toml` alone —
    /// configuration, but untrusted for execution until approved.
    #[serde(skip)]
    pub lsp_local: crate::lsp::config::LspConfig,
    pub theme: ThemeConfig,
    pub keymap: crate::keymap::KeymapConfig,
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
/// Default filesystem watcher poll interval in milliseconds.
pub const DEFAULT_POLL_INTERVAL_MS: u64 = 2_000;
/// Minimum accepted watcher poll interval in milliseconds.
pub const MIN_POLL_INTERVAL_MS: u64 = 250;
/// Maximum accepted watcher poll interval in milliseconds.
pub const MAX_POLL_INTERVAL_MS: u64 = 60_000;
/// Default max entries per page for directory pagination.
pub const DEFAULT_MAX_ENTRIES_PER_PAGE: u32 = 1_000;
/// Minimum allowed value for max_entries_per_page.
pub const MIN_ENTRIES_PER_PAGE: u32 = 100;
/// Maximum allowed value for max_entries_per_page.
pub const MAX_ENTRIES_PER_PAGE: u32 = 50_000;
/// Default max entries for deep search walk.
pub const DEFAULT_SEARCH_MAX_ENTRIES: u32 = 10_000;
/// Default max entries for DirSnapshot.
pub const DEFAULT_SNAPSHOT_MAX_ENTRIES: u32 = 500_000;
/// Minimum allowed value for snapshot_max_entries.
pub const MIN_SNAPSHOT_MAX_ENTRIES: u32 = 10_000;
/// Maximum allowed value for snapshot_max_entries.
pub const MAX_SNAPSHOT_MAX_ENTRIES: u32 = 5_000_000;
/// Default max file size for editor (10 MiB).
pub const DEFAULT_MAX_EDITOR_BYTES: u64 = 10 * 1024 * 1024;
/// Hard ceiling for the editor byte budget (256 MiB).
///
/// `max_editor_bytes` is unclamped user config; without this ceiling a very
/// large setting produces an effectively unbounded capture/load budget. The
/// ceiling also fits `usize` on every supported platform, so the `usize`
/// conversion below is lossless rather than falling back to `usize::MAX`.
pub const MAX_EDITOR_BYTES: u64 = 256 * 1024 * 1024;
const _: () = assert!((MAX_EDITOR_BYTES as u128) <= (usize::MAX as u128));
/// Default max line count for editor.
pub const DEFAULT_MAX_EDITOR_LINES: usize = 100_000;
/// Default preview timeout in milliseconds.
pub const DEFAULT_PREVIEW_TIMEOUT_MS: u64 = 2000;
/// Default scroll lines per mouse wheel tick.
pub const DEFAULT_SCROLL_LINES: u16 = 3;
/// Default number of lines to stream for S3 head preview.
pub const DEFAULT_S3_HEAD_LINES: usize = 100;
/// Default number of recovery snapshots retained per workspace.
pub const DEFAULT_RECOVERY_MAX_RECORDS: usize = 32;
/// Minimum accepted recovery snapshot retention count.
pub const MIN_RECOVERY_MAX_RECORDS: usize = 1;
/// Maximum accepted recovery snapshot retention count.
pub const MAX_RECOVERY_MAX_RECORDS: usize = 1024;
/// Default maximum recovery snapshot age (7 days).
pub const DEFAULT_RECOVERY_MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;
/// Minimum accepted recovery snapshot age (1 hour).
pub const MIN_RECOVERY_MAX_AGE_SECS: u64 = 60 * 60;
/// Maximum accepted recovery snapshot age (100 years). The upper clamp is not
/// cosmetic: the age cutoff is computed with signed seconds and a value at or
/// above `i64::MAX` would wrap negative and delete fresh snapshots as if they
/// were ancient. It is also far below `i64::MAX` for every arithmetic step.
pub const MAX_RECOVERY_MAX_AGE_SECS: u64 = 100 * 365 * 24 * 60 * 60;
/// Default minimum interval between throttled snapshot writes.
pub const DEFAULT_RECOVERY_MIN_INTERVAL_MS: u64 = 2_000;

/// A project-local config file may carry `[[lsp.trust]]` entries, but
/// honoring them would let a checked-out repository grant its own argv
/// execution. Strip them (with a notice) — grants belong to trusted layers.
fn strip_untrusted_trust(parsed: &mut AppConfig, path: &Path, is_project_layer: bool) {
    if is_project_layer && !parsed.lsp.trust.is_empty() {
        eprintln!(
            "fm: ignoring [[lsp.trust]] in project-local config {} — \
             execution grants belong in the global config",
            path.display()
        );
        parsed.lsp.trust.clear();
    }
}

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
#[cfg(test)]
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
    pub fn load_checked(
        cli_config_path: Option<&Path>,
        cli_overrides: Option<&AppConfig>,
    ) -> Result<Self, String> {
        use std::io::Read;
        let mut config = Self::default();
        let mut paths: Vec<_> = candidate_paths()
            .into_iter()
            .rev()
            .map(|p| (p, false))
            .collect();
        if let Some(path) = cli_config_path {
            paths.push((path.to_owned(), true));
        }
        // The one untrusted layer: the auto-discovered `.fm-tui.toml` in the
        // current directory. Everything else (global, env, --config, CLI
        // overrides) is the user's own trusted configuration.
        let project_local = std::env::current_dir()
            .ok()
            .map(|cwd| cwd.join(".fm-tui.toml"));
        for (path, explicit) in paths {
            let file = match std::fs::File::open(&path) {
                Ok(file) => file,
                Err(error) if !explicit && error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!("Cannot read config {}: {error}", path.display()))
                }
            };
            let mut content = String::new();
            file.take(1024 * 1024 + 1)
                .read_to_string(&mut content)
                .map_err(|error| format!("Cannot read config {}: {error}", path.display()))?;
            if content.len() > 1024 * 1024 {
                return Err(format!("Config {} exceeds 1 MiB", path.display()));
            }
            let mut parsed: Self = toml::from_str(&content)
                .map_err(|error| format!("Invalid config {}: {error}", path.display()))?;
            let is_project_layer = project_local.as_deref() == Some(path.as_path());
            strip_untrusted_trust(&mut parsed, &path, is_project_layer);
            // Provenance must survive the merge for the trust gate: keep the
            // per-layer lsp sections alongside the merged view.
            if is_project_layer {
                config.lsp_local = config.lsp_local.merge(&parsed.lsp);
            } else {
                config.lsp_global = config.lsp_global.merge(&parsed.lsp);
            }
            config = config.merge(&parsed);
        }
        if let Some(overrides) = cli_overrides {
            config.lsp_global = config.lsp_global.merge(&overrides.lsp);
            config = config.merge(overrides);
        }
        crate::keymap::Keymap::compile(&config.keymap)
            .map_err(|error| format!("Invalid keymap configuration: {error}"))?;
        Ok(config)
    }
    /// Merge `other` on top of `self` — `other`'s `Some` values win.
    pub fn merge(self, other: &AppConfig) -> AppConfig {
        AppConfig {
            keymap: self.keymap.merge(&other.keymap),
            general: GeneralConfig {
                default_path: other
                    .general
                    .default_path
                    .clone()
                    .or(self.general.default_path),
                show_hidden: other.general.show_hidden.or(self.general.show_hidden),
                confirm_delete: other.general.confirm_delete.or(self.general.confirm_delete),
                mouse: other.general.mouse.or(self.general.mouse),
                max_entries_per_page: other
                    .general
                    .max_entries_per_page
                    .or(self.general.max_entries_per_page),
                search_max_entries: other
                    .general
                    .search_max_entries
                    .or(self.general.search_max_entries),
                snapshot_max_entries: other
                    .general
                    .snapshot_max_entries
                    .or(self.general.snapshot_max_entries),
                max_editor_bytes: other
                    .general
                    .max_editor_bytes
                    .or(self.general.max_editor_bytes),
                max_editor_lines: other
                    .general
                    .max_editor_lines
                    .or(self.general.max_editor_lines),
                search_exclude_dirs: other
                    .general
                    .search_exclude_dirs
                    .clone()
                    .or_else(|| self.general.search_exclude_dirs.clone()),
                search_max_file_bytes: other
                    .general
                    .search_max_file_bytes
                    .or(self.general.search_max_file_bytes),
                search_max_files: other
                    .general
                    .search_max_files
                    .or(self.general.search_max_files),
                search_max_hits: other
                    .general
                    .search_max_hits
                    .or(self.general.search_max_hits),
                search_max_bytes_scanned: other
                    .general
                    .search_max_bytes_scanned
                    .or(self.general.search_max_bytes_scanned),
                search_batch_files: other
                    .general
                    .search_batch_files
                    .or(self.general.search_batch_files),
                search_batch_hits: other
                    .general
                    .search_batch_hits
                    .or(self.general.search_batch_hits),
                search_max_excerpt_bytes: other
                    .general
                    .search_max_excerpt_bytes
                    .or(self.general.search_max_excerpt_bytes),
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
                preview_timeout_ms: other
                    .preview
                    .preview_timeout_ms
                    .or(self.preview.preview_timeout_ms),
                s3_head_lines: other.preview.s3_head_lines.or(self.preview.s3_head_lines),
            },
            tree: TreeConfig {
                sort_by: other.tree.sort_by.clone().or(self.tree.sort_by),
                dirs_first: other.tree.dirs_first.or(self.tree.dirs_first),
                use_icons: other.tree.use_icons.or(self.tree.use_icons),
                scroll_lines: other.tree.scroll_lines.or(self.tree.scroll_lines),
            },
            watcher: WatcherConfig {
                enabled: other.watcher.enabled.or(self.watcher.enabled),
                debounce_ms: other.watcher.debounce_ms.or(self.watcher.debounce_ms),
                auto_refresh: other.watcher.auto_refresh.or(self.watcher.auto_refresh),
                mode: other
                    .watcher
                    .mode
                    .clone()
                    .or_else(|| self.watcher.mode.clone()),
                poll_interval_ms: other
                    .watcher
                    .poll_interval_ms
                    .or(self.watcher.poll_interval_ms),
            },
            terminal: TerminalConfig {
                enabled: other.terminal.enabled.or(self.terminal.enabled),
                default_shell: other
                    .terminal
                    .default_shell
                    .clone()
                    .or(self.terminal.default_shell),
                scrollback_lines: other
                    .terminal
                    .scrollback_lines
                    .or(self.terminal.scrollback_lines),
            },
            layout: LayoutConfig {
                explorer_width: other.layout.explorer_width.or(self.layout.explorer_width),
                explorer_visible: other
                    .layout
                    .explorer_visible
                    .or(self.layout.explorer_visible),
                terminal_height: other.layout.terminal_height.or(self.layout.terminal_height),
                terminal_visible: other
                    .layout
                    .terminal_visible
                    .or(self.layout.terminal_visible),
            },
            session: SessionConfig {
                enabled: other.session.enabled.or(self.session.enabled),
                state_dir: other
                    .session
                    .state_dir
                    .clone()
                    .or_else(|| self.session.state_dir.clone()),
            },
            recovery: RecoveryConfig {
                enabled: other.recovery.enabled.or(self.recovery.enabled),
                state_dir: other
                    .recovery
                    .state_dir
                    .clone()
                    .or_else(|| self.recovery.state_dir.clone()),
                max_records: other.recovery.max_records.or(self.recovery.max_records),
                max_age_secs: other.recovery.max_age_secs.or(self.recovery.max_age_secs),
                min_interval_ms: other
                    .recovery
                    .min_interval_ms
                    .or(self.recovery.min_interval_ms),
            },
            git: GitConfig {
                enabled: other.git.enabled.or(self.git.enabled),
            },
            lsp: self.lsp.merge(&other.lsp),
            // Per-layer lsp provenance merges through its own channels in
            // `load_checked`; other layers arrive empty and change nothing.
            lsp_global: self.lsp_global.merge(&other.lsp_global),
            lsp_local: self.lsp_local.merge(&other.lsp_local),
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
    #[cfg(test)]
    pub fn load(cli_config_path: Option<&Path>, cli_overrides: Option<&AppConfig>) -> AppConfig {
        Self::load_checked(cli_config_path, cli_overrides).expect("invalid test configuration")
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
    ///
    /// If nonblank in `[preview].syntax_theme`, that value always wins.
    /// Otherwise, the default depends on the active theme scheme.
    pub fn syntax_theme_name(&self, theme_scheme: &str) -> &str {
        self.preview
            .syntax_theme
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| {
                if theme_scheme.eq_ignore_ascii_case("light") {
                    "InspiredGitHub"
                } else {
                    "base16-ocean.dark"
                }
            })
    }

    /// Number of lines for S3 head preview.
    pub fn s3_head_lines(&self) -> usize {
        self.preview.s3_head_lines.unwrap_or(DEFAULT_S3_HEAD_LINES)
    }

    /// Preview timeout in milliseconds for directory scans.
    pub fn preview_timeout_ms(&self) -> u64 {
        self.preview
            .preview_timeout_ms
            .unwrap_or(DEFAULT_PREVIEW_TIMEOUT_MS)
    }

    /// Canonical preview policy. Legacy `head_tail` and `head_and_tail` are aliases.
    /// Full remains bounded by max_full_preview_bytes, falling back to head/tail.
    pub fn preview_view_mode(&self) -> &str {
        match self.preview.default_view_mode.as_deref().unwrap_or("full") {
            "head_tail" | "head_and_tail" => "head_and_tail",
            "head_only" => "head_only",
            "tail_only" => "tail_only",
            _ => "full",
        }
    }

    /// Whether the watcher is enabled.
    pub fn watcher_enabled(&self) -> bool {
        self.watcher.enabled.unwrap_or(false)
    }

    /// Whether the watcher should automatically apply FS changes (auto-refresh).
    /// Defaults to false — manual refresh only.
    pub fn watcher_auto_refresh(&self) -> bool {
        self.watcher.auto_refresh.unwrap_or(false)
    }

    /// Watcher debounce interval in milliseconds.
    pub fn debounce_ms(&self) -> u64 {
        self.watcher.debounce_ms.unwrap_or(DEFAULT_DEBOUNCE_MS)
    }

    /// Selected watcher backend (event or polling). Unknown spellings fall
    /// back to the event backend.
    pub fn watcher_mode(&self) -> WatcherMode {
        self.watcher
            .mode
            .as_deref()
            .map(WatcherMode::from_str)
            .unwrap_or_default()
    }

    /// Polling backend interval in milliseconds, clamped to a sane range so a
    /// misconfigured value cannot create a busy loop or an unusably slow poll.
    pub fn poll_interval_ms(&self) -> u64 {
        self.watcher
            .poll_interval_ms
            .unwrap_or(DEFAULT_POLL_INTERVAL_MS)
            .clamp(MIN_POLL_INTERVAL_MS, MAX_POLL_INTERVAL_MS)
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

    /// Lines to scroll per mouse wheel tick. Clamped to 1..=10.
    pub fn scroll_lines(&self) -> usize {
        self.tree
            .scroll_lines
            .unwrap_or(DEFAULT_SCROLL_LINES)
            .clamp(1, 10) as usize
    }

    /// Theme scheme: "dark", "light", or "custom".
    pub fn theme_scheme(&self) -> &str {
        self.theme.scheme.as_deref().unwrap_or("dark")
    }

    /// Whether the embedded terminal is enabled.
    pub fn terminal_enabled(&self) -> bool {
        self.terminal.enabled.unwrap_or(true)
    }

    /// Default shell for the embedded terminal.
    pub fn terminal_shell(&self) -> String {
        self.terminal
            .default_shell
            .clone()
            .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()))
    }

    /// Scrollback lines for the embedded terminal.
    pub fn terminal_scrollback(&self) -> usize {
        self.terminal
            .scrollback_lines
            .unwrap_or(1000)
            .min(crate::terminal::emulator::MAX_SCROLLBACK_LINES)
    }

    /// Whether workspace sessions are persisted and restored (default true).
    pub fn session_enabled(&self) -> bool {
        self.session.enabled.unwrap_or(true)
    }

    /// Resolved private session state directory, or `None` when the platform
    /// provides no state/data directory and no explicit override is configured.
    /// A `None` here means persistence is off; `main` reports it as a visible
    /// status line ("Session persistence unavailable"), so it degrades visibly.
    pub fn session_state_dir(&self) -> Option<PathBuf> {
        match self.session.state_dir.as_deref() {
            Some(dir) if !dir.trim().is_empty() => Some(PathBuf::from(dir)),
            _ => default_session_state_dir(),
        }
    }

    /// Whether bounded recovery snapshots are persisted (default true).
    pub fn recovery_enabled(&self) -> bool {
        self.recovery.enabled.unwrap_or(true)
    }

    /// Whether optional read-only Git indicators are enabled (default true).
    ///
    /// Disabling removes every decoration and stops further `git` queries;
    /// missing/unavailable Git still degrades to no indicators rather than an error.
    pub fn git_enabled(&self) -> bool {
        self.git.enabled.unwrap_or(true)
    }

    /// Resolved private recovery state directory, or `None` when the platform
    /// provides no state/data directory and no explicit override is configured.
    /// A `None` here degrades visibly (main reports it), never silently.
    pub fn recovery_state_dir(&self) -> Option<PathBuf> {
        match self.recovery.state_dir.as_deref() {
            Some(dir) if !dir.trim().is_empty() => Some(PathBuf::from(dir)),
            _ => default_recovery_state_dir(),
        }
    }

    /// Maximum retained recovery snapshots per workspace, clamped so a
    /// misconfiguration cannot disable retention or exhaust the disk.
    pub fn recovery_max_records(&self) -> usize {
        self.recovery
            .max_records
            .unwrap_or(DEFAULT_RECOVERY_MAX_RECORDS)
            .clamp(MIN_RECOVERY_MAX_RECORDS, MAX_RECOVERY_MAX_RECORDS)
    }

    /// Maximum recovery snapshot age, clamped to a sane range in seconds. Both
    /// bounds matter: the lower one keeps retention meaningful and the upper one
    /// (100 years) keeps the signed age cutoff from wrapping negative, which
    /// would prune every fresh snapshot as if it were ancient.
    pub fn recovery_max_age_secs(&self) -> u64 {
        self.recovery
            .max_age_secs
            .unwrap_or(DEFAULT_RECOVERY_MAX_AGE_SECS)
            .clamp(MIN_RECOVERY_MAX_AGE_SECS, MAX_RECOVERY_MAX_AGE_SECS)
    }

    /// Minimum interval between throttled recovery snapshot writes.
    pub fn recovery_min_interval_ms(&self) -> u64 {
        self.recovery
            .min_interval_ms
            .unwrap_or(DEFAULT_RECOVERY_MIN_INTERVAL_MS)
    }

    /// Max entries to load per page when expanding large directories.
    /// Clamped to [MIN_ENTRIES_PER_PAGE, MAX_ENTRIES_PER_PAGE].
    pub fn max_entries_per_page(&self) -> usize {
        let raw = self
            .general
            .max_entries_per_page
            .unwrap_or(DEFAULT_MAX_ENTRIES_PER_PAGE);
        raw.clamp(MIN_ENTRIES_PER_PAGE, MAX_ENTRIES_PER_PAGE) as usize
    }

    /// Max entries for deep search filesystem walk.
    pub fn search_max_entries(&self) -> usize {
        self.general
            .search_max_entries
            .unwrap_or(DEFAULT_SEARCH_MAX_ENTRIES) as usize
    }

    /// Directory names excluded from search/indexing. Absent or empty uses the
    /// built-in defaults so exclusions can never be accidentally disabled.
    pub fn search_exclude_dirs(&self) -> Vec<String> {
        match &self.general.search_exclude_dirs {
            Some(names) if !names.is_empty() => names.clone(),
            _ => crate::search::default_excludes(),
        }
    }

    /// Resolved, always-valid content-search bounds.
    pub fn search_limits(&self) -> crate::search::SearchLimits {
        let defaults = crate::search::SearchLimits::default();
        crate::search::SearchLimits {
            max_file_bytes: self
                .general
                .search_max_file_bytes
                .unwrap_or(defaults.max_file_bytes)
                .max(1),
            max_files: (self
                .general
                .search_max_files
                .unwrap_or(defaults.max_files as u32)
                .max(1)) as usize,
            max_hits: (self
                .general
                .search_max_hits
                .unwrap_or(defaults.max_hits as u32)
                .max(1)) as usize,
            max_bytes_scanned: self
                .general
                .search_max_bytes_scanned
                .unwrap_or(defaults.max_bytes_scanned)
                .max(1),
            batch_files: (self
                .general
                .search_batch_files
                .unwrap_or(defaults.batch_files as u32)
                .max(1)) as usize,
            batch_hits: (self
                .general
                .search_batch_hits
                .unwrap_or(defaults.batch_hits as u32)
                .max(1)) as usize,
            max_excerpt_bytes: (self
                .general
                .search_max_excerpt_bytes
                .unwrap_or(defaults.max_excerpt_bytes as u32)
                .max(1)) as usize,
            max_depth: defaults.max_depth,
            max_pending: defaults.max_pending,
        }
    }

    /// Max file size in bytes the editor will open, clamped to
    /// `MAX_EDITOR_BYTES` so no configured value yields an effectively
    /// unbounded budget.
    pub fn max_editor_bytes(&self) -> u64 {
        self.general
            .max_editor_bytes
            .unwrap_or(DEFAULT_MAX_EDITOR_BYTES)
            .min(MAX_EDITOR_BYTES)
    }

    /// Editor byte budget as a `usize`.
    ///
    /// `max_editor_bytes()` is clamped to `MAX_EDITOR_BYTES`, which fits
    /// `usize` on every supported platform, so this conversion is lossless and
    /// can never degenerate to the `usize::MAX` sentinel.
    pub fn max_editor_bytes_usize(&self) -> usize {
        self.max_editor_bytes() as usize
    }

    /// Max line count the editor will open.
    pub fn max_editor_lines(&self) -> usize {
        self.general
            .max_editor_lines
            .unwrap_or(DEFAULT_MAX_EDITOR_LINES as u64) as usize
    }

    /// Max entries for DirSnapshot.
    /// Clamped to [MIN_SNAPSHOT_MAX_ENTRIES, MAX_SNAPSHOT_MAX_ENTRIES].
    pub fn snapshot_max_entries(&self) -> usize {
        let raw = self
            .general
            .snapshot_max_entries
            .unwrap_or(DEFAULT_SNAPSHOT_MAX_ENTRIES);
        raw.clamp(MIN_SNAPSHOT_MAX_ENTRIES, MAX_SNAPSHOT_MAX_ENTRIES) as usize
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn task3_blank_syntax_theme_uses_current_theme_default() {
        let mut config = AppConfig::default();
        config.preview.syntax_theme = Some(" ".into());
        assert_eq!(config.syntax_theme_name("light"), "InspiredGitHub");
        assert_eq!(config.syntax_theme_name("dark"), "base16-ocean.dark");
    }

    #[test]
    fn task3_preview_aliases_and_bounded_terminal_history() {
        let mut config = AppConfig::default();
        assert_eq!(config.preview_view_mode(), "full");
        for (input, expected) in [
            ("full", "full"),
            ("head_tail", "head_and_tail"),
            ("head_and_tail", "head_and_tail"),
            ("head_only", "head_only"),
            ("tail_only", "tail_only"),
            ("legacy_unknown", "full"),
        ] {
            config.preview.default_view_mode = Some(input.into());
            assert_eq!(config.preview_view_mode(), expected);
        }
        config.terminal.scrollback_lines = Some(usize::MAX);
        assert_eq!(config.terminal_scrollback(), 100_000);
        config.terminal.scrollback_lines = Some(0);
        assert_eq!(config.terminal_scrollback(), 0);
    }

    #[test]
    fn extreme_max_editor_bytes_is_clamped_below_usize_max() {
        let mut config = AppConfig::default();
        // An extreme configured value must not become an unbounded budget.
        config.general.max_editor_bytes = Some(u64::MAX);
        assert_eq!(config.max_editor_bytes(), MAX_EDITOR_BYTES);
        let bounded = config.max_editor_bytes_usize();
        assert!(bounded <= MAX_EDITOR_BYTES as usize);
        assert_ne!(bounded, usize::MAX);
        assert_eq!(bounded, MAX_EDITOR_BYTES as usize);

        // An ordinary value passes through unchanged.
        config.general.max_editor_bytes = Some(4096);
        assert_eq!(config.max_editor_bytes(), 4096);
        assert_eq!(config.max_editor_bytes_usize(), 4096);

        // The default is unaffected by the clamp.
        config.general.max_editor_bytes = None;
        assert_eq!(config.max_editor_bytes(), DEFAULT_MAX_EDITOR_BYTES);
    }

    #[test]
    fn keymap_config_profile_merge_and_context_override() {
        let base: AppConfig = toml::from_str(
            r#"
[keymap]
profile = "web"
timeout_ms = 900
[[keymap.bindings]]
command = "document.save"
context = "editor"
keys = ["F9"]
[[keymap.bindings]]
command = "document.close"
context = "editor"
keys = []
"#,
        )
        .unwrap();
        let over: AppConfig = toml::from_str(
            r#"
[keymap]
profile = "standard"
[[keymap.bindings]]
command = "document.save"
context = "editor"
keys = ["F10"]
"#,
        )
        .unwrap();
        let merged = base.merge(&over);
        assert_eq!(
            merged.keymap.profile,
            Some(crate::keymap::KeymapProfile::Standard)
        );
        assert_eq!(merged.keymap.timeout_ms, Some(900));
        let bindings = merged.keymap.bindings.unwrap();
        assert_eq!(bindings.len(), 2);
        assert_eq!(
            bindings
                .iter()
                .find(|b| b.command == "document.save")
                .unwrap()
                .keys,
            ["F10"]
        );
        assert!(bindings
            .iter()
            .find(|b| b.command == "document.close")
            .unwrap()
            .keys
            .is_empty());
        assert!(toml::from_str::<AppConfig>("[keymap]\nprofile = 'unknown'").is_err());
    }

    #[test]
    fn keymap_checked_config_errors_do_not_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for content in [
            "[keymap]\nprofile = 'unknown'",
            "[keymap]\nprofile = 'web'\n[[keymap.bindings]]\ncommand = 'unknown'\ncontext = 'editor'\nkeys = ['F9']",
            "not { valid toml",
        ] {
            std::fs::write(&path, content).unwrap();
            assert!(AppConfig::load_checked(Some(&path), None).is_err());
        }
        assert!(AppConfig::load_checked(Some(&dir.path().join("missing")), None).is_err());
    }

    #[test]
    fn test_default_values() {
        let cfg = AppConfig::default();
        assert!(!cfg.show_hidden());
        assert!(cfg.confirm_delete());
        assert!(cfg.mouse_enabled());
        assert!(cfg.preview_enabled());
        assert_eq!(cfg.max_full_preview_bytes(), 1_048_576);
        assert_eq!(cfg.head_lines(), 50);
        assert_eq!(cfg.tail_lines(), 20);
        assert_eq!(
            cfg.syntax_theme_name(cfg.theme_scheme()),
            "base16-ocean.dark"
        );
        assert!(!cfg.watcher_enabled());
        assert!(!cfg.watcher_auto_refresh());
        assert_eq!(cfg.debounce_ms(), 300);
        assert_eq!(cfg.sort_by(), "name");
        assert!(cfg.dirs_first());
        assert!(cfg.use_icons());
        assert_eq!(cfg.theme_scheme(), "dark");
        assert_eq!(cfg.max_entries_per_page(), 1000);
        assert_eq!(cfg.search_max_entries(), 10000);
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
auto_refresh = true

[theme]
scheme = "light"
"#;
        let cfg: AppConfig = toml::from_str(toml).expect("parse failed");
        assert!(cfg.show_hidden());
        assert!(!cfg.confirm_delete());
        assert!(!cfg.mouse_enabled());
        assert!(!cfg.preview_enabled());
        assert_eq!(cfg.max_full_preview_bytes(), 2_000_000);
        assert_eq!(cfg.head_lines(), 100);
        assert_eq!(cfg.tail_lines(), 40);
        assert_eq!(
            cfg.syntax_theme_name(cfg.theme_scheme()),
            "Solarized (dark)"
        );
        assert!(!cfg.watcher_enabled());
        assert_eq!(cfg.debounce_ms(), 500);
        assert!(cfg.watcher_auto_refresh());
        assert_eq!(cfg.sort_by(), "size");
        assert!(!cfg.dirs_first());
        assert!(!cfg.use_icons());
        assert_eq!(cfg.theme_scheme(), "light");
    }

    #[test]
    fn test_toml_parsing_partial() {
        let toml = r#"
[general]
show_hidden = true
"#;
        let cfg: AppConfig = toml::from_str(toml).expect("parse failed");
        assert!(cfg.show_hidden());
        // Everything else should be defaults
        assert!(cfg.confirm_delete());
        assert_eq!(cfg.max_full_preview_bytes(), 1_048_576);
        assert_eq!(cfg.sort_by(), "name");
    }

    #[test]
    fn test_toml_parsing_empty() {
        let cfg: AppConfig = toml::from_str("").expect("parse failed");
        assert!(!cfg.show_hidden());
        assert!(cfg.confirm_delete());
    }

    #[test]
    fn test_syntax_theme_defaults_by_scheme() {
        let dark_cfg = AppConfig {
            theme: ThemeConfig {
                scheme: Some("dark".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            dark_cfg.syntax_theme_name(dark_cfg.theme_scheme()),
            "base16-ocean.dark"
        );

        let light_cfg = AppConfig {
            theme: ThemeConfig {
                scheme: Some("light".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            light_cfg.syntax_theme_name(light_cfg.theme_scheme()),
            "InspiredGitHub"
        );

        let unset_cfg = AppConfig::default();
        assert_eq!(
            unset_cfg.syntax_theme_name(unset_cfg.theme_scheme()),
            "base16-ocean.dark"
        );
    }

    #[test]
    fn test_syntax_theme_user_override_wins() {
        let cfg = AppConfig {
            theme: ThemeConfig {
                scheme: Some("light".to_string()),
                ..Default::default()
            },
            preview: PreviewConfig {
                syntax_theme: Some("Solarized (dark)".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            cfg.syntax_theme_name(cfg.theme_scheme()),
            "Solarized (dark)"
        );
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
        assert!(merged.show_hidden()); // overridden
        assert!(merged.confirm_delete()); // from base
        assert_eq!(merged.head_lines(), 100); // overridden
        assert_eq!(merged.tail_lines(), 20); // from base
    }

    #[test]
    fn test_merge_none_does_not_clear_some() {
        let base = AppConfig {
            watcher: WatcherConfig {
                enabled: Some(false),
                debounce_ms: Some(500),
                auto_refresh: None,
                ..Default::default()
            },
            ..Default::default()
        };
        let over = AppConfig::default(); // all None

        let merged = base.merge(&over);
        assert!(!merged.watcher_enabled()); // base preserved
        assert_eq!(merged.debounce_ms(), 500); // base preserved
    }

    #[test]
    fn test_watcher_mode_round_trip_and_unknown_fallback() {
        let toml = r#"
[watcher]
enabled = true
mode = "polling"
poll_interval_ms = 1500
"#;
        let cfg: AppConfig = toml::from_str(toml).expect("parse failed");
        assert_eq!(cfg.watcher_mode(), WatcherMode::Polling);
        assert_eq!(cfg.watcher_mode().as_str(), "polling");
        assert_eq!(cfg.poll_interval_ms(), 1500);

        // Default (unset) is the event backend with the documented interval.
        let default = AppConfig::default();
        assert_eq!(default.watcher_mode(), WatcherMode::Event);
        assert_eq!(default.watcher_mode().as_str(), "event");
        assert_eq!(default.poll_interval_ms(), DEFAULT_POLL_INTERVAL_MS);

        // Unknown spellings must not silently disable watching.
        let unknown: AppConfig = toml::from_str("[watcher]\nmode = \"nonsense\"\n").unwrap();
        assert_eq!(unknown.watcher_mode(), WatcherMode::Event);
    }

    #[test]
    fn test_watcher_poll_interval_is_clamped() {
        let low: AppConfig = toml::from_str("[watcher]\npoll_interval_ms = 1\n").unwrap();
        assert_eq!(low.poll_interval_ms(), MIN_POLL_INTERVAL_MS);
        let zero: AppConfig = toml::from_str("[watcher]\npoll_interval_ms = 0\n").unwrap();
        assert_eq!(zero.poll_interval_ms(), MIN_POLL_INTERVAL_MS);
        let high: AppConfig = toml::from_str("[watcher]\npoll_interval_ms = 999_999\n").unwrap();
        assert_eq!(high.poll_interval_ms(), MAX_POLL_INTERVAL_MS);
    }

    #[test]
    fn test_watcher_mode_merge_precedence_keeps_base_when_override_unset() {
        let base = AppConfig {
            watcher: WatcherConfig {
                mode: Some("polling".to_string()),
                poll_interval_ms: Some(4000),
                ..Default::default()
            },
            ..Default::default()
        };
        // An unset override must not clear the base mode/interval.
        let merged = base.clone().merge(&AppConfig::default());
        assert_eq!(merged.watcher_mode(), WatcherMode::Polling);
        assert_eq!(merged.poll_interval_ms(), 4000);

        // An explicit override wins.
        let over = AppConfig {
            watcher: WatcherConfig {
                mode: Some("event".to_string()),
                poll_interval_ms: Some(1000),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = base.merge(&over);
        assert_eq!(merged.watcher_mode(), WatcherMode::Event);
        assert_eq!(merged.poll_interval_ms(), 1000);
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
        assert!(cfg.show_hidden());
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
        assert!(cfg.show_hidden());
    }

    #[test]
    fn test_load_with_no_files_returns_defaults() {
        // When no files found (env vars not set, no CWD config, no global config),
        // we should get all defaults.
        let cfg = AppConfig::load(None, None);
        assert!(!cfg.show_hidden());
        assert!(cfg.confirm_delete());
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

    #[test]
    fn test_pagination_config_parsing() {
        let toml = r#"
[general]
max_entries_per_page = 2000
search_max_entries = 5000
"#;
        let cfg: AppConfig = toml::from_str(toml).expect("parse failed");
        assert_eq!(cfg.max_entries_per_page(), 2000);
        assert_eq!(cfg.search_max_entries(), 5000);
    }

    #[test]
    fn test_pagination_config_clamping() {
        // Below minimum
        let cfg = AppConfig {
            general: GeneralConfig {
                max_entries_per_page: Some(10),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(cfg.max_entries_per_page(), 100); // clamped to MIN

        // Above maximum
        let cfg = AppConfig {
            general: GeneralConfig {
                max_entries_per_page: Some(100_000),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(cfg.max_entries_per_page(), 50_000); // clamped to MAX
    }

    #[test]
    fn test_pagination_config_merge() {
        let base = AppConfig {
            general: GeneralConfig {
                max_entries_per_page: Some(500),
                search_max_entries: Some(8000),
                ..Default::default()
            },
            ..Default::default()
        };
        let over = AppConfig {
            general: GeneralConfig {
                max_entries_per_page: Some(3000),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = base.merge(&over);
        assert_eq!(merged.max_entries_per_page(), 3000); // overridden
        assert_eq!(merged.search_max_entries(), 8000); // from base
    }

    #[test]
    fn test_s3_head_lines_default() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.s3_head_lines(), 100);
    }

    #[test]
    fn test_s3_head_lines_from_toml() {
        let toml = r#"
[preview]
s3_head_lines = 50
"#;
        let cfg: AppConfig = toml::from_str(toml).expect("parse");
        assert_eq!(cfg.s3_head_lines(), 50);
    }

    #[test]
    fn test_s3_head_lines_merge_override() {
        let base = AppConfig {
            preview: PreviewConfig {
                s3_head_lines: Some(100),
                ..Default::default()
            },
            ..Default::default()
        };
        let over = AppConfig {
            preview: PreviewConfig {
                s3_head_lines: Some(200),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = base.merge(&over);
        assert_eq!(merged.s3_head_lines(), 200);
    }

    #[test]
    fn test_s3_head_lines_cli_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg_path = dir.path().join("config.toml");
        std::fs::write(
            &cfg_path,
            r#"
[preview]
s3_head_lines = 50
"#,
        )
        .expect("write");

        let cli_overrides = AppConfig {
            preview: PreviewConfig {
                s3_head_lines: Some(75),
                ..Default::default()
            },
            ..Default::default()
        };

        let cfg = AppConfig::load(Some(&cfg_path), Some(&cli_overrides));
        assert_eq!(cfg.s3_head_lines(), 75); // CLI override wins
    }

    #[test]
    fn task3_search_keys_default_clamp_and_merge() {
        let cfg = AppConfig::default();
        assert!(cfg.search_exclude_dirs().iter().any(|name| name == ".venv"));
        let limits = cfg.search_limits();
        assert!(limits.validate().is_ok());
        assert_eq!(limits.max_file_bytes, crate::search::DEFAULT_MAX_FILE_BYTES);
        assert_eq!(limits.batch_files, crate::search::DEFAULT_BATCH_FILES);

        let toml = r#"
[general]
search_exclude_dirs = [".git", "vendor"]
search_max_file_bytes = 1024
search_max_files = 25
search_max_hits = 7
search_batch_files = 2
search_batch_hits = 3
search_max_excerpt_bytes = 40
"#;
        let cfg: AppConfig = toml::from_str(toml).expect("parse search keys");
        assert_eq!(cfg.search_exclude_dirs(), vec![".git", "vendor"]);
        let limits = cfg.search_limits();
        assert_eq!(limits.max_file_bytes, 1024);
        assert_eq!(limits.max_files, 25);
        assert_eq!(limits.max_hits, 7);
        assert_eq!(limits.batch_files, 2);
        assert_eq!(limits.batch_hits, 3);
        assert_eq!(limits.max_excerpt_bytes, 40);

        // Zero values are clamped to a valid minimum rather than disabling search.
        let zero: AppConfig =
            toml::from_str("[general]\nsearch_max_hits = 0\nsearch_batch_files = 0\n")
                .expect("parse zero keys");
        let limits = zero.search_limits();
        assert_eq!(limits.max_hits, 1);
        assert_eq!(limits.batch_files, 1);
        assert!(limits.validate().is_ok());

        // Empty exclusion list falls back to the safe built-in defaults.
        let empty: AppConfig =
            toml::from_str("[general]\nsearch_exclude_dirs = []\n").expect("parse empty excludes");
        assert!(empty
            .search_exclude_dirs()
            .iter()
            .any(|name| name == "target"));
    }

    #[test]
    fn test_session_toggle_and_state_directory_merge_and_defaults() {
        // Defaults: sessions enabled, platform state directory used.
        let defaults = AppConfig::default();
        assert!(defaults.session_enabled());
        assert_eq!(defaults.session_state_dir(), default_session_state_dir());

        // Explicit directory wins over the platform default.
        let file: AppConfig =
            toml::from_str("[session]\nenabled = false\nstate_dir = \"/tmp/fm-state\"\n")
                .expect("parse session");
        assert!(!file.session_enabled());
        assert_eq!(
            file.session_state_dir(),
            Some(std::path::PathBuf::from("/tmp/fm-state"))
        );

        // A blank override falls back to the platform default.
        let blank: AppConfig =
            toml::from_str("[session]\nstate_dir = \"  \"\n").expect("parse blank");
        assert_eq!(blank.session_state_dir(), default_session_state_dir());

        // Merge: the higher-priority source's Some values win field by field.
        let cli = AppConfig {
            session: SessionConfig {
                enabled: Some(true),
                state_dir: None,
            },
            ..Default::default()
        };
        let merged = file.merge(&cli);
        assert!(merged.session_enabled());
        assert_eq!(
            merged.session_state_dir(),
            Some(std::path::PathBuf::from("/tmp/fm-state"))
        );
    }

    #[test]
    fn test_recovery_toggle_state_directory_and_retention_merge_and_defaults() {
        let defaults = AppConfig::default();
        assert!(defaults.recovery_enabled());
        assert_eq!(defaults.recovery_state_dir(), default_recovery_state_dir());
        assert_eq!(
            defaults.recovery_max_records(),
            DEFAULT_RECOVERY_MAX_RECORDS
        );
        assert_eq!(
            defaults.recovery_max_age_secs(),
            DEFAULT_RECOVERY_MAX_AGE_SECS
        );
        assert_eq!(
            defaults.recovery_min_interval_ms(),
            DEFAULT_RECOVERY_MIN_INTERVAL_MS
        );

        let file: AppConfig = toml::from_str(
            "[recovery]\nenabled = false\nstate_dir = \"/tmp/fm-recovery\"\nmax_records = 4\nmax_age_secs = 7200\nmin_interval_ms = 50\n",
        )
        .expect("parse recovery");
        assert!(!file.recovery_enabled());
        assert_eq!(
            file.recovery_state_dir(),
            Some(std::path::PathBuf::from("/tmp/fm-recovery"))
        );
        assert_eq!(file.recovery_max_records(), 4);
        assert_eq!(file.recovery_max_age_secs(), 7200);
        assert_eq!(file.recovery_min_interval_ms(), 50);

        // Clamps keep a misconfiguration bounded rather than disabling retention.
        let clamped: AppConfig =
            toml::from_str("[recovery]\nmax_records = 0\nmax_age_secs = 1\n").expect("clamped");
        assert_eq!(clamped.recovery_max_records(), MIN_RECOVERY_MAX_RECORDS);
        assert_eq!(clamped.recovery_max_age_secs(), MIN_RECOVERY_MAX_AGE_SECS);

        // A blank override falls back to the platform default.
        let blank: AppConfig = toml::from_str("[recovery]\nstate_dir = \"  \"\n").expect("blank");
        assert_eq!(blank.recovery_state_dir(), default_recovery_state_dir());

        // Merge: the higher-priority source's Some values win field by field.
        let cli = AppConfig {
            recovery: RecoveryConfig {
                enabled: Some(true),
                state_dir: None,
                max_records: Some(9),
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = file.merge(&cli);
        assert!(merged.recovery_enabled());
        assert_eq!(
            merged.recovery_state_dir(),
            Some(std::path::PathBuf::from("/tmp/fm-recovery"))
        );
        assert_eq!(merged.recovery_max_records(), 9);
    }

    #[test]
    fn git_indicators_default_enable_and_merge_by_higher_priority_source() {
        // Default: indicators on (Phase 8 Task 1 already refreshes at startup).
        assert!(AppConfig::default().git_enabled());

        // TOML can disable.
        let file: AppConfig = toml::from_str("[git]\nenabled = false\n").unwrap();
        assert!(!file.git_enabled());

        // CLI/partial Some wins over an earlier source field by field.
        let cli = AppConfig {
            git: GitConfig {
                enabled: Some(true),
            },
            ..Default::default()
        };
        assert!(file.clone().merge(&cli).git_enabled());

        // An absent field keeps the lower-priority source.
        let absent: AppConfig = toml::from_str("[git]\n").unwrap();
        assert!(!absent.clone().merge(&file).git_enabled());
        assert!(absent.merge(&AppConfig::default()).git_enabled());
    }

    #[test]
    fn lsp_toml_parses_servers_languages_and_trust() {
        let config: AppConfig = toml::from_str(
            r#"
[lsp]
enabled = false

[lsp.servers.rust]
argv = ["rust-analyzer", "--stdio"]
root_markers = ["Cargo.toml"]

[lsp.servers.python]
argv = ["pylsp"]

[[lsp.trust]]
root = "~/work"
argv = ["./vendor/ra", "--project"]

[lsp.languages]
templ = "html"
"#,
        )
        .unwrap();
        assert_eq!(config.lsp.enabled, Some(false));
        assert!(!config.lsp.enabled());
        let rust = &config.lsp.servers["rust"];
        assert_eq!(rust.argv, vec!["rust-analyzer", "--stdio"]);
        assert_eq!(rust.root_markers, vec!["Cargo.toml"]);
        assert_eq!(config.lsp.servers["python"].argv, vec!["pylsp"]);
        assert_eq!(config.lsp.trust.len(), 1);
        assert_eq!(config.lsp.trust[0].argv, vec!["./vendor/ra", "--project"]);
        assert_eq!(config.lsp.languages["templ"], "html");
        // Bare TOML parse never populates provenance — `load_checked` does.
        assert!(config.lsp_global.servers.is_empty());
        assert!(config.lsp_local.servers.is_empty());
    }

    #[test]
    fn project_layer_trust_entries_are_stripped_but_global_keeps_grants() {
        // `load_checked` marks exactly `cwd/.fm-tui.toml` as the project
        // layer; the strip helper is the testable unit.
        let mut project: AppConfig =
            toml::from_str("[[lsp.trust]]\nroot = \"/tmp\"\nargv = [\"ra\"]\n").unwrap();
        assert_eq!(project.lsp.trust.len(), 1);
        strip_untrusted_trust(&mut project, Path::new("/p/.fm-tui.toml"), true);
        assert!(project.lsp.trust.is_empty());

        // Any other layer keeps its explicit grants.
        let mut global: AppConfig =
            toml::from_str("[[lsp.trust]]\nroot = \"/tmp\"\nargv = [\"ra\"]\n").unwrap();
        strip_untrusted_trust(
            &mut global,
            Path::new("/home/u/.config/fm-tui/config.toml"),
            false,
        );
        assert_eq!(global.lsp.trust.len(), 1);
    }

    #[test]
    fn load_checked_populates_lsp_provenance_and_keeps_trusted_grants() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[lsp.servers.rust]\nargv = [\"rust-analyzer\"]\n\
             [[lsp.trust]]\nroot = \"/tmp\"\nargv = [\"ra\"]\n",
        )
        .unwrap();
        let config = AppConfig::load_checked(Some(&path), None).unwrap();
        // A --config file is a trusted layer: servers land in lsp_global,
        // nothing lands in lsp_local, and its grants survive.
        assert!(config.lsp_global.servers.contains_key("rust"));
        assert!(config.lsp_local.servers.is_empty());
        assert_eq!(config.lsp.trust.len(), 1);
        // The merged view sees the same server (provenance + merged both set).
        assert_eq!(config.lsp.servers["rust"].argv, vec!["rust-analyzer"]);
    }
}
