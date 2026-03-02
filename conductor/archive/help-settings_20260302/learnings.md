# Learnings: Settings Panel in Help Overlay

## Inherited Patterns
- (from previous tracks — see patterns.md for full reference)

## New Patterns from This Track

### Tab-Aware Overlay Architecture
When adding multiple modes/tabs to an overlay, pass the full state struct to the widget
rather than individual fields. This allows the widget to render conditionally based on state
without needing multiple constructors or render paths.
- **Example**: `HelpOverlay::new(&theme, &help_state)` instead of `HelpOverlay::new(&theme, scroll_offset)`
- **Benefit**: Future tabs or states can be added without changing the constructor signature.

### Lazy Initialization for Expensive State
Use `Option<State>` and initialize on first demand (e.g., first tab switch) instead of
always constructing at startup. This avoids unnecessary work when the user never opens
that feature.
- **Example**: `help_state.settings_state: Option<SettingsState>` initialized only on first tab switch to Settings.

### Live Config Application Pattern
When saving settings, apply changes to the running `AppConfig` immediately, then serialize
to disk. This provides instant feedback without requiring a restart.
- **Key**: Match on `(section, key)` tuples for a clean dispatch table.
- **Side effects**: Some settings (show_hidden, sort_by, dirs_first, theme) have immediate UI
  side effects (re-sort, re-flatten, re-resolve theme). These must be handled explicitly.

### TOML Merge Strategy
When saving settings, read the existing file, parse to a `toml::Table`, merge modified
entries, and write back. This preserves comments and unmodified settings that might have
been hand-edited by the user.

### Entry Line Index Computation for Auto-Scroll
For variable-height entry lists (section headers take extra lines), compute the line index
of an entry by iterating through all entries before it, counting headers and separators.
This is necessary for accurate auto-scrolling.

### Type Bridge Pattern: SettingValueKind
Use an intermediate enum (`SettingValueKind`) to bridge between the config's heterogeneous
field types and a uniform UI/editing API. This allows generic toggle/cycle/edit operations
without knowing the specific config field type.
