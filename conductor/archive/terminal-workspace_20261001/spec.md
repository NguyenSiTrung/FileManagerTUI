# Spec: Terminal Workspace for Headless and Web Terminals

## Overview

Transform FileManagerTUI into a reliable IDE-like terminal workspace for
headless VMs, SSH/tmux sessions, and Jupyter/Kubeflow web terminals. Reuse Rust,
Ratatui, the filesystem tree, preview loaders, editor, and PTY infrastructure.
This is an architectural feature track, not a rewrite or a browser frontend.

Users must be able to retain multiple editable documents while browsing files,
running shell commands, searching a project, and using optional Git/LSP features.
Single-binary deployment and useful behavior without optional executables remain
central constraints.

## Approved Decisions

- Priority: **high**; no active Conductor track dependencies.
- Include the core redesign, read-only Git indicators, diagnostics, and LSP.
- Language servers are installed/configured executables. No automatic downloads.
- Parallel work is allowed only for independent tasks with disjoint file ownership.
- **Verification is automated only. No manual-verification tasks or gates exist,
  including at phase boundaries and at the end of the track.** This overrides the
  manual-verification clause in `conductor/workflow.md` for this track only.
- Track creation does not authorize application implementation or remote publication.
- Time estimate is unspecified, not an inferred promise.

## Current Baseline

The review was against commit `cf22b36`. Notable source anchors:

| Area | Current implementation |
|---|---|
| Application-wide edit mode | `src/app.rs` `AppMode` and `editor_state`; `src/handler.rs` mode dispatch |
| Fixed explorer width | `src/ui.rs` horizontal 40/60 split |
| Creation/rename safety | `src/fs/operations.rs` `create_file` and `rename`; `src/handler.rs` input dialogs |
| Saves/line endings | `src/editor.rs` `new`, `from_file`, and `save` |
| Paste/terminal input | `src/event.rs`, `src/tui.rs`, `src/handler.rs` |
| Clipboard fallback | `src/app.rs` copy helpers; `src/main.rs` `ShowCopyableText` |
| Preview/loading costs | `src/app.rs` `update_preview`, `open_search`, and `build_path_index` |
| Repeated highlighting | `src/components/editor.rs` viewport rendering |
| Terminal emulation | `src/terminal/emulator.rs` mode and status-report handling |
| Config/help drift | `src/config.rs`, `src/components/help.rs`, `src/components/settings.rs`, `README.md` |

`cargo test --locked --offline` could not resolve `crossterm` in this environment.
No passing application-test baseline has been established here. Implementation
must obtain dependencies and establish that baseline before feature changes.

The missing local Beads workspace was recovered non-destructively with
`bd --sandbox bootstrap --yes --json` from Git origin's `refs/dolt/data`.
All 115 existing issues were recovered: 113 closed and two open archived
terminal-copy checkpoints. Those issues are not modified or dependencies of this
track. Beads 1.3.0 uses embedded Dolt here; no SQL server is required.

## Functional Requirements

### FR-1: Collision-Safe File Operations

- New-file creation must fail without modifying an existing destination.
- Rename must not silently replace an existing file or directory. Prefer a
  platform-native no-replace operation; an existence check alone is not a
  race-safe guarantee. If safety cannot be provided, refuse rather than overwrite.
- Keep root protection, delete confirmations, copy/move collision behavior,
  cancellation, and existing filesystem-operation undo working.
- Report the operation, affected path, and cause on permission, mount, or I/O errors.

### FR-2: Safe Saves and External Changes

- Track the loaded/saved revision and detect an externally changed, replaced,
  or deleted target before committing a save. Offer reload, save-as, or an
  explicitly confirmed overwrite; never silently discard either version.
- Write a same-directory temporary file, flush it, and safely replace the
  destination where supported. Retain the original on failure. Never fall back
  to truncating the destination to conceal replacement failures.
- Preserve LF/CRLF and trailing-newline policy on ordinary edits. Unsupported
  mixed endings or encodings require an explicit warning before normalization.
- Preserve applicable permissions. Do not replace a symlink with a regular file
  without an explicit policy; detect a changed symlink target.
- Treat read-only mounts, unsupported replacement, hard links, and concurrent
  writers explicitly. A pre-save fingerprint is not a universal filesystem
  compare-and-swap guarantee; document residual races and platform limits.
- Retain dirty buffers after all failed/cancelled saves. Dirty state reflects the
  saved undo revision, so undoing back to the saved revision clears the indicator.

### FR-3: Editing, Paste, and Clipboard

- Enable and handle bracketed paste, restoring terminal state on exit/panic.
- Insert multiline paste at the selection/cursor as one undoable transaction,
  without per-line auto-indent or interpreting pasted text as commands.
- Keep file-operation clipboard and text clipboard separate. Text selection
  copy uses native commands, OSC 52, or the browser-copy overlay, with honest
  success/fallback feedback. Browser clipboard reads are not assumed available.
- Support inline selection paste, repeated paste, trailing newlines, and empty text.
- Use explicit conversions between UTF-8 byte offsets, grapheme boundaries,
  tab expansion, and terminal display columns.
- Add horizontal scrolling and optional wrapping to preview and editor; maintain
  accurate cursor, selection, find-match, and mouse mapping under either mode.
- Preserve find/replace, indentation, undo/redo, and configurable large-file guards.

### FR-4: Documents Independent of Focus

- Maintain a document store keyed by stable identity/path, not tree row indexes.
- Every document retains its buffer, cursor, viewport, undo history, dirty/saved
  revisions, and external-change state across focus and tab changes.
- Separate panel focus, active document, transient preview, and modal overlay.
  Browsing and terminal input must work while an editable document remains open.
- Single-click/select previews; double-click or Enter retains a document.
  Editing a temporary preview pins it before another selection can replace it.
- Reopening a file activates its existing document instead of duplicating buffers.
- Provide tabs, open-document navigation, and reveal-in-explorer.
- Close/quit presents Save/Discard/Cancel for dirty documents; cancel and failed
  save retain all affected buffers.
- Reflect rename/delete/external-change events without writing an obsolete path
  or dropping an open document silently.
- Preserve binary, directory, notebook, large-file, and read-only S3 previews.
  S3 editing stays unavailable; notebook rendering is not kernel execution.

### FR-5: Adaptive Workspace Layout

- Use resizable explorer and terminal panes rather than a fixed 40/60 split.
- Hide/show explorer and terminal, maximize editor/terminal, and restore the
  previous layout without restarting the shell.
- Provide readable layouts at 120x40, 80x24, and 60x20. At smaller sizes show a
  bounded compact state; dialogs and essential controls must not render off-screen.
- Provide breadcrumbs, document tabs, dirty markers, and editor-aware status
  information: path, line/column, language, encoding, line ending, and read-only state.
- Keep borders/chrome restrained. Plain icons and no-mouse operation are supported.
- Honor preview enable/disable, including `--no-preview`; separate disabling
  automatic preview from opening an editable document.

### FR-6: Commands and Browser-Terminal Profiles

- Introduce one command registry for stable command identifiers, availability,
  dispatch metadata, help, and keybinding descriptions.
- Support configurable keybindings plus standard and explicit web-terminal
  profiles; do not claim reliable automatic browser detection.
- Save, Quick Open, project search, document switching, focus switching, and pane
  toggling must have routes that do not exclusively require browser-reserved keys.
- Provide a keyboard/mouse command menu accessible without closing dirty documents.
- Route text/find/modal input before inappropriate normal-mode commands.
- Reserve only explicit configured workspace shortcuts in terminal focus;
  pass ordinary shell editing keys, Tab, Esc, and Ctrl+C through appropriately.
- Validate conflicting/unknown bindings. Apply live settings deliberately and
  reconcile README/help/config claims, including wrapping and large-file controls.

### FR-7: Navigation, Search, and Background Work

- Enter expands a directory or opens a text document. Quick Open opens the selected
  document directly; secondary actions remain available without an obligatory menu.
- Index filenames asynchronously and incrementally; show loading, incomplete,
  excluded, or capped results instead of falsely implying a complete search.
- Provide project content search with literal matching, configurable exclusions,
  cancellation, and navigable file/line/column results.
- Keep a native Rust implementation; optional installed search tools must not
  become a prerequisite for the basic application.
- Preserve selection/multi-selection by path through refresh, sort, and pagination.
- Move blocking file reads, directory enumeration, and highlighting work off the
  render/input path; ignore late results using request/document generation IDs.
- Cache syntax states/highlighted lines; invalidate affected ranges on edits/themes.
- Coalesce redraws/output bursts and bound queues/results so background activity
  cannot cause unbounded memory growth.
- Support event-based watching plus configurable polling for mounted storage.
  Preserve user watcher preferences and handle disk changes without overwriting buffers.
- Apply size/output limits to notebook previews as well as ordinary text files.

### FR-8: Sessions and Private Recovery

- Restore workspace root, pane layout, open-document paths, active document,
  cursors/viewports, and recent files.
- Persist versioned, workspace-scoped recovery snapshots for dirty documents.
  Offer restore/discard on restart, without silently overwriting newer disk files.
- Use private user state storage, restrictive permissions where supported,
  bounded retention, explicit disable/clear controls, and safe handling of corrupt
  records or missing/unwritable state directories.
- Do not store copied text in the shared fixed `/tmp/.fm_clipboard` path.
- Do not resume arbitrary processes, executable trust, or secrets from session state.
  Restored configuration does not grant project LSP execution permission.

### FR-9: Read-Only Git Indicators

- Use an optional installed Git executable for asynchronous read-only status.
- Display branch/detached/unborn state, modified/staged/untracked/conflicted
  decorations, and aggregated directory indicators.
- Parse machine-readable NUL-delimited output so spaces, newlines, and Unicode
  filenames are handled; do not interpolate paths into shell commands.
- Bound/cancel requests, reject stale repository results, and degrade gracefully
  outside repositories, in read-only mounts, and when Git is unavailable.
- No stage, commit, push, checkout, reset, stash, or other Git write actions.

### FR-10: Installed-Server LSP and Diagnostics

- Configure language-to-server executable/argument lists in global/local TOML.
  Spawn stdio processes without shell interpolation. Do not download servers.
- Global user-configured commands are trusted configuration; project-provided
  executable changes require explicit interactive trust approval or an explicit
  user CLI/config grant. Headless startup must default to no execution, not block.
- Make LSP optional: unsupported files, missing servers, startup/crash/timeout
  errors, and unsupported capabilities cannot block normal editing.
- Implement bounded JSON-RPC framing, request IDs, initialize/initialized,
  negotiated capabilities, shutdown/exit, cancellation, and process cleanup.
- Implement didOpen/didChange/didSave/didClose with monotonically increasing
  document versions. Apply the negotiated synchronization kind.
- Map negotiated UTF-8/UTF-16/UTF-32 positions correctly to buffer offsets and
  terminal cells; reject invalid ranges instead of slicing invalid UTF-8.
- Support completion (including advertised edit forms), hover, definition,
  references, document symbols, and published diagnostics.
- Apply completion edits as one undoable operation; do not execute server-supplied
  commands automatically. Sanitize/limit rendered server text.
- Provide diagnostics severity/counts, file/line/column results, navigation, and
  inline/gutter indications without relying on color alone.
- Handle stale versions, server restart, unsaved buffers, close/rename/delete,
  multi-document sessions, and oversized/malformed responses.
- Unsupported server-driven workspace edits and execute-command requests must
  receive an explicit unsupported response, never silently mutate files.

### FR-11: Embedded-Terminal Correctness

- Evaluate a mature Rust emulator behind the existing emulator-facing API before
  extending the custom parser; select using automated compatibility fixtures.
- Cover alternate screens, terminal modes, cursor visibility, status responses,
  bracketed paste, Unicode width, resize, scrollback, and process exit/restart.
- Preserve shell autocomplete, interrupt delivery, Esc, modified input, and
  configured workspace escape shortcuts.
- PTY output must not reinterpret terminal OSC content as permission to execute
  commands, copy secrets, or mutate workspace files.
- Hiding the pane does not terminate the shell; quitting performs bounded cleanup.

## Non-Functional Requirements

- Keep Rust 2021, Ratatui, Crossterm, Tokio, and single-binary deployment.
- Git and language servers are optional executables; no required GUI/display,
  network service, external search tool, or package manager for basic use.
- Dev-only browser/PTY harness dependencies do not become runtime requirements.
- Retain Linux/static-musl, macOS, and Windows build compatibility. Platform-specific
  behavior is guarded and tested; unsupported guarantees fail safely.
- Every background subsystem has bounded input/output, cancellation, and error paths.
- Test injected slow/failed I/O rather than depend on brittle wall-clock assertions.
- Maintain the existing binary-size target of less than 10 MB; record artifact size
  and explicitly report a regression instead of hiding it.
- Target at least 80% coverage of new core logic, with automated coverage evidence.
- No unrelated refactor, CI-trigger change, Git identity override, or remote push.

## Automated Acceptance Criteria

| ID | Required automated evidence |
|---|---|
| AC-1 | At 80x24, open two files, edit one, browse, run a shell command, paste YAML, return, save, and verify exact bytes and retained state. |
| AC-2 | Existing destinations survive create/rename collisions; original files survive injected save failures, read-only cases, and external-change conflicts. |
| AC-3 | Dirty close/quit cancellation, saved-revision undo, rename/delete, recovery, and corrupt session records retain predictable state. |
| AC-4 | Unicode/tab/wide-character cursor and selection mapping, wrapped/horizontal views, and multiline paste/undo have deterministic tests. |
| AC-5 | Ratatui TestBackend fixtures cover 120x40, 80x24, 60x20, and tiny rectangles without out-of-bounds rendering or inaccessible controls. |
| AC-6 | Injected delayed/cancelled requests prove filename/content search, loads, highlighting, watcher polling, and Git updates cannot replace newer state. |
| AC-7 | Temporary Git repositories cover clean/dirty/untracked/staged/conflicted/unborn states; fake/missing Git paths cover fallback without Git writes. |
| AC-8 | Fake stdio LSP servers cover framing, position encodings, sync versions, completion/hover/navigation/symbols/diagnostics, faults, and process cleanup. |
| AC-9 | Automated PTY tests cover input forwarding, resize, bracketed paste, alternate screen, emulator replies, hide/show, shell exit, and safe shutdown. |
| AC-10 | A controlled local browser-terminal fixture tests web-profile commands, paste, mouse copy fallback, resize, and the full workspace workflow without saved auth. |
| AC-11 | Missing Git/LSP, no mouse/icons/watcher/terminal, binary/large/notebook previews, and read-only S3 have automated regression coverage. |
| AC-12 | `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`, automated browser/PTY suites, coverage, and configured cross-platform build checks pass. |

AC-10 exercises a controlled xterm.js-style terminal, not an untested live
Kubeflow/Jupyter deployment. Existing deployment access is not required and
production compatibility must not be claimed from fixture evidence alone.

No acceptance criterion requires a human to perform verification. Failures or
unavailable automated environments are reported as failures/blockers, not waived
by adding manual-verification tasks.

## Out of Scope

- Browser frontend or remotely exposed server for the application.
- Notebook kernels, notebook-cell editing/execution, debugger, or extension marketplace.
- Git write operations, S3 writes/editing, and arbitrary server-driven workspace edits.
- Automatic language-server downloads or automatic execution of untrusted project commands.
- Multi-cursor editing and multiple editor groups.
- Replacing all modules, changing release-trigger policy, committing/pushing without authority.
