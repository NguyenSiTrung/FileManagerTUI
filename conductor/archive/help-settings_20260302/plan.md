# Plan: Settings Panel in Help Overlay

## Phase 1: Settings State & Data Model

- [ ] Task 1: Create `SettingsState` struct and `SettingEntry` model in `src/components/settings.rs`
  - [ ] Define `SettingValueKind` enum: `Bool(bool)`, `UInt(u64)`, `Str(String)`, `Enum(String, Vec<String>)`
  - [ ] Define `SettingEntry` struct: section, key, description, current_value, default_value, modified_value (Option), kind
  - [ ] Define `SettingsState` struct: entries Vec, selected_index, scroll_offset, editing (bool), edit_buffer (String), modified_count
  - [ ] Write `SettingsState::from_config(config: &AppConfig)` that populates all entries from the config sections with descriptions and defaults
  - [ ] Write unit tests: `from_config` produces correct entry count, all sections represented, default values match constants

- [ ] Task 2: Extend `HelpState` with tab support in `src/components/help.rs` and `src/app.rs`
  - [ ] Add `HelpTab` enum: `Keybindings`, `Settings`
  - [ ] Add `active_tab: HelpTab` field to `HelpState`
  - [ ] Add `settings_state: Option<SettingsState>` to `HelpState` (lazily initialized on tab switch)
  - [ ] Write unit tests: default tab is Keybindings, tab toggle cycles correctly

## Phase 2: Settings Widget (View)

- [ ] Task 1: Implement `SettingsWidget` in `src/components/settings.rs`
  - [ ] Build render method: grouped by section headers (General, Preview, Tree, Watcher, Terminal, Theme)
  - [ ] Render each entry as: `  field_name          current_value  (default: X)  description`
  - [ ] Highlight selected row with theme's `tree_selected_bg/fg`
  - [ ] Show `[*]` indicator next to modified entries
  - [ ] Implement scroll with the same pattern as HelpOverlay (skip/take on lines)
  - [ ] Write unit tests: widget builds correct number of lines, modified indicator appears

- [ ] Task 2: Update `HelpOverlay` to support tabbed rendering
  - [ ] Add tab bar rendering at the top: `[ Keybindings ]  [ Settings ]` with active tab highlighted
  - [ ] When `active_tab == Keybindings`: render current keybinding content (unchanged)
  - [ ] When `active_tab == Settings`: render `SettingsWidget` inside the overlay area
  - [ ] Update `total_lines()` to be tab-aware
  - [ ] Write unit tests: tab bar renders, content switches based on active tab

## Phase 3: Settings Input Handling

- [ ] Task 1: Update `handle_help_mode` in `src/handler.rs` for tab switching
  - [ ] Tab / Shift+Tab or ←/→ switches between Keybindings and Settings tabs
  - [ ] On switch to Settings: lazily initialize `SettingsState::from_config(&app.config)` if not already initialized
  - [ ] Scroll keys (j/k/g/G) apply to whichever tab is active
  - [ ] Write unit tests: Tab key switches tab, scroll applies to active tab

- [ ] Task 2: Implement settings editing in handler
  - [ ] When in Settings tab, Enter/Space toggles booleans
  - [ ] Enter on numeric/string fields enters inline edit mode (edit_buffer = current value string)
  - [ ] In edit mode: character input appends to buffer, Backspace deletes, Enter confirms, Esc cancels
  - [ ] For enum fields: Enter cycles through valid options (no edit buffer needed)
  - [ ] Backspace/Delete on non-editing field resets to default
  - [ ] Track modified fields with `modified_value: Some(new_value)` on each entry
  - [ ] Write unit tests: boolean toggle, numeric input, enum cycling, reset to default

## Phase 4: Save Dialog & Config Persistence

- [ ] Task 1: Add save dialog and TOML serialization to `src/config.rs`
  - [ ] Add `serialize_to_toml(entries: &[SettingEntry]) -> String` function that generates clean TOML output
  - [ ] Add `save_config_to_file(path: &Path, entries: &[SettingEntry]) -> Result<()>` that reads existing file, merges changes, and writes back
  - [ ] Only write sections/fields that have been modified (sparse write)
  - [ ] Write unit tests: serialization produces valid TOML, merge preserves existing fields

- [ ] Task 2: Implement Ctrl+S save flow and apply logic in handler
  - [ ] Add `DialogKind::SaveSettings` variant with `Global`/`Local`/`Cancel` options
  - [ ] Ctrl+S in Settings tab opens SaveSettings dialog
  - [ ] On Global: save to `~/.config/fm-tui/config.toml`, apply to `app.config`
  - [ ] On Local: save to `.fm-tui.toml` in CWD, apply to `app.config`
  - [ ] Apply changes live to the running App (update `app.config` fields, re-derive affected state like theme_colors)
  - [ ] Clear modified indicators after save
  - [ ] Show status message: "Settings saved to <path>"
  - [ ] Write unit tests: save dialog opens, save writes correct file, config applies live

- [ ] Task 3: Implement `o` key (open config file) and Esc discard confirmation
  - [ ] `o` key in Settings tab: resolve global config path, open in editor via `enter_edit_mode()`
  - [ ] Esc in Settings tab with unsaved changes: show "Discard unsaved changes? (y/n)" confirmation
  - [ ] Esc with no unsaved changes: close help overlay normally
  - [ ] Write unit tests: `o` opens config file path, Esc with changes prompts, Esc without changes closes

## Phase 5: UI Integration & Polish

- [ ] Task 1: Update `ui.rs` to render the tabbed help overlay
  - [ ] Pass `HelpState` (including active_tab and settings_state) to the `HelpOverlay` widget
  - [ ] Ensure overlay sizing accommodates settings content (wider if needed)
  - [ ] Write unit tests: render doesn't panic, overlay displays correctly

- [ ] Task 2: Final integration testing and polish
  - [ ] Full manual verification: open help → switch to settings → view all sections → edit values → save → verify config file
  - [ ] Verify backward compatibility: `?` opens help on Keybindings tab, existing scroll and close behavior preserved
  - [ ] Run `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`

- [ ] Task: Conductor - User Manual Verification 'UI Integration & Polish' (Protocol in workflow.md)
