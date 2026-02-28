//! Theme data model: built-in palettes and resolution from config.
//!
//! The theme system provides two built-in palettes (dark and light) and
//! supports custom color overrides from the config file.

use ratatui::style::Color;

use crate::config::{ThemeColorsConfig, ThemeConfig};

// ── Runtime theme colors ─────────────────────────────────────────────────────

/// All runtime colors used in the UI.
///
/// Constructed from a config-level `ThemeConfig` via `resolve_theme()`.
#[derive(Debug, Clone)]
pub struct ThemeColors {
    // Tree panel
    pub tree_bg: Color,
    pub tree_fg: Color,
    pub tree_selected_bg: Color,
    pub tree_selected_fg: Color,
    pub tree_dir_fg: Color,
    pub tree_file_fg: Color,
    pub tree_hidden_fg: Color,

    // Preview panel
    pub preview_bg: Color,
    pub preview_fg: Color,
    pub preview_line_nr_fg: Color,

    // Status bar
    pub status_bg: Color,
    pub status_fg: Color,

    // Borders & chrome
    pub border_fg: Color,
    pub border_focused_fg: Color,

    // Dialogs
    pub dialog_bg: Color,
    pub dialog_border_fg: Color,

    // Semantic colors (not configurable, consistent across themes)
    pub error_fg: Color,
    pub warning_fg: Color,
    pub success_fg: Color,
    pub info_fg: Color,
    pub accent_fg: Color,
    pub dim_fg: Color,

    // Editor (preview edit mode)
    pub editor_line_nr: Color,
    pub editor_line_nr_current: Color,
    pub editor_gutter_sep: Color,
    pub editor_cursor_fg: Color,
    pub editor_cursor_bg: Color,
    pub editor_current_line_bg: Color,
    pub editor_find_match_bg: Color,
    pub editor_find_bar_bg: Color,
}

// ── Built-in palettes ────────────────────────────────────────────────────────

/// Dark theme using Catppuccin Mocha palette.
pub fn dark_theme() -> ThemeColors {
    ThemeColors {
        // Tree panel — dark base
        tree_bg: Color::Reset,
        tree_fg: Color::Rgb(205, 214, 244),       // #cdd6f4 (text)
        tree_selected_bg: Color::Rgb(69, 71, 90), // #45475a (surface1)
        tree_selected_fg: Color::Rgb(205, 214, 244), // #cdd6f4
        tree_dir_fg: Color::Rgb(137, 180, 250),   // #89b4fa (blue)
        tree_file_fg: Color::Rgb(205, 214, 244),  // #cdd6f4
        tree_hidden_fg: Color::Rgb(108, 112, 134), // #6c7086 (overlay0)

        // Preview — same base
        preview_bg: Color::Reset,
        preview_fg: Color::Rgb(205, 214, 244),
        preview_line_nr_fg: Color::Rgb(108, 112, 134), // #6c7086

        // Status bar
        status_bg: Color::Rgb(30, 30, 46), // #1e1e2e (base)
        status_fg: Color::Rgb(205, 214, 244),

        // Borders
        border_fg: Color::Rgb(88, 91, 112), // #585b70 (surface2)
        border_focused_fg: Color::Rgb(137, 180, 250), // #89b4fa (blue)

        // Dialogs
        dialog_bg: Color::Rgb(49, 50, 68), // #313244 (surface0)
        dialog_border_fg: Color::Rgb(137, 180, 250),

        // Semantic
        error_fg: Color::Rgb(243, 139, 168),   // #f38ba8 (red)
        warning_fg: Color::Rgb(249, 226, 175), // #f9e2af (yellow)
        success_fg: Color::Rgb(166, 227, 161), // #a6e3a1 (green)
        info_fg: Color::Rgb(137, 180, 250),    // #89b4fa (blue)
        accent_fg: Color::Rgb(203, 166, 247),  // #cba6f7 (mauve)
        dim_fg: Color::Rgb(108, 112, 134),     // #6c7086

        // Editor
        editor_line_nr: Color::Rgb(108, 112, 134), // #6c7086 (overlay0)
        editor_line_nr_current: Color::Rgb(249, 226, 175), // #f9e2af (yellow)
        editor_gutter_sep: Color::Rgb(69, 71, 90), // #45475a (surface1)
        editor_cursor_fg: Color::Rgb(30, 30, 46),  // #1e1e2e (base)
        editor_cursor_bg: Color::Rgb(205, 214, 244), // #cdd6f4 (text)
        editor_current_line_bg: Color::Rgb(49, 50, 68), // #313244 (surface0)
        editor_find_match_bg: Color::Rgb(249, 226, 175), // #f9e2af (yellow)
        editor_find_bar_bg: Color::Rgb(49, 50, 68), // #313244 (surface0)
    }
}

/// Light theme — complementary light palette.
pub fn light_theme() -> ThemeColors {
    ThemeColors {
        // Tree panel — light base
        tree_bg: Color::Reset,
        tree_fg: Color::Rgb(76, 79, 105), // #4c4f69 (text)
        tree_selected_bg: Color::Rgb(204, 208, 218), // #ccd0da (surface1)
        tree_selected_fg: Color::Rgb(76, 79, 105),
        tree_dir_fg: Color::Rgb(30, 102, 245), // #1e66f5 (blue)
        tree_file_fg: Color::Rgb(76, 79, 105),
        tree_hidden_fg: Color::Rgb(156, 160, 176), // #9ca0b0 (overlay0)

        // Preview
        preview_bg: Color::Reset,
        preview_fg: Color::Rgb(76, 79, 105),
        preview_line_nr_fg: Color::Rgb(156, 160, 176),

        // Status bar
        status_bg: Color::Rgb(239, 241, 245), // #eff1f5 (base)
        status_fg: Color::Rgb(76, 79, 105),

        // Borders
        border_fg: Color::Rgb(172, 176, 190), // #acb0be (surface2)
        border_focused_fg: Color::Rgb(30, 102, 245),

        // Dialogs
        dialog_bg: Color::Rgb(230, 233, 239), // #e6e9ef (surface0)
        dialog_border_fg: Color::Rgb(30, 102, 245),

        // Semantic
        error_fg: Color::Rgb(210, 15, 57),    // #d20f39 (red)
        warning_fg: Color::Rgb(223, 142, 29), // #df8e1d (yellow)
        success_fg: Color::Rgb(64, 160, 43),  // #40a02b (green)
        info_fg: Color::Rgb(30, 102, 245),
        accent_fg: Color::Rgb(136, 57, 239), // #8839ef (mauve)
        dim_fg: Color::Rgb(156, 160, 176),

        // Editor
        editor_line_nr: Color::Rgb(156, 160, 176), // #9ca0b0
        editor_line_nr_current: Color::Rgb(223, 142, 29), // #df8e1d (yellow)
        editor_gutter_sep: Color::Rgb(204, 208, 218), // #ccd0da (surface1)
        editor_cursor_fg: Color::Rgb(239, 241, 245), // #eff1f5 (base)
        editor_cursor_bg: Color::Rgb(76, 79, 105), // #4c4f69 (text)
        editor_current_line_bg: Color::Rgb(230, 233, 239), // #e6e9ef (surface0)
        editor_find_match_bg: Color::Rgb(223, 142, 29), // #df8e1d (yellow)
        editor_find_bar_bg: Color::Rgb(230, 233, 239), // #e6e9ef
    }
}

// ── Color parsing ────────────────────────────────────────────────────────────

/// Parse a hex color string like `"#aabbcc"` into a `ratatui::style::Color`.
/// Returns `None` for malformed input.
pub fn parse_hex_color(hex: &str) -> Option<Color> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

/// Parse a hex color string, falling back to the provided default on error.
fn parse_or(hex_opt: Option<&str>, fallback: Color) -> Color {
    hex_opt.and_then(parse_hex_color).unwrap_or(fallback)
}

// ── Theme resolution ─────────────────────────────────────────────────────────

/// Resolve the final `ThemeColors` from config.
///
/// - `"dark"` (default): dark Catppuccin palette
/// - `"light"`: light Catppuccin palette
/// - `"custom"`: start from dark palette, then override with custom hex values
pub fn resolve_theme(config: &ThemeConfig) -> ThemeColors {
    let scheme = config.scheme.as_deref().unwrap_or("dark");
    match scheme {
        "light" => light_theme(),
        "custom" => {
            let mut theme = dark_theme();
            if let Some(custom) = &config.custom {
                apply_custom_colors(&mut theme, custom);
            }
            theme
        }
        _ => dark_theme(), // "dark" or any unrecognized value
    }
}

/// Apply custom hex color overrides on top of an existing theme.
fn apply_custom_colors(theme: &mut ThemeColors, custom: &ThemeColorsConfig) {
    if let Some(ref c) = custom.tree_bg {
        theme.tree_bg = parse_or(Some(c), theme.tree_bg);
    }
    if let Some(ref c) = custom.tree_fg {
        theme.tree_fg = parse_or(Some(c), theme.tree_fg);
    }
    if let Some(ref c) = custom.tree_selected_bg {
        theme.tree_selected_bg = parse_or(Some(c), theme.tree_selected_bg);
    }
    if let Some(ref c) = custom.tree_selected_fg {
        theme.tree_selected_fg = parse_or(Some(c), theme.tree_selected_fg);
    }
    if let Some(ref c) = custom.tree_dir_fg {
        theme.tree_dir_fg = parse_or(Some(c), theme.tree_dir_fg);
    }
    if let Some(ref c) = custom.tree_file_fg {
        theme.tree_file_fg = parse_or(Some(c), theme.tree_file_fg);
    }
    if let Some(ref c) = custom.tree_hidden_fg {
        theme.tree_hidden_fg = parse_or(Some(c), theme.tree_hidden_fg);
    }
    if let Some(ref c) = custom.preview_bg {
        theme.preview_bg = parse_or(Some(c), theme.preview_bg);
    }
    if let Some(ref c) = custom.preview_fg {
        theme.preview_fg = parse_or(Some(c), theme.preview_fg);
    }
    if let Some(ref c) = custom.preview_line_nr_fg {
        theme.preview_line_nr_fg = parse_or(Some(c), theme.preview_line_nr_fg);
    }
    if let Some(ref c) = custom.status_bg {
        theme.status_bg = parse_or(Some(c), theme.status_bg);
    }
    if let Some(ref c) = custom.status_fg {
        theme.status_fg = parse_or(Some(c), theme.status_fg);
    }
    if let Some(ref c) = custom.border_fg {
        theme.border_fg = parse_or(Some(c), theme.border_fg);
    }
    if let Some(ref c) = custom.dialog_bg {
        theme.dialog_bg = parse_or(Some(c), theme.dialog_bg);
    }
    if let Some(ref c) = custom.dialog_border_fg {
        theme.dialog_border_fg = parse_or(Some(c), theme.dialog_border_fg);
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_color_valid() {
        assert_eq!(parse_hex_color("#ff0000"), Some(Color::Rgb(255, 0, 0)));
        assert_eq!(parse_hex_color("#00ff00"), Some(Color::Rgb(0, 255, 0)));
        assert_eq!(parse_hex_color("#0000ff"), Some(Color::Rgb(0, 0, 255)));
        assert_eq!(parse_hex_color("#1a1b26"), Some(Color::Rgb(26, 27, 38)));
    }

    #[test]
    fn test_parse_hex_color_without_hash() {
        assert_eq!(parse_hex_color("ff0000"), Some(Color::Rgb(255, 0, 0)));
    }

    #[test]
    fn test_parse_hex_color_invalid() {
        assert_eq!(parse_hex_color("#zzzzzz"), None);
        assert_eq!(parse_hex_color("#fff"), None); // too short
        assert_eq!(parse_hex_color(""), None);
        assert_eq!(parse_hex_color("#"), None);
    }

    #[test]
    fn test_resolve_dark_theme() {
        let config = ThemeConfig {
            scheme: Some("dark".to_string()),
            custom: None,
        };
        let theme = resolve_theme(&config);
        assert_eq!(theme.tree_dir_fg, Color::Rgb(137, 180, 250));
    }

    #[test]
    fn test_resolve_light_theme() {
        let config = ThemeConfig {
            scheme: Some("light".to_string()),
            custom: None,
        };
        let theme = resolve_theme(&config);
        assert_eq!(theme.tree_dir_fg, Color::Rgb(30, 102, 245));
    }

    #[test]
    fn test_resolve_default_is_dark() {
        let config = ThemeConfig::default();
        let theme = resolve_theme(&config);
        assert_eq!(theme.tree_dir_fg, Color::Rgb(137, 180, 250));
    }

    #[test]
    fn test_resolve_custom_overrides() {
        let config = ThemeConfig {
            scheme: Some("custom".to_string()),
            custom: Some(ThemeColorsConfig {
                tree_bg: Some("#1a1b26".to_string()),
                tree_fg: Some("#c0caf5".to_string()),
                ..Default::default()
            }),
        };
        let theme = resolve_theme(&config);
        // Custom values applied
        assert_eq!(theme.tree_bg, Color::Rgb(26, 27, 38));
        assert_eq!(theme.tree_fg, Color::Rgb(192, 202, 245));
        // Non-custom values fall back to dark theme
        assert_eq!(theme.tree_dir_fg, Color::Rgb(137, 180, 250));
    }

    #[test]
    fn test_custom_with_invalid_hex_falls_back() {
        let config = ThemeConfig {
            scheme: Some("custom".to_string()),
            custom: Some(ThemeColorsConfig {
                tree_bg: Some("#zzzzzz".to_string()),
                ..Default::default()
            }),
        };
        let theme = resolve_theme(&config);
        // Invalid hex keeps the dark theme default (Color::Reset for tree_bg)
        assert_eq!(theme.tree_bg, Color::Reset);
    }

    #[test]
    fn test_unknown_scheme_falls_back_to_dark() {
        let config = ThemeConfig {
            scheme: Some("neon".to_string()),
            custom: None,
        };
        let theme = resolve_theme(&config);
        assert_eq!(theme.tree_dir_fg, Color::Rgb(137, 180, 250));
    }

    #[test]
    fn test_dark_and_light_different() {
        let dark = dark_theme();
        let light = light_theme();
        // Key colors should differ between themes
        assert_ne!(dark.tree_fg, light.tree_fg);
        assert_ne!(dark.tree_selected_bg, light.tree_selected_bg);
        assert_ne!(dark.tree_dir_fg, light.tree_dir_fg);
        assert_ne!(dark.error_fg, light.error_fg);
    }
}
