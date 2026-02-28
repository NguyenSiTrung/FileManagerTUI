# Track Learnings: search-action_20260228

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Handler uses 3-level dispatch: global keys → panel-specific keys → dialog keys (from: preview-panel_20260227)
- Use `Clear` widget + centered `Block` for modal overlays in ratatui (from: file-ops-dialogs_20260227)
- Binary detection: check known extensions first (fast), then null-byte scan in 8KB (from: preview-panel_20260227)
- Handler uses mode-based dispatch: `handle_normal_mode` vs `handle_dialog_mode` (from: file-ops-dialogs_20260227)
- PreviewWidget follows same pattern as TreeWidget — struct with `block()` builder, implements `Widget` trait (from: preview-panel_20260227)
- Widget builder pattern: `WidgetName::new(state, theme).block(block)` (from: config-polish_20260228)
- Use `SkimMatcherV2` from `fuzzy-matcher` for fuzzy search (from: fuzzy-search_20260228)
- Terminal input must be routed BEFORE general global keys in handler (from: terminal-panel_20260228)

---

<!-- Learnings from implementation will be appended below -->
