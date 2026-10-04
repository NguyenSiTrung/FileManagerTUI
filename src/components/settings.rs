use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Widget},
};

use crate::config::{
    AppConfig, DEFAULT_DEBOUNCE_MS, DEFAULT_HEAD_LINES, DEFAULT_MAX_EDITOR_BYTES,
    DEFAULT_MAX_EDITOR_LINES, DEFAULT_MAX_ENTRIES_PER_PAGE, DEFAULT_MAX_FULL_PREVIEW_BYTES,
    DEFAULT_POLL_INTERVAL_MS, DEFAULT_PREVIEW_TIMEOUT_MS, DEFAULT_S3_HEAD_LINES,
    DEFAULT_SCROLL_LINES, DEFAULT_SEARCH_MAX_ENTRIES, DEFAULT_SNAPSHOT_MAX_ENTRIES,
    DEFAULT_TAIL_LINES,
};
use crate::theme::ThemeColors;

// ── Data model ───────────────────────────────────────────────────────────────

/// The kind of a setting value, used for editing behavior.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingValueKind {
    Bool(bool),
    UInt(u64),
    Str(String),
    /// Enum-like: current value + list of valid options.
    Enum(String, Vec<String>),
}

impl SettingValueKind {
    /// Display the value as a string.
    pub fn display(&self) -> String {
        match self {
            SettingValueKind::Bool(b) => b.to_string(),
            SettingValueKind::UInt(n) => n.to_string(),
            SettingValueKind::Str(s) => s.clone(),
            SettingValueKind::Enum(s, _) => s.clone(),
        }
    }
}

/// A single config setting entry for display/editing.
#[derive(Debug, Clone)]
pub struct SettingEntry {
    /// TOML section name (e.g., "general", "preview").
    pub section: &'static str,
    /// Field key (e.g., "show_hidden").
    pub key: &'static str,
    /// Human-readable description.
    pub description: &'static str,
    /// Current effective value.
    pub current_value: SettingValueKind,
    /// Built-in default value.
    pub default_value: SettingValueKind,
    /// Buffered modified value (None = unchanged).
    pub modified_value: Option<SettingValueKind>,
}

impl SettingEntry {
    /// The display value: modified if set, else current.
    pub fn effective_display(&self) -> String {
        self.modified_value
            .as_ref()
            .unwrap_or(&self.current_value)
            .display()
    }

    /// Whether this entry has been modified from the current value.
    pub fn is_modified(&self) -> bool {
        self.modified_value.is_some()
    }
}

/// State for the settings panel within the help overlay.
#[derive(Debug, Default)]
pub struct SettingsState {
    /// All setting entries, ordered by section.
    pub entries: Vec<SettingEntry>,
    /// Currently selected entry index.
    pub selected_index: usize,
    /// Scroll offset for rendering.
    pub scroll_offset: usize,
    /// Whether we're in inline edit mode (for numeric/string fields).
    pub editing: bool,
    /// Buffer for inline text editing.
    pub edit_buffer: String,
}

impl SettingsState {
    /// Typed partial TOML patch; never silently truncate an integer or skip a section.
    pub fn modified_table(&self) -> Result<toml::Table, String> {
        let mut table = toml::Table::new();
        for entry in &self.entries {
            let Some(value) = &entry.modified_value else {
                continue;
            };
            let value = match value {
                SettingValueKind::Bool(v) => toml::Value::Boolean(*v),
                SettingValueKind::UInt(v) => toml::Value::Integer(
                    i64::try_from(*v).map_err(|_| "Setting exceeds TOML integer range")?,
                ),
                SettingValueKind::Str(v) | SettingValueKind::Enum(v, _) => {
                    toml::Value::String(v.clone())
                }
            };
            let section = table
                .entry(entry.section.to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            section
                .as_table_mut()
                .ok_or("Invalid setting section")?
                .insert(entry.key.to_string(), value);
        }
        Ok(table)
    }

    /// Merge a partial upgrade, validating its complete effective keymap before mutation.
    pub fn merged_config(&self, current: &AppConfig) -> Result<AppConfig, String> {
        let patch: AppConfig = toml::Value::Table(self.modified_table()?)
            .try_into()
            .map_err(|e| format!("Invalid settings: {e}"))?;
        let mut merged = current.clone().merge(&patch);
        if let Some(n) = merged.terminal.scrollback_lines.as_mut() {
            *n = (*n).min(crate::terminal::emulator::MAX_SCROLLBACK_LINES);
        }
        if merged
            .preview
            .syntax_theme
            .as_ref()
            .is_some_and(|name| name.trim().is_empty())
        {
            merged.preview.syntax_theme = None;
        }
        crate::keymap::Keymap::compile(&merged.keymap)?;
        let layout = merged.layout.state();
        merged.layout.explorer_width = Some(layout.explorer_width());
        merged.layout.terminal_height = Some(layout.terminal_height());
        Ok(merged)
    }

    /// Display originating document-local/runtime preferences, not stale global defaults.
    pub fn from_app(app: &crate::app::App) -> Self {
        let mut state = Self::from_config(&app.config);
        let fallback = crate::commands::CommandContext::capture(app);
        let origin = app.help_state.origin.as_ref().unwrap_or(&fallback);
        let wrap = origin
            .text_view_document(app)
            .and_then(|id| app.workspace.documents.get(id))
            .map_or(app.preview_state.line_wrap, |document| {
                document.editor.line_wrap
            });
        for entry in &mut state.entries {
            match (entry.section, entry.key) {
                ("preview", "line_wrap") => entry.current_value = SettingValueKind::Bool(wrap),
                ("watcher", "auto_refresh") => {
                    entry.current_value = SettingValueKind::Bool(app.watcher_active)
                }
                ("layout", "explorer_width") => {
                    entry.current_value =
                        SettingValueKind::UInt(app.workspace.layout.explorer_width().into())
                }
                ("layout", "terminal_height") => {
                    entry.current_value =
                        SettingValueKind::UInt(app.workspace.layout.terminal_height().into())
                }
                ("layout", "explorer_visible") => {
                    entry.current_value =
                        SettingValueKind::Bool(app.workspace.layout.explorer_visible())
                }
                ("layout", "terminal_visible") => {
                    entry.current_value =
                        SettingValueKind::Bool(app.workspace.layout.terminal_visible())
                }
                _ => {}
            }
        }
        state
    }

    /// Build the settings state from the current app configuration.
    pub fn from_config(config: &AppConfig) -> Self {
        let layout = config.layout.state();
        let entries = vec![
            // ── General ──────────────────────────────────────────────────
            SettingEntry {
                section: "general",
                key: "show_hidden",
                description: "Show hidden (dot) files by default",
                current_value: SettingValueKind::Bool(config.show_hidden()),
                default_value: SettingValueKind::Bool(false),
                modified_value: None,
            },
            SettingEntry {
                section: "general",
                key: "confirm_delete",
                description: "Confirm before deleting files",
                current_value: SettingValueKind::Bool(config.confirm_delete()),
                default_value: SettingValueKind::Bool(true),
                modified_value: None,
            },
            SettingEntry {
                section: "general",
                key: "mouse",
                description: "Mouse support preference (capture requires restart)",
                current_value: SettingValueKind::Bool(config.mouse_enabled()),
                default_value: SettingValueKind::Bool(true),
                modified_value: None,
            },
            SettingEntry {
                section: "general",
                key: "max_entries_per_page",
                description: "Max entries per page when expanding dirs (100–50000)",
                current_value: SettingValueKind::UInt(config.max_entries_per_page() as u64),
                default_value: SettingValueKind::UInt(DEFAULT_MAX_ENTRIES_PER_PAGE as u64),
                modified_value: None,
            },
            SettingEntry {
                section: "general",
                key: "search_max_entries",
                description: "Max entries for deep search walk",
                current_value: SettingValueKind::UInt(config.search_max_entries() as u64),
                default_value: SettingValueKind::UInt(DEFAULT_SEARCH_MAX_ENTRIES as u64),
                modified_value: None,
            },
            SettingEntry {
                section: "general",
                key: "snapshot_max_entries",
                description: "Max entries in DirSnapshot (10000–5000000)",
                current_value: SettingValueKind::UInt(config.snapshot_max_entries() as u64),
                default_value: SettingValueKind::UInt(DEFAULT_SNAPSHOT_MAX_ENTRIES as u64),
                modified_value: None,
            },
            SettingEntry {
                section: "general",
                key: "max_editor_bytes",
                description: "Max file size in bytes for editor",
                current_value: SettingValueKind::UInt(config.max_editor_bytes()),
                default_value: SettingValueKind::UInt(DEFAULT_MAX_EDITOR_BYTES),
                modified_value: None,
            },
            SettingEntry {
                section: "general",
                key: "max_editor_lines",
                description: "Max line count for editor",
                current_value: SettingValueKind::UInt(config.max_editor_lines() as u64),
                default_value: SettingValueKind::UInt(DEFAULT_MAX_EDITOR_LINES as u64),
                modified_value: None,
            },
            // ── Preview ──────────────────────────────────────────────────
            SettingEntry {
                section: "preview",
                key: "enabled",
                description: "Enable the preview panel",
                current_value: SettingValueKind::Bool(config.preview_enabled()),
                default_value: SettingValueKind::Bool(true),
                modified_value: None,
            },
            SettingEntry {
                section: "preview",
                key: "max_full_preview_bytes",
                description: "Max file size for full preview (above → head+tail)",
                current_value: SettingValueKind::UInt(config.max_full_preview_bytes()),
                default_value: SettingValueKind::UInt(DEFAULT_MAX_FULL_PREVIEW_BYTES),
                modified_value: None,
            },
            SettingEntry {
                section: "preview",
                key: "head_lines",
                description: "Lines from top of large files",
                current_value: SettingValueKind::UInt(config.head_lines() as u64),
                default_value: SettingValueKind::UInt(DEFAULT_HEAD_LINES as u64),
                modified_value: None,
            },
            SettingEntry {
                section: "preview",
                key: "tail_lines",
                description: "Lines from bottom of large files",
                current_value: SettingValueKind::UInt(config.tail_lines() as u64),
                default_value: SettingValueKind::UInt(DEFAULT_TAIL_LINES as u64),
                modified_value: None,
            },
            SettingEntry {
                section: "preview",
                key: "default_view_mode",
                description: "Default view for large files",
                current_value: SettingValueKind::Enum(
                    config.preview_view_mode().to_string(),
                    vec![
                        "full".to_string(),
                        "head_and_tail".to_string(),
                        "head_only".to_string(),
                        "tail_only".to_string(),
                    ],
                ),
                default_value: SettingValueKind::Enum(
                    "full".to_string(),
                    vec![
                        "full".to_string(),
                        "head_and_tail".to_string(),
                        "head_only".to_string(),
                        "tail_only".to_string(),
                    ],
                ),
                modified_value: None,
            },
            SettingEntry {
                section: "preview",
                key: "tab_width",
                description: "Tab rendering width",
                current_value: SettingValueKind::UInt(config.preview.tab_width.unwrap_or(4) as u64),
                default_value: SettingValueKind::UInt(4),
                modified_value: None,
            },
            SettingEntry {
                section: "preview",
                key: "line_wrap",
                description: "Wrap originating text view; other documents retain their state",
                current_value: SettingValueKind::Bool(config.preview.line_wrap.unwrap_or(false)),
                default_value: SettingValueKind::Bool(false),
                modified_value: None,
            },
            SettingEntry {
                section: "preview",
                key: "syntax_theme",
                description: "Syntax highlighting theme name",
                current_value: SettingValueKind::Str(
                    config.syntax_theme_name(config.theme_scheme()).to_string(),
                ),
                default_value: SettingValueKind::Str(
                    if config.theme_scheme().eq_ignore_ascii_case("light") {
                        "InspiredGitHub"
                    } else {
                        "base16-ocean.dark"
                    }
                    .to_string(),
                ),
                modified_value: None,
            },
            SettingEntry {
                section: "preview",
                key: "preview_timeout_ms",
                description: "Timeout for directory deep scans (ms)",
                current_value: SettingValueKind::UInt(config.preview_timeout_ms()),
                default_value: SettingValueKind::UInt(DEFAULT_PREVIEW_TIMEOUT_MS),
                modified_value: None,
            },
            SettingEntry {
                section: "preview",
                key: "s3_head_lines",
                description: "Lines to stream for S3 head preview",
                current_value: SettingValueKind::UInt(config.s3_head_lines() as u64),
                default_value: SettingValueKind::UInt(DEFAULT_S3_HEAD_LINES as u64),
                modified_value: None,
            },
            // ── Tree ─────────────────────────────────────────────────────
            SettingEntry {
                section: "tree",
                key: "sort_by",
                description: "Sort order for tree entries",
                current_value: SettingValueKind::Enum(
                    config.sort_by().to_string(),
                    vec![
                        "name".to_string(),
                        "size".to_string(),
                        "modified".to_string(),
                    ],
                ),
                default_value: SettingValueKind::Enum(
                    "name".to_string(),
                    vec![
                        "name".to_string(),
                        "size".to_string(),
                        "modified".to_string(),
                    ],
                ),
                modified_value: None,
            },
            SettingEntry {
                section: "tree",
                key: "dirs_first",
                description: "List directories before files",
                current_value: SettingValueKind::Bool(config.dirs_first()),
                default_value: SettingValueKind::Bool(true),
                modified_value: None,
            },
            SettingEntry {
                section: "tree",
                key: "use_icons",
                description: "Use nerd font icons (false = ASCII)",
                current_value: SettingValueKind::Bool(config.use_icons()),
                default_value: SettingValueKind::Bool(true),
                modified_value: None,
            },
            SettingEntry {
                section: "tree",
                key: "scroll_lines",
                description: "Lines to scroll per mouse wheel tick (1–10)",
                current_value: SettingValueKind::UInt(config.scroll_lines() as u64),
                default_value: SettingValueKind::UInt(DEFAULT_SCROLL_LINES as u64),
                modified_value: None,
            },
            // ── Watcher ──────────────────────────────────────────────────
            SettingEntry {
                section: "watcher",
                key: "enabled",
                description:
                    "Enable change detection entirely (spawns backend on restart; full disable)",
                current_value: SettingValueKind::Bool(config.watcher_enabled()),
                default_value: SettingValueKind::Bool(false),
                modified_value: None,
            },
            SettingEntry {
                section: "watcher",
                key: "debounce_ms",
                description: "Debounce interval in milliseconds (restart required)",
                current_value: SettingValueKind::UInt(config.debounce_ms()),
                default_value: SettingValueKind::UInt(DEFAULT_DEBOUNCE_MS),
                modified_value: None,
            },
            SettingEntry {
                section: "watcher",
                key: "auto_refresh",
                description: "Tree auto-refresh on changes; detection stays on for open documents",
                current_value: SettingValueKind::Bool(config.watcher_auto_refresh()),
                default_value: SettingValueKind::Bool(false),
                modified_value: None,
            },
            SettingEntry {
                section: "watcher",
                key: "mode",
                description: "Backend: event notifications or polling (restart required)",
                current_value: SettingValueKind::Enum(
                    config.watcher_mode().as_str().to_string(),
                    vec!["event".to_string(), "polling".to_string()],
                ),
                default_value: SettingValueKind::Enum(
                    "event".to_string(),
                    vec!["event".to_string(), "polling".to_string()],
                ),
                modified_value: None,
            },
            SettingEntry {
                section: "watcher",
                key: "poll_interval_ms",
                description: "Polling interval ms (250–60000; restart required)",
                current_value: SettingValueKind::UInt(config.poll_interval_ms()),
                default_value: SettingValueKind::UInt(DEFAULT_POLL_INTERVAL_MS),
                modified_value: None,
            },
            // ── Recovery ─────────────────────────────────────────────────
            SettingEntry {
                section: "recovery",
                key: "enabled",
                description: "Persist bounded private recovery snapshots for dirty documents",
                current_value: SettingValueKind::Bool(config.recovery_enabled()),
                default_value: SettingValueKind::Bool(true),
                modified_value: None,
            },
            // ── Git ──────────────────────────────────────────────────────
            SettingEntry {
                section: "git",
                key: "enabled",
                description: "Show optional read-only Git branch/status indicators",
                current_value: SettingValueKind::Bool(config.git_enabled()),
                default_value: SettingValueKind::Bool(true),
                modified_value: None,
            },
            // ── LSP ──────────────────────────────────────────────────────
            SettingEntry {
                section: "lsp",
                key: "enabled",
                description: "Start configured language servers for open documents",
                current_value: SettingValueKind::Bool(config.lsp.enabled()),
                default_value: SettingValueKind::Bool(true),
                modified_value: None,
            },
            SettingEntry {
                section: "layout",
                key: "explorer_width",
                description: "Saved explorer columns (16–80)",
                current_value: SettingValueKind::UInt(layout.explorer_width().into()),
                default_value: SettingValueKind::UInt(24),
                modified_value: None,
            },
            SettingEntry {
                section: "layout",
                key: "explorer_visible",
                description: "Show explorer when viewport permits",
                current_value: SettingValueKind::Bool(layout.explorer_visible()),
                default_value: SettingValueKind::Bool(true),
                modified_value: None,
            },
            SettingEntry {
                section: "layout",
                key: "terminal_height",
                description: "Saved terminal rows including header (4–60)",
                current_value: SettingValueKind::UInt(layout.terminal_height().into()),
                default_value: SettingValueKind::UInt(7),
                modified_value: None,
            },
            SettingEntry {
                section: "layout",
                key: "terminal_visible",
                description: "Show unstarted terminal; explicit action starts shell",
                current_value: SettingValueKind::Bool(layout.terminal_visible()),
                default_value: SettingValueKind::Bool(false),
                modified_value: None,
            },
            // ── Terminal ─────────────────────────────────────────────────
            SettingEntry {
                section: "terminal",
                key: "enabled",
                description: "Enable embedded terminal",
                current_value: SettingValueKind::Bool(config.terminal_enabled()),
                default_value: SettingValueKind::Bool(true),
                modified_value: None,
            },
            SettingEntry {
                section: "terminal",
                key: "default_shell",
                description: "Shell for next terminal start (no live restart)",
                current_value: SettingValueKind::Str(config.terminal_shell()),
                default_value: SettingValueKind::Str("/bin/sh".to_string()),
                modified_value: None,
            },
            SettingEntry {
                section: "terminal",
                key: "scrollback_lines",
                description: "Terminal history capacity (0–100000; oldest rows removed)",
                current_value: SettingValueKind::UInt(config.terminal_scrollback() as u64),
                default_value: SettingValueKind::UInt(1000),
                modified_value: None,
            },
            // ── Theme ────────────────────────────────────────────────────
            SettingEntry {
                section: "theme",
                key: "scheme",
                description: "Color scheme",
                current_value: SettingValueKind::Enum(
                    config.theme_scheme().to_string(),
                    vec![
                        "dark".to_string(),
                        "light".to_string(),
                        "custom".to_string(),
                    ],
                ),
                default_value: SettingValueKind::Enum(
                    "dark".to_string(),
                    vec![
                        "dark".to_string(),
                        "light".to_string(),
                        "custom".to_string(),
                    ],
                ),
                modified_value: None,
            },
            SettingEntry {
                section: "keymap",
                key: "profile",
                description: "Explicit workspace profile; retains TOML overrides",
                current_value: SettingValueKind::Enum(
                    match config.keymap.profile.unwrap_or_default() {
                        crate::keymap::KeymapProfile::Standard => "standard",
                        crate::keymap::KeymapProfile::Web => "web",
                    }
                    .into(),
                    vec!["standard".into(), "web".into()],
                ),
                default_value: SettingValueKind::Enum(
                    "standard".into(),
                    vec!["standard".into(), "web".into()],
                ),
                modified_value: None,
            },
        ];

        Self {
            entries,
            selected_index: 0,
            scroll_offset: 0,
            editing: false,
            edit_buffer: String::new(),
        }
    }

    /// Count how many entries have been modified.
    pub fn modified_count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_modified()).count()
    }

    /// Get the distinct section names in order of appearance.
    #[cfg(test)]
    pub fn sections(&self) -> Vec<&'static str> {
        let mut seen = Vec::new();
        for entry in &self.entries {
            if !seen.contains(&entry.section) {
                seen.push(entry.section);
            }
        }
        seen
    }

    /// Move selection down by one.
    pub fn select_next(&mut self) {
        if self.selected_index < self.entries.len().saturating_sub(1) {
            self.selected_index += 1;
        }
    }

    /// Move selection up by one.
    pub fn select_prev(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(1);
    }

    /// Jump to first entry.
    pub fn select_first(&mut self) {
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    /// Jump to last entry.
    pub fn select_last(&mut self) {
        self.selected_index = self.entries.len().saturating_sub(1);
    }

    /// Compute the line index where the entry at `index` starts.
    /// Each entry renders as 2 lines (key+value line and description line).
    /// Section headers add 1 line (+ 1 blank separator between sections).
    fn entry_line_index(&self, index: usize) -> usize {
        let mut line = 0;
        let mut current_section = "";
        for (i, entry) in self.entries.iter().enumerate() {
            if entry.section != current_section {
                if !current_section.is_empty() {
                    line += 1; // blank separator
                }
                line += 1; // section header
                current_section = entry.section;
            }
            if i == index {
                return line;
            }
            line += 2; // key line + description line
        }
        line
    }

    /// Update scroll offset to keep the selected entry visible,
    /// clamped within valid range.
    /// `visible_height` is the number of visible content lines.
    /// `total_lines` is the total number of content lines.
    pub fn update_scroll(&mut self, visible_height: usize, total_lines: usize) {
        if visible_height == 0 {
            return;
        }

        // Clamp scroll to valid range: can't scroll past the content end
        let max_scroll = total_lines.saturating_sub(visible_height);
        self.scroll_offset = self.scroll_offset.min(max_scroll);

        let entry_start = self.entry_line_index(self.selected_index);
        let entry_end = entry_start + 1; // entry spans 2 lines (0-indexed end)

        // Scroll up if selection is above viewport
        if entry_start < self.scroll_offset {
            self.scroll_offset = entry_start;
        }
        // Scroll down if selection is below viewport
        if entry_end >= self.scroll_offset + visible_height {
            self.scroll_offset = (entry_end + 1).saturating_sub(visible_height);
        }

        // Final clamp
        self.scroll_offset = self.scroll_offset.min(max_scroll);
    }

    /// Toggle a boolean entry at the current selection.
    /// Returns true if the toggle was performed.
    pub fn toggle_bool(&mut self) -> bool {
        if let Some(entry) = self.entries.get_mut(self.selected_index) {
            let current = entry
                .modified_value
                .as_ref()
                .unwrap_or(&entry.current_value);
            if let SettingValueKind::Bool(val) = current {
                entry.modified_value = Some(SettingValueKind::Bool(!val));
                return true;
            }
        }
        false
    }

    /// Cycle an enum entry at the current selection.
    /// Returns true if the cycle was performed.
    pub fn cycle_enum(&mut self) -> bool {
        if let Some(entry) = self.entries.get_mut(self.selected_index) {
            let current = entry
                .modified_value
                .as_ref()
                .unwrap_or(&entry.current_value);
            if let SettingValueKind::Enum(val, options) = current {
                let idx = options.iter().position(|o| o == val).unwrap_or(0);
                let next = (idx + 1) % options.len();
                let new_val = options[next].clone();
                let opts = options.clone();
                entry.modified_value = Some(SettingValueKind::Enum(new_val, opts));
                return true;
            }
        }
        false
    }

    /// Reset the currently selected entry to its default value.
    pub fn reset_to_default(&mut self) {
        if let Some(entry) = self.entries.get_mut(self.selected_index) {
            entry.modified_value = Some(entry.default_value.clone());
        }
    }

    /// Start inline editing for the selected entry (numeric/string).
    /// Returns true if edit mode was entered.
    pub fn start_editing(&mut self) -> bool {
        if let Some(entry) = self.entries.get(self.selected_index) {
            match entry
                .modified_value
                .as_ref()
                .unwrap_or(&entry.current_value)
            {
                SettingValueKind::UInt(_) | SettingValueKind::Str(_) => {
                    self.edit_buffer = entry.effective_display();
                    self.editing = true;
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    /// Confirm inline edit: parse and store the buffer value.
    /// Returns true if the value was accepted.
    pub fn confirm_edit(&mut self) -> bool {
        if !self.editing {
            return false;
        }
        self.editing = false;

        if let Some(entry) = self.entries.get_mut(self.selected_index) {
            let current = entry
                .modified_value
                .as_ref()
                .unwrap_or(&entry.current_value);
            match current {
                SettingValueKind::UInt(_) => {
                    if let Ok(n) = self.edit_buffer.trim().parse::<u64>() {
                        entry.modified_value = Some(SettingValueKind::UInt(n));
                        return true;
                    }
                    // Parse failed — discard
                }
                SettingValueKind::Str(_) => {
                    entry.modified_value = Some(SettingValueKind::Str(self.edit_buffer.clone()));
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    /// Cancel inline edit.
    pub fn cancel_edit(&mut self) {
        self.editing = false;
        self.edit_buffer.clear();
    }

    /// Clear all modified values (after save).
    pub fn clear_modifications(&mut self) {
        for entry in &mut self.entries {
            if let Some(ref modified) = entry.modified_value {
                // After save, the current value becomes the modified value.
                entry.current_value = modified.clone();
            }
            entry.modified_value = None;
        }
    }
}

// ── Widget ───────────────────────────────────────────────────────────────────

/// Section display names for the UI.
fn section_display_name(section: &str) -> &str {
    match section {
        "general" => "General",
        "preview" => "Preview",
        "tree" => "Tree",
        "watcher" => "Watcher",
        "recovery" => "Recovery",
        "terminal" => "Terminal",
        "git" => "Git",
        "layout" => "Layout",
        "keymap" => "Keymap",
        "theme" => "Theme",
        _ => section,
    }
}

/// Settings panel widget.
pub struct SettingsWidget<'a> {
    state: &'a SettingsState,
    theme: &'a ThemeColors,
    block: Option<Block<'a>>,
}

impl<'a> SettingsWidget<'a> {
    pub fn new(state: &'a SettingsState, theme: &'a ThemeColors) -> Self {
        Self {
            state,
            theme,
            block: None,
        }
    }

    #[allow(dead_code)]
    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = block.into();
        self
    }

    /// Build all content lines for the settings panel.
    pub fn build_content_lines(&self) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        let mut current_section = "";

        for (entry_idx, entry) in self.state.entries.iter().enumerate() {
            // Section header
            if entry.section != current_section {
                if !current_section.is_empty() {
                    lines.push(Line::from("")); // blank separator
                }
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("── {} ", section_display_name(entry.section)),
                        Style::default()
                            .fg(self.theme.accent_fg)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("─".repeat(40), Style::default().fg(self.theme.dim_fg)),
                ]));
                current_section = entry.section;
            }

            let is_selected = entry_idx == self.state.selected_index;
            let modified_marker = if entry.is_modified() { "[*] " } else { "    " };

            // If selected and editing, show inline edit buffer
            let value_str = if is_selected && self.state.editing {
                format!("{}▏", self.state.edit_buffer)
            } else {
                entry.effective_display()
            };

            let default_str = format!("(default: {})", entry.default_value.display());

            let key_width = 24;
            let val_width = 16;
            let key_padded = format!("  {}{:<kw$}", modified_marker, entry.key, kw = key_width);
            let val_padded = format!("{:<vw$}", value_str, vw = val_width);

            let base_style = if is_selected {
                Style::default()
                    .bg(self.theme.tree_selected_bg)
                    .fg(self.theme.tree_selected_fg)
            } else {
                Style::default()
            };

            lines.push(Line::from(vec![
                Span::styled(
                    key_padded,
                    base_style
                        .fg(if is_selected {
                            self.theme.tree_selected_fg
                        } else if entry.is_modified() {
                            self.theme.warning_fg
                        } else {
                            self.theme.tree_file_fg
                        })
                        .add_modifier(if is_selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
                Span::styled(
                    val_padded,
                    base_style.fg(if is_selected {
                        self.theme.tree_selected_fg
                    } else {
                        self.theme.tree_file_fg
                    }),
                ),
                Span::styled(
                    format!("  {}", default_str),
                    base_style.fg(self.theme.dim_fg),
                ),
            ]));

            // Description on a sub-line (indented, dimmed)
            lines.push(Line::from(vec![Span::styled(
                format!("      └ {}", entry.description),
                base_style.fg(self.theme.dim_fg),
            )]));
        }

        // Footer
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            " Enter=edit  Space=toggle  Backspace=reset  Ctrl+S=save  o=open config  Esc=close ",
            Style::default().fg(self.theme.dim_fg),
        )]));

        lines
    }

    /// Total number of content lines.
    pub fn total_lines(&self) -> usize {
        self.build_content_lines().len()
    }
}

impl<'a> Widget for SettingsWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Clear the area
        Clear.render(area, buf);

        let inner = if let Some(ref block) = self.block {
            let inner = block.inner(area);
            block.clone().render(area, buf);
            inner
        } else {
            area
        };

        let content_lines = self.build_content_lines();
        let visible_height = inner.height as usize;
        let scroll = self.state.scroll_offset;

        for (i, line) in content_lines
            .iter()
            .skip(scroll)
            .take(visible_height)
            .enumerate()
        {
            let line_y = inner.y + i as u16;
            if line_y >= inner.y + inner.height {
                break;
            }
            buf.set_line(inner.x + 1, line_y, line, inner.width.saturating_sub(2));
        }

        // Scroll indicator
        if content_lines.len() > visible_height {
            let total = content_lines.len();
            let indicator = format!(" {}/{} ", (scroll + 1).min(total), total);
            let ind_span = Span::styled(indicator, Style::default().fg(self.theme.dim_fg));
            let ind_x = area.x + area.width.saturating_sub(ind_span.width() as u16 + 1);
            let ind_y = area.y + area.height - 1;
            buf.set_span(ind_x, ind_y, &ind_span, ind_span.width() as u16);
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    #[test]
    fn adaptive_settings_register_validated_layout_preferences() {
        let config: AppConfig = toml::from_str(
            "[layout]\nexplorer_width = 999\nexplorer_visible = false\nterminal_height = 0\nterminal_visible = true",
        ).unwrap();
        let settings = SettingsState::from_config(&config);
        for (key, value) in [
            ("explorer_width", "80"),
            ("explorer_visible", "false"),
            ("terminal_height", "4"),
            ("terminal_visible", "true"),
        ] {
            assert_eq!(
                settings
                    .entries
                    .iter()
                    .find(|e| e.section == "layout" && e.key == key)
                    .map(SettingEntry::effective_display)
                    .as_deref(),
                Some(value)
            );
        }
    }

    #[test]
    fn task3_watcher_reset_uses_actual_disabled_builtin_default() {
        let config = AppConfig::default();
        let state = SettingsState::from_config(&config);
        let entry = state
            .entries
            .iter()
            .find(|entry| entry.section == "watcher" && entry.key == "enabled")
            .unwrap();
        assert_eq!(
            entry.default_value,
            SettingValueKind::Bool(config.watcher_enabled())
        );
    }

    #[test]
    fn task3_settings_show_origin_document_local_wrap_and_runtime_watcher() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "text").unwrap();
        let mut config = AppConfig::default();
        config.preview.line_wrap = Some(true);
        config.watcher.enabled = Some(true);
        config.watcher.auto_refresh = Some(true);
        let mut app = crate::app::App::new(dir.path(), config).unwrap();
        app.open_document_path(&path, true);
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .toggle_wrap();
        app.watcher_active = false;
        app.set_overlay(crate::app::AppMode::Help);
        let settings = SettingsState::from_app(&app);
        for key in ["line_wrap", "auto_refresh"] {
            assert_eq!(
                settings
                    .entries
                    .iter()
                    .find(|entry| entry.key == key)
                    .unwrap()
                    .current_value,
                SettingValueKind::Bool(false)
            );
        }
    }

    #[test]
    fn git_setting_toggles_merges_and_persists_like_other_options() {
        let config = AppConfig::default();
        let mut state = SettingsState::from_config(&config);
        let index = state
            .entries
            .iter()
            .position(|entry| entry.section == "git" && entry.key == "enabled")
            .expect("git.enabled must be a settings entry");
        assert_eq!(
            state.entries[index].current_value,
            SettingValueKind::Bool(true)
        );

        // Space toggles it off; the modified table persists the typed value.
        state.selected_index = index;
        assert!(state.toggle_bool());
        let table = state.modified_table().unwrap();
        assert_eq!(
            table["git"]["enabled"].as_bool(),
            Some(false),
            "disabled state must round-trip through the settings table"
        );
        // Merge participates: disabling wins the merged config.
        let merged = state.merged_config(&config).unwrap();
        assert!(!merged.git_enabled());
    }

    #[test]
    fn task3_partial_profile_upgrade_retains_bindings_and_clamps_history() {
        use crate::keymap::{BindingOverride, FocusContext, KeymapProfile};
        let mut config = AppConfig::default();
        config.keymap.bindings = Some(vec![BindingOverride {
            command: "document.save".into(),
            context: FocusContext::Editor,
            keys: vec!["F9".into()],
        }]);
        config.preview.enabled = Some(false);
        let mut settings = SettingsState::from_config(&config);
        settings
            .entries
            .iter_mut()
            .find(|e| e.section == "keymap")
            .unwrap()
            .modified_value = Some(SettingValueKind::Enum("web".into(), vec![]));
        settings
            .entries
            .iter_mut()
            .find(|e| e.key == "scrollback_lines")
            .unwrap()
            .modified_value = Some(SettingValueKind::UInt(100_001));
        let merged = settings.merged_config(&config).unwrap();
        assert_eq!(merged.keymap.profile, Some(KeymapProfile::Web));
        assert_eq!(
            crate::keymap::Keymap::compile(&merged.keymap)
                .unwrap()
                .binding_labels(crate::commands::CommandId::Save, FocusContext::Editor),
            vec!["F9"]
        );
        assert!(!merged.preview_enabled());
        assert_eq!(merged.terminal.scrollback_lines, Some(100_000));
        settings
            .entries
            .iter_mut()
            .find(|e| e.key == "scrollback_lines")
            .unwrap()
            .modified_value = Some(SettingValueKind::UInt(u64::MAX));
        assert!(settings.merged_config(&config).is_err());
    }

    #[test]
    fn task3_settings_registers_keymap_profile_explicitly() {
        let mut config = AppConfig::default();
        config.keymap.profile = Some(crate::keymap::KeymapProfile::Web);
        let settings = SettingsState::from_config(&config);
        let profile = settings
            .entries
            .iter()
            .find(|e| e.section == "keymap" && e.key == "profile")
            .expect("registered keymap profile");
        assert_eq!(profile.current_value.display(), "web");
    }

    #[test]
    fn from_config_produces_entries() {
        let config = AppConfig::default();
        let state = SettingsState::from_config(&config);
        // Should have entries for all config sections
        assert!(
            state.entries.len() > 20,
            "Expected 20+ entries, got {}",
            state.entries.len()
        );
    }

    #[test]
    fn all_sections_represented() {
        let config = AppConfig::default();
        let state = SettingsState::from_config(&config);
        let sections = state.sections();
        assert!(sections.contains(&"general"));
        assert!(sections.contains(&"preview"));
        assert!(sections.contains(&"tree"));
        assert!(sections.contains(&"watcher"));
        assert!(sections.contains(&"terminal"));
        assert!(sections.contains(&"theme"));
    }

    #[test]
    fn default_values_match_config() {
        let config = AppConfig::default();
        let state = SettingsState::from_config(&config);

        // Check show_hidden default
        let show_hidden = state
            .entries
            .iter()
            .find(|e| e.key == "show_hidden")
            .unwrap();
        assert_eq!(show_hidden.default_value, SettingValueKind::Bool(false));
        assert_eq!(show_hidden.current_value, SettingValueKind::Bool(false));

        // Check confirm_delete default
        let confirm = state
            .entries
            .iter()
            .find(|e| e.key == "confirm_delete")
            .unwrap();
        assert_eq!(confirm.default_value, SettingValueKind::Bool(true));

        // Check head_lines default
        let head = state
            .entries
            .iter()
            .find(|e| e.key == "head_lines")
            .unwrap();
        assert_eq!(head.default_value, SettingValueKind::UInt(50));
    }

    #[test]
    fn modified_count_starts_at_zero() {
        let config = AppConfig::default();
        let state = SettingsState::from_config(&config);
        assert_eq!(state.modified_count(), 0);
    }

    #[test]
    fn toggle_bool_marks_modified() {
        let config = AppConfig::default();
        let mut state = SettingsState::from_config(&config);
        // select show_hidden (first entry)
        state.selected_index = 0;
        assert!(state.toggle_bool());
        assert_eq!(state.modified_count(), 1);
        assert!(state.entries[0].is_modified());
    }

    #[test]
    fn cycle_enum_works() {
        let config = AppConfig::default();
        let mut state = SettingsState::from_config(&config);
        // Find sort_by entry
        let idx = state
            .entries
            .iter()
            .position(|e| e.key == "sort_by")
            .unwrap();
        state.selected_index = idx;
        assert!(state.cycle_enum());
        // Should cycle from "name" to "size"
        let entry = &state.entries[idx];
        assert_eq!(entry.effective_display(), "size");
    }

    #[test]
    fn reset_to_default_marks_modified() {
        let config = AppConfig::default();
        let mut state = SettingsState::from_config(&config);
        state.selected_index = 0;
        state.reset_to_default();
        // Even though default matches current for default config,
        // it should still be marked as modified (modified_value is Some).
        assert!(state.entries[0].is_modified());
    }

    #[test]
    fn inline_edit_numeric() {
        let config = AppConfig::default();
        let mut state = SettingsState::from_config(&config);
        // Find head_lines
        let idx = state
            .entries
            .iter()
            .position(|e| e.key == "head_lines")
            .unwrap();
        state.selected_index = idx;
        assert!(state.start_editing());
        assert!(state.editing);
        state.edit_buffer = "100".to_string();
        assert!(state.confirm_edit());
        assert!(!state.editing);
        assert_eq!(state.entries[idx].effective_display(), "100");
    }

    #[test]
    fn cancel_edit_reverts() {
        let config = AppConfig::default();
        let mut state = SettingsState::from_config(&config);
        let idx = state
            .entries
            .iter()
            .position(|e| e.key == "head_lines")
            .unwrap();
        state.selected_index = idx;
        state.start_editing();
        state.edit_buffer = "999".to_string();
        state.cancel_edit();
        assert!(!state.editing);
        // Value should still be original
        assert_eq!(state.entries[idx].effective_display(), "50");
    }

    #[test]
    fn clear_modifications_resets_all() {
        let config = AppConfig::default();
        let mut state = SettingsState::from_config(&config);
        state.selected_index = 0;
        state.toggle_bool();
        assert_eq!(state.modified_count(), 1);
        state.clear_modifications();
        assert_eq!(state.modified_count(), 0);
    }

    #[test]
    fn content_lines_nonzero() {
        let config = AppConfig::default();
        let state = SettingsState::from_config(&config);
        let theme = crate::theme::dark_theme();
        let widget = SettingsWidget::new(&state, &theme);
        assert!(widget.total_lines() > 0);
    }

    #[test]
    fn select_navigation() {
        let config = AppConfig::default();
        let mut state = SettingsState::from_config(&config);
        assert_eq!(state.selected_index, 0);
        state.select_next();
        assert_eq!(state.selected_index, 1);
        state.select_prev();
        assert_eq!(state.selected_index, 0);
        state.select_prev(); // Should not go below 0
        assert_eq!(state.selected_index, 0);
        state.select_last();
        assert_eq!(state.selected_index, state.entries.len() - 1);
        state.select_first();
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn watcher_mode_and_poll_interval_round_trip_through_settings() {
        let config = AppConfig::default();
        let mut state = SettingsState::from_config(&config);
        for entry in &mut state.entries {
            match (entry.section, entry.key) {
                ("watcher", "mode") => {
                    entry.modified_value =
                        Some(SettingValueKind::Enum("polling".to_string(), vec![]))
                }
                ("watcher", "poll_interval_ms") => {
                    entry.modified_value = Some(SettingValueKind::UInt(1_500))
                }
                _ => {}
            }
        }
        let merged = state.merged_config(&config).unwrap();
        assert_eq!(merged.watcher_mode(), crate::config::WatcherMode::Polling);
        assert_eq!(merged.poll_interval_ms(), 1_500);

        // A re-render from the merged config preserves the chosen mode (no reset).
        let state = SettingsState::from_config(&merged);
        let mode = state
            .entries
            .iter()
            .find(|e| e.section == "watcher" && e.key == "mode")
            .unwrap();
        assert_eq!(
            mode.current_value,
            SettingValueKind::Enum("polling".into(), vec!["event".into(), "polling".into()])
        );
    }
}
