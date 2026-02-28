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

## [2026-02-28 15:45] - All Phases: Search Action Menu
- **Implemented:** Complete search action menu feature across all 5 phases
- **Files changed:** src/app.rs, src/handler.rs, src/ui.rs, src/components/mod.rs, src/components/search_action.rs, src/components/help.rs, README.md
- **Commit:** a4f23aa
- **Learnings:**
  - Patterns: Two-state overlay transition (Search → SearchAction) preserves the search query when going back via Esc
  - Patterns: Clone SearchActionState before match in handler to avoid borrow conflicts — same pattern as DialogKind clone
  - Gotchas: Existing tests for `search_confirm` needed updating since the behavior changed from direct navigation to two-step flow
  - Gotchas: `enter_edit_mode()` requires the preview to be updated first (call `update_preview()` before `enter_edit_mode()`) since it reads `preview_state.current_path`
  - Gotchas: `detect_file_type` needs to handle the directory case before checking binary — directories are never binary
  - Context: Action filtering is handled at both handler level (guards on key matches) and widget level (dynamic action list)
---
