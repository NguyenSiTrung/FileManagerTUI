# Track Learnings: terminal-workspace_20261001

Patterns, gotchas, and context discovered during planning and implementation.

## Codebase Patterns (Inherited)

Source of truth: [../../patterns.md](../../patterns.md). The consolidated
patterns supersede older archived assumptions; check source before reusing one.

- Keep tree data, widget rendering, event dispatch, and application orchestration
  separate. Reuse the existing widgets and filesystem model.
- Reuse loaded `SyntaxSet`/theme state. Newline-aware syntect syntax sets require
  newline-terminated input even when editor buffers store lines without endings.
- Use `spawn_blocking` for blocking I/O and bridge results through typed events.
  Clone owned configuration before spawning; do not share mutable App references.
- Preserve canonical sorting after directory loads and snapshot-based pagination.
- Optional configuration fields participate in merge, CLI overrides, settings
  registration, live side effects, and default accessors.
- Mouse handlers use render-derived rectangles and border/gutter offsets;
  automated tests must seed or render those rectangles.
- Terminal input must not fall through to tree commands. Shell Tab remains
  autocomplete; modified-key comparisons need explicit modifier handling.
- Native clipboard operations run asynchronously; OSC 52 may be unavailable, so
  keep the browser-selection overlay without a false copy-success promise.
- New preview/editor behavior must retain binary/large-file guards and read-only
  S3 semantics. S3 runtime state is not ordinary local filesystem configuration.

## Patterns Intentionally Changed by This Track

- Global `AppMode::Edit` and one editor buffer are replaced by independent
  document/focus/overlay state.
- Index-based multi-selection clearing on flatten is replaced by path-based
  selection preservation.
- Fixed 40/60 layout becomes adaptive/resizable.
- Main-thread filename indexing and render-time file loading move to background work.
- Pausing all watcher activity during editing is replaced by document-aware changes.
- Unconditional terminal Esc/focus interception is replaced by configured
  workspace shortcuts and ordinary key forwarding.
- The historical statement that full preview has no internal size guard is stale:
  `load_highlighted_content` now includes a defensive size check.
- The old claim that Ctrl+W toggles wrapping is not a current verified feature;
  the settings/help/handler contract must be reconciled.

## Relevant Archived Tracks

- `preview-edit_20260228`: editor state, undo/redo, find/replace.
- `focus-nav-remap_20260228`: panel input conflicts and modifier handling.
- `terminal-panel_20260228`, `terminal-mouse-copy_20260302`: PTY and clipboard.
- `large-dir-perf_20260228`, `large-dir-robust_20260301`: paginated snapshots.
- `preview-hardening_20260304`: preview limits and prior regression risks.
- `help-settings_20260302`, `config-polish_20260228`: settings merge/live apply.
- `s3-browse_20260310`: virtual tree and read-only backend behavior.

## Planning Decisions: 2026-10-01

- User approved Git indicators, diagnostics, and installed-command LSP in scope.
- User approved high priority and parallel work only with disjoint ownership.
- User explicitly requires automated-only verification, including the end of
  the track. Do not regenerate manual-verification tasks from the global workflow.
- The approved specification and plan do not authorize application implementation.
- No Git commit, remote push, or Dolt remote sync is performed during creation.

## Environment Findings: 2026-10-01

- Fresh checkout omitted Git-ignored `.beads/`; remote `refs/dolt/data` existed.
  `bd --sandbox bootstrap --yes --json` restored 115 existing issues without
  reinitialization or remote writes. Directory permissions were restricted to 0700.
- Beads 1.3.0 uses embedded Dolt; old skill references requiring a SQL server do
  not apply to this workspace.
- Two open archived terminal-copy checkpoint issues were preserved unchanged.
- Offline Cargo tests could not resolve `crossterm`. Implementation must establish
  a real baseline with dependencies available; no passing tests are inferred.

---

Implementation discoveries are appended below with task, affected files,
verification evidence, and reusable patterns.
