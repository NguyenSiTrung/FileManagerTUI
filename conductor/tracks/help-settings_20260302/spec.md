# Spec: Settings Panel in Help Overlay

## Overview

Add an interactive Settings panel as a second tab within the existing Help overlay (`?`).
Users can view all TOML-configurable settings with their current values, defaults, and
descriptions, then edit values inline and save to either the global or project-local
config file via a save dialog.

This makes configuration discoverable and editable without leaving the app or manually
editing TOML files — ideal for new users and quick adjustments.

## Functional Requirements

### FR-1: Tabbed Help Overlay
- The Help overlay (`?`) gains two tabs: **Keybindings** (current content) and **Settings**.
- Tab/Shift+Tab or ←/→ switches between tabs.
- The active tab is visually highlighted in the overlay title bar.
- Default tab on open is "Keybindings" (preserves current behavior).

### FR-2: Settings Panel — View
- Lists ALL config fields from `AppConfig`, grouped by section:
  - General, Preview, Tree, Watcher, Terminal, Theme
- Each entry displays:
  - **Field name** (e.g., `show_hidden`)
  - **Current effective value** (resolved after merge)
  - **Default value** (in parentheses, dimmed)
  - **Description** (brief tooltip text explaining the setting)
- Sections have category headers matching the TOML `[section]` names.
- Scrollable with j/k/↑/↓, g/G for top/bottom.

### FR-3: Settings Panel — Edit
- Navigate to a setting with ↑/↓.
- **Boolean fields**: Press Enter or Space to toggle (`true` ↔ `false`).
- **Numeric fields**: Press Enter to enter inline edit mode, type a new number, press Enter to confirm or Esc to cancel.
- **String/enum fields**: Press Enter to enter inline edit mode, type a new value, press Enter to confirm or Esc to cancel. For enum-like fields (sort_by, scheme, default_view_mode), cycle through valid options with Enter.
- Changed fields are marked with a `[*]` modified indicator.
- Changes are buffered — they do NOT apply until saved.

### FR-4: Reset to Default
- Press Backspace/Delete on a setting to reset it to its built-in default.
- The field is marked as modified (since it changed from the file value).

### FR-5: Save Dialog
- Press Ctrl+S to save modified settings.
- A dialog appears: **"Save to: [G]lobal (~/.config/fm-tui/config.toml) / [L]ocal (.fm-tui.toml) / [C]ancel"**
- On G or L:
  - Read existing file (if any), merge changes into it (preserving comments where possible via raw TOML write), write back.
  - If the file doesn't exist, create it with only the modified sections/fields.
  - Apply changes to the running `App` config immediately.
  - Clear all modified indicators.
  - Show status message: "Settings saved to <path>".
- On C or Esc: Return to settings panel, changes remain buffered.

### FR-6: Open Config File
- Press `o` in the Settings tab to open the global config file (`~/.config/fm-tui/config.toml`) in the inline editor.
- If the file doesn't exist, show a status message suggesting to save settings first.

### FR-7: Discard Changes
- Press Esc in the Settings tab:
  - If no unsaved changes: close the Help overlay (return to Normal mode).
  - If unsaved changes exist: show a confirmation: "Discard unsaved changes? (y/n)".

## Non-Functional Requirements

- **NFR-1**: Settings panel must render without lag — all data is in-memory from `AppConfig`.
- **NFR-2**: TOML serialization must produce clean, human-readable output (not minified).
- **NFR-3**: Saving must not corrupt existing config files — read-modify-write with proper error handling.

## Acceptance Criteria

1. Pressing `?` opens Help overlay on "Keybindings" tab (backward compatible).
2. Tab key switches to "Settings" tab showing all config fields grouped by section.
3. Each setting shows name, current value, default, and description.
4. Boolean fields toggle on Enter/Space; numeric/string fields support inline editing.
5. Modified fields show `[*]` indicator.
6. Backspace resets a field to its default value.
7. Ctrl+S opens save dialog with Global/Local/Cancel options.
8. Saving writes correct TOML to the selected file and applies changes live.
9. `o` opens the config file in the inline editor.
10. Esc with unsaved changes prompts for confirmation.
11. All existing Help overlay behavior (keybindings tab, scroll, close) is preserved.

## Out of Scope

- Theme color customization UI (editing individual hex colors) — too complex for v1.
- Keybinding remapping UI — separate feature.
- Config file syntax validation beyond TOML structure.
- Undo/redo for config changes (just reset-to-default).
