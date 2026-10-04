# Terminal Workspace Implementation Plan

Last revised: 2026-10-01 (shared viewports, revisions, pane integration and bounded transport seams).

> **For agentic workers:** Use the `executing-plans` skill for sequential tasks
> and `subagent-driven-development` only for the explicitly parallel phases.
> Read this plan together with its specification. Checkbox tasks are tracked in
> Beads; a checkpoint is an automated task, never a request for manual verification.

**Goal:** Provide a safe, responsive IDE-like terminal workspace for headless
and web-terminal users, including optional read-only Git, installed-server LSP,
and diagnostics.

**Architecture:** Keep App as the coordinator while extracting document/focus,
commands, layout, background work, session, Git, and LSP into focused modules.
Typed events carry generation/version-tagged results back to the coordinator.
Reuse tree, preview, editor, configuration, and PTY primitives instead of a rewrite.

**Tech Stack:** Rust 2021, Ratatui, Crossterm, Tokio, Syntect, existing filesystem/
PTY dependencies; optional installed Git and language-server executables.
Browser-test tooling is development-only.

**Spec:** [spec.md](spec.md).

## Global Constraints

- Priority is high; track status remains new until implementation is requested.
- Verification is automated only. No manual-verification tasks, phase sign-offs,
  or manual completion gates are permitted for this track.
- Keep a single binary; basic browsing/editing needs no optional executable,
  display, package manager, or network service.
- Git is read-only. LSP servers are installed executable/argument lists, with no
  shell interpolation or automatic downloads.
- Untrusted project LSP commands need explicit trust; restored sessions do not
  grant it. Unsupported server workspace edits/commands must not execute.
- Preserve Linux/static-musl, macOS, and Windows build compatibility; platform
  limitations fail safely. Keep and measure the less-than-10-MB binary target.
- Target at least 80% coverage for new core logic and report actual coverage.
- Preserve read-only S3, notebook/binary/large-file previews, optional mouse/icons,
  and existing file-operation confirmations and undo.
- All background queues, results, timeouts, and recovery retention are bounded.
- No Git identity override, unrelated refactor, CI-trigger change, or remote push.
- Use the active repository Git-authority policy. Planning is not commit/push
  authorization; commit completed implementation tasks only when authorized.

## Execution and Test Protocol

1. Read the task's Beads record, specification requirements, and named source.
2. Add its regression tests, run the listed filter, and capture a genuine red
   result. A dependency-resolution failure is not a regression-test red result.
3. Implement only the named boundary and run the focused tests to green.
4. Run `cargo test`, `cargo clippy -- -D warnings`, and `cargo fmt --check`
   at each completed code task/checkpoint; fix failures before marking complete.
5. Update the task and append evidence to `learnings.md`. The coordinator owns
   track metadata/learnings, module registration, and shared Cargo lock updates
   during parallel work.
6. Use `cargo test <filter> -- --nocapture` for the unit examples below. Examples
   belong inside the named module's `#[cfg(test)]` block unless otherwise stated.
   Tests may use `tempfile::TempDir` and injected collaborators.
7. The two explicitly parallel phases have complete `files`/`depends` ownership.
   Do not edit unlisted shared files concurrently. All other phases are sequential.
8. Each phase depends on the previous phase. Closing a phase requires all child
   tasks, including its automated checkpoint, to have passed.
9. Automated prompts for dirty buffers or LSP trust are product behavior, not
   manual verification. Tests exercise both accept and cancel/refuse paths.
10. Production tool dependencies must be absent from default test prerequisites:
    use temporary Git repositories and fake LSP/PTY peers. Optional real-server
    smoke tests are additional evidence, not substitutes for deterministic tests.

## File and Interface Map

| Boundary | Files and responsibility |
|---|---|
| Filesystem safety | `src/fs/operations.rs`, new `src/fs/save.rs`: no-replace operations and safe save results |
| Text coordinates | New `src/text.rs`: byte/grapheme/display/LSP conversion primitives |
| Document workspace | New `src/workspace/{mod,documents,focus,layout}.rs`: document lifetime, focus, and rectangles |
| Commands | New `src/commands.rs`, `src/keymap.rs`: command IDs, availability, bindings, profiles |
| Presentation | New `src/components/workspace_chrome.rs`; existing editor, preview, status, help widgets |
| Background work | New `src/background.rs`, `src/search.rs`, `src/highlighting.rs`: bounded versioned jobs |
| State persistence | New `src/session.rs`, `src/recovery.rs`: versioned private workspace state |
| Optional Git | New `src/git.rs`: machine-readable read-only Git state |
| Terminal | Existing `src/terminal/*`: emulator adapter, PTY input/output/lifecycle |
| Optional LSP | New `src/lsp/{mod,positions,transport,client,config,features}.rs`: protocol and language features |
| Diagnostics | New `src/diagnostics.rs`, `src/components/diagnostics.rs`: versioned state and navigable results |
| Automated TUI acceptance | New `scripts/test-terminal-workspace.py`, `tools/terminal-tests/*`: PTY and browser fixtures |
| Integration | Existing `src/{app,handler,ui,event,main,config,theme}.rs`, module registration, settings |

Planned APIs below are contracts, not assertions that these symbols already
exist. Add a module's declarations in the task that introduces it. Avoid broad
`pub` exposure solely to test: keep core tests in their owning module.

## Phase 1: Collision-Safe Operations and Safe Saves
<!-- execution: parallel -->

- [x] Task 1: Establish a reproducible application-test baseline
  <!-- files: Cargo.toml, Cargo.lock -->

  **Requirements:** Current Baseline, AC-12.
  **Produces:** recorded dependency/toolchain baseline; no inferred passing tests.
  - [x] Run `cargo test --locked`, `cargo clippy --locked -- -D warnings`, and
    `cargo fmt --check` with dependencies available. Record failures and stop
    feature edits until a runnable baseline exists.
  - [x] Record tool versions and baseline release-binary size. Do not change
    dependencies just to bypass a failing existing test.
  - [x] Establish coverage tooling as a development check; record baseline
    covered/new-core scope rather than pretend existing coverage is known.

- [x] Task 2: Make creation and rename refuse destination collisions
  <!-- files: src/fs/operations.rs, Cargo.toml, Cargo.lock -->
  <!-- depends: task1 -->

  **Requirements:** FR-1, AC-2.
  **Consumes:** current operation return types.
  **Produces:** existing `create_file`/`rename` APIs with no silent overwrite.
  - [x] Add the regression before changing `File::create`:
    ```rust
    #[test]
    fn create_existing_file_preserves_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "keep: true\n").unwrap();
        assert!(create_file(&path).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "keep: true\n");
    }
    ```
  - [x] Add rename-to-existing-file/directory, same-path, symlink, and racing
    destination tests. Exercise the no-replace primitive rather than only a
    preflight existence check.
  - [x] Use exclusive creation; implement platform-safe no-replace rename or
    explicit refusal on unsupported platforms. Preserve operation undo semantics.
  - [x] Run `cargo test fs::operations`; expect all fixtures green with original
    source/destination bytes unchanged on rejected operations.

- [x] Task 3: Introduce revision-aware safe editor saves
  <!-- files: src/fs/save.rs, src/fs/mod.rs, src/editor.rs -->
  <!-- depends: task1 -->

  **Requirements:** FR-2, AC-2, AC-3.
  **Produces:** `FileRevision`, `SaveError`, and
  `save::save_document(path, bytes, expected_revision)`; EditorState tracks
  line endings, source revision, and saved undo revision.
  - [x] Add load/save round-trip and CRLF regression:
    ```rust
    #[test]
    fn save_preserves_crlf_and_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.txt");
        std::fs::write(&path, b"a\r\nb\r\n").unwrap();
        let mut editor = EditorState::from_file(&path).unwrap();
        editor.insert_char('x');
        editor.save().unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"xa\r\nb\r\n");
    }
    ```
  - [x] Inject failures at write, flush, replacement, and revision validation;
    assert original bytes survive and the editor remains dirty.
  - [x] Test external edit/delete/replacement, symlink retargeting, permissions,
    hard-link policy, mixed endings, and undo-to-saved-revision.
  - [x] Implement same-directory exclusive temporary files and safe replacement,
    preserve applicable permissions, and return explicit conflict/unsupported
    results. Cleanup only temporary files created by this save attempt.
  - [x] Run `cargo test fs::save` and `cargo test editor`; do not assume chmod
    tests fail under root, use injected I/O failure tests as the mandatory evidence.

- [x] Task 4: Integrate operation/save outcomes into non-destructive dialogs
  <!-- files: src/app.rs, src/handler.rs, src/components/dialog.rs, src/error.rs -->
  <!-- depends: task2, task3 -->

  **Requirements:** FR-1, FR-2, AC-2, AC-3.
  **Consumes:** collision errors and safe-save conflicts.
  **Produces:** Reload/Save As/explicit Overwrite/Cancel actions retaining buffers.
  - [x] Add handler regressions for cancelled conflict and failed SaveConfirm;
    use the existing `save_confirm_yes_stays_in_dialog_on_save_error` fixture pattern.
    ```rust
    // Required invariant in each conflict/cancel handler test.
    assert!(app.editor_state.as_ref().unwrap().modified);
    assert!(!app.should_quit);
    ```
  - [x] Add Save As destination validation without overwrite-by-default. Surface
    operation/path/cause and preserve the caller's focus and dirty buffer.
  - [x] Restore watcher preferences instead of unconditionally enabling watching
    after editor exit. Record the policy for document-aware changes in Phase 6.
  - [x] Run `cargo test handler` and `cargo test app`.

- [x] Task 5: Automated checkpoint for file safety
  <!-- files: -->
  <!-- depends: task4 -->

  **Requirements:** AC-2, AC-3, AC-12.
  - [x] Run the full quality gates; verify collision, fault injection, line-ending,
    permission, and external-revision tests passed on supported test platforms.
  - [x] Record unsupported platform guarantees explicitly; no manual gate.

## Phase 2: Text Coordinates, Paste, Clipboard, and Viewports

- [x] Task 1: Define byte, grapheme, tab, and display-column conversions

  **Files:** create `src/text.rs`; modify `src/main.rs`, `src/editor.rs`,
  `src/handler.rs`, `src/components/editor.rs`, `Cargo.toml`, `Cargo.lock`.
  **Requirements:** FR-3, AC-4.
  **Produces:** `TextPosition { line, byte }`; conversion helpers
  `byte_to_display_col(text, byte, tab_width)` and
  `display_col_to_byte(text, col, tab_width)`; grapheme-boundary navigation.
  - [x] Add ASCII/combining/CJK/emoji/tab tests before converting editor positions:
    ```rust
    #[test]
    fn tab_and_wide_text_map_back_to_byte_boundaries() {
        let text = "\t中a";
        assert_eq!(byte_to_display_col(text, 1, 4), 4);
        assert_eq!(byte_to_display_col(text, 4, 4), 6);
        assert_eq!(display_col_to_byte(text, 6, 4), 4);
    }
    ```
  - [x] Make every buffer mutation/selection/search conversion use byte-safe
    ranges; distinguish terminal cell positions from document positions.
  - [x] Update cursor, end-of-line, joining, indent, and mouse tests deliberately,
    including grapheme selections and empty/short lines.
  - [x] Run `cargo test text` and `cargo test editor`.

- [x] Task 2: Handle bracketed paste as one editor transaction

  **Files:** modify `src/tui.rs`, `src/event.rs`, `src/main.rs`,
  `src/handler.rs`, `src/editor.rs`.
  **Requirements:** FR-3, AC-4, AC-9.
  **Produces:** typed paste event and `EditorState::insert_text(&str)` using
  one compound undo entry without normal-key dispatch or auto-indent.
  - [x] Add the exact-byte/undo regression:
    ```rust
    #[test]
    fn multiline_paste_is_one_undoable_insert() {
        let mut e = EditorState::new("", "config.yaml".into());
        e.insert_text("training:\n  lr: 0.001\n");
        assert_eq!(e.buffer.join("\n"), "training:\n  lr: 0.001\n");
        e.undo();
        assert_eq!(e.buffer.join("\n"), "");
    }
    ```
  - [x] Test selection replacement, CRLF input, Unicode, trailing/empty text,
    huge-paste bounds, and pasted `q`/escape-like text.
  - [x] Enable/disable bracketed paste in setup, restore, suspend/resume, panic,
    and error cleanup. Keep terminal-paste routing separate from editor paste.
  - [x] Run `cargo test paste`; assert genuine paste events are not dropped.

- [x] Task 3: Unify text-copy fallbacks and correct internal inline paste

  **Files:** modify `src/editor.rs`, `src/app.rs`, `src/main.rs`,
  `src/handler.rs`, `src/ui.rs`.
  **Requirements:** FR-3, FR-8, AC-4, AC-10.
  **Produces:** reusable async text-copy outcome and a line/selection-aware
  internal text clipboard; file-operation clipboard remains separate.
  - [x] Add a selection-copy/paste test that inserts in the middle of a line,
    not below it; assert one undo restores the original.
    ```rust
    let mut e = EditorState::new("alphaomega", "text.txt".into());
    e.set_cursor_position(0, 5);
    e.insert_text("beta");
    assert_eq!(e.buffer[0], "alphabetaomega");
    e.undo();
    assert_eq!(e.buffer[0], "alphaomega");
    ```
  - [x] Test native success/failure, OSC 52 unavailable, bounded multiline
    browser overlay, disabled mouse capture, and focus restoration with fake
    clipboard collaborators. Never mutate the test runner's clipboard.
  - [x] Remove fixed `/tmp/.fm_clipboard` persistence; keep copied text in
    memory only. Do not promise browser clipboard-read access.
  - [x] Run `cargo test clipboard` and existing preview/terminal selection tests.

- [x] Task 4: Add horizontal scroll and optional wrapped text layout

  **Files:** modify `src/editor.rs`, `src/components/editor.rs`,
  `src/components/preview.rs`, `src/handler.rs`, `src/app.rs`, `src/text.rs`,
  `src/ui.rs`.
  **Requirements:** FR-3, FR-5, AC-4, AC-5.
  **Consumes:** text conversions; **produces:** shared visual-line/offset mapping.
  - [x] Add long-line cursor tracking, wrapping, selection, find-match, gutter,
    mouse mapping, and zero-width fixtures.
    ```rust
    // Long-line navigation must leave the cursor in the rendered code viewport.
    assert!(cursor_screen_col >= code_area.x);
    assert!(cursor_screen_col < code_area.x + code_area.width);
    ```
  - [x] Track horizontal offset independently of vertical offset; compute
    visual rows under wrap and clamp viewports when mode/size changes.
  - [x] Run `cargo test components::editor`, `cargo test components::preview`,
    and mouse-coordinate tests; assert rendered cells match selected text.

- [x] Task 5: Automated checkpoint for editing input

  **Requirements:** AC-4, AC-9, AC-12.
  - [x] Run full quality gates and paste/Unicode/wrap/clipboard fixtures.
  - [x] Confirm terminal state is restored by automated PTY teardown assertions.

## Phase 3: Independent Documents and Focus

- [x] Task 1: Introduce a stable document store

  **Files:** create `src/workspace/mod.rs`, `src/workspace/documents.rs`;
  modify `src/main.rs`, `src/editor.rs` (read-only content-revision accessor),
  `src/fs/save.rs` (bounded revision-consistent loading prerequisite).
  **Requirements:** FR-4, AC-3.
  **Produces:** `DocumentId`, `DocumentStore`, `Document` owning EditorState
  with `text() -> String`, `is_pinned() -> bool`, and
  `has_external_change() -> bool`; `OpenDisposition::{Preview,Pinned}`,
  and `open(path, disposition) -> Result<DocumentId, DocumentError>`,
  `activate(id)`, `get(id)`, `get_mut(id)`, `active_id()`, `pin(id)`.
  - [x] Test deduplication and document-local state before App integration:
    ```rust
    let first = docs.open(&a, OpenDisposition::Pinned).unwrap();
    let second = docs.open(&b, OpenDisposition::Pinned).unwrap();
    docs.get_mut(first).unwrap().editor.insert_text("unsaved");
    docs.activate(second).unwrap();
    docs.activate(first).unwrap();
    assert!(docs.get(first).unwrap().editor.modified);
    assert_eq!(docs.open(&a, OpenDisposition::Pinned).unwrap(), first);
    ```
  - [x] Pin on first edit, keep clean temporary-preview replacement bounded,
    and refuse to evict dirty documents.
  - [x] Include path alias/symlink policy and binary/large/S3 read-only guards.
  - [x] Run `cargo test workspace::documents`.

- [x] Task 2: Separate focus, active document, and modal overlays

  **Files:** create `src/workspace/focus.rs`; modify `src/workspace/mod.rs`,
  `src/workspace/documents.rs` (integration helpers),
  `src/app.rs`, `src/handler.rs`, `src/ui.rs`, `src/event.rs`.
  **Requirements:** FR-4, FR-6, AC-1, AC-3.
  **Consumes:** DocumentStore; **produces:** workspace state with explicit
  panel focus/overlay and document-targeted input routing.
  **Execution refinement:** Sequentially verify focus primitives, migrate App's
  editor ownership, then remove global Edit/focus adapters and complete modal
  targeting. An ownership-only intermediate stage does not complete this task.
  - [x] Add a regression that edits A, focuses tree, selects B, focuses terminal,
    then reactivates A without losing unsaved bytes.
    ```rust
    assert_eq!(app.workspace.documents.get(a_id).unwrap().editor.buffer[0],
               "unsaved");
    assert!(!app.should_quit);
    ```
  - [x] Route modal, find, editor, tree, and terminal input by context; do not
    create a second global Edit mode disguised as a document flag.
  - [x] Replace the single `editor_state` ownership incrementally and update
    legacy tests to the new explicit behavior.
  - [x] Run `cargo test workspace`, `cargo test handler`, and `cargo test app`.

- [x] Task 3: Add tabs, previews, direct opening, and open-document navigation

  **Files:** create `src/components/document_tabs.rs`; modify
  `src/components/mod.rs`, `src/ui.rs`, `src/handler.rs`, `src/app.rs`,
  `src/components/search_action.rs`.
  **Requirements:** FR-4, FR-7, AC-1, AC-5.
  **Produces:** tab widget consuming document summaries, direct tree/Quick Open
  open actions, pin/activate/reveal/open-document-list commands.
  - [x] Test Enter on directory versus text file, single/double click,
    edit-to-pin, tab overflow, duplicate basenames, and secondary search actions.
    ```rust
    assert_eq!(docs.active_id(), Some(opened_id));
    assert!(docs.get(opened_id).unwrap().is_pinned());
    ```
  - [x] Keep temporary preview and pinned editor content visually distinct;
    selecting another tree row cannot rename the active editor title.
  - [x] Run `cargo test document_tabs`, `cargo test search`, and routing tests.

- [x] Task 4: Make close, quit, rename, and delete document-safe

  **Files:** modify `src/workspace/documents.rs`, `src/app.rs`, `src/handler.rs`,
  `src/components/dialog.rs`, `src/event.rs`; coordinator-only prerequisite
  `src/fs/save.rs` (read-only known-rename revision comparison).
  **Requirements:** FR-2, FR-4, AC-3.
  **Produces:** document-scoped close/save conflict workflows and disk-change states.
  - [x] Test multiple dirty documents with Save/Discard/Cancel, including a
    failed save in the middle of a quit operation.
    ```rust
    assert!(!app.should_quit);
    assert!(app.workspace.documents.get(cancelled_id).is_some());
    ```
  - [x] Update owned renames by document identity; surface deleted/changed
    paths and never save to an obsolete name without an explicit decision.
  - [x] Keep normal text `q` and Ctrl+C from becoming quit requests in editor/
    shell contexts; retain quit as an explicit command.
  - [x] Run `cargo test document_lifecycle` and full handler tests.

- [x] Task 5: Automated checkpoint for document workspace

  **Requirements:** AC-1, AC-3, AC-11, AC-12.
  - [x] Run quality gates plus multi-document/focus/read-only preview regressions.
  - [x] Assert every edited document survives switching and cancelled operations.

## Phase 4: Commands, Keymaps, and Browser Profiles

- [x] Task 1: Introduce command registry and context-aware command menu

  **Files:** create `src/commands.rs`, `src/components/command_menu.rs`;
  modify `src/main.rs`, `src/components/mod.rs`, `src/app.rs`, `src/handler.rs`,
  `src/ui.rs` (menu rendering), `src/workspace/focus.rs` (modal integration).
  **Requirements:** FR-6, AC-1, AC-10.
  **Produces:** stable `CommandId`, command descriptions/availability, and
  `dispatch_command(app, id)`; menu actions target document/focus rather than rows.
  - [x] Register save/save-as/close/quit, opening/search, focus, pane toggles,
    wrapping, and recovery controls with one metadata source.
  - [x] Test unavailable commands in read-only/binary/S3 contexts and menu use
    while a dirty document remains open:
    ```rust
    assert!(commands.available("document.save", &context));
    assert!(!commands.available("document.save", &s3_context));
    ```
  - [x] Render bounded menus and restore prior focus when dismissed.
  - [x] Run `cargo test commands` and `cargo test command_menu`.

- [x] Task 2: Add configurable standard/web keymaps without shell conflicts

  **Files:** create `src/keymap.rs`; modify `src/config.rs`, `src/main.rs`,
  `src/handler.rs`, `src/app.rs`, `src/commands.rs` (menu-entry command ID and
  keymap dispatch integration).
  **Requirements:** FR-6, AC-9, AC-10.
  **Produces:** `KeymapProfile::{Standard,Web}`, parsed key sequences, conflict
  validation, and context-scoped command resolution.
  - [x] Add tests for profile overrides, unknown commands, duplicate/conflicting
    bindings, legacy modifier encodings, and raw shell/editor input.
    ```rust
    assert!(keymap.resolve(FocusContext::Terminal, esc).is_none());
    assert!(keymap.resolve(FocusContext::Terminal, tab).is_none());
    ```
  - [x] Assign/test an explicit workspace command prefix; leave ordinary terminal
    keys forwarded. Provide menu/mouse routes when a browser consumes shortcuts.
  - [x] Expose explicit web profile in CLI/TOML; do not infer it from an SSH/container
    environment. Add tests excluding Ctrl+P/T/S/R from required web workflows.
  - [x] Run `cargo test keymap` and input-dispatch tests.

- [x] Task 3: Generate help and reconcile live settings with actual behavior

  **Files:** modify `src/components/help.rs`, `src/components/settings.rs`,
  `src/config.rs`, `src/handler.rs`, `src/ui.rs`, `README.md`, `src/app.rs`
  (live runtime effects), `src/commands.rs` (binding metadata/legacy aliases),
  `src/terminal/mod.rs`, `src/terminal/emulator.rs` (scrollback limit),
  `src/components/command_menu.rs` (active binding title); coordinator-only
  prerequisite `src/fs/save.rs` (bounded shared safe publication).
  **Requirements:** FR-5, FR-6, AC-11.
  **Consumes:** registry/keymap metadata; **produces:** consistent help/settings.
  - [x] Test that help reflects active bindings and disabled features rather
    than static contradictory shortcuts.
    ```rust
    assert_eq!(help_binding("document.save", &web_keymap),
               web_keymap.binding_label("document.save"));
    ```
  - [x] Wire preview disable, wrapping, view-mode cycling, watcher preferences,
    and terminal scrollback settings to actual behavior; reconcile legacy aliases.
  - [x] Test config merge/CLI/live application and partial TOML upgrades.
  - [x] Run `cargo test config`, `cargo test help`, and `cargo test settings`.

- [x] Task 4: Automated checkpoint for command accessibility

  **Requirements:** AC-1, AC-9, AC-10, AC-12.
  - [x] Run full gates; verify every essential action has a tested web-profile
    route and ordinary editor/shell input stays context-correct.

## Phase 5: Adaptive Layout and Workspace Presentation
<!-- execution: parallel -->

- [x] Task 1: Implement pure adaptive layout and resize state
  <!-- files: src/workspace/layout.rs -->

  **Requirements:** FR-5, AC-5.
  **Produces:** `LayoutState`, `WorkspaceRects`, and
  `compute_layout(area: Rect, state: &LayoutState) -> WorkspaceRects`,
  `WorkspaceRects::all_inside(Rect) -> bool`, and pane-toggle/restore methods.
  - [x] Test visible/hidden/maximized panes and geometry with no terminal I/O:
    ```rust
    for (w, h) in [(120, 40), (80, 24), (60, 20), (1, 1)] {
        let area = Rect::new(0, 0, w, h);
        let rects = compute_layout(area, &LayoutState::default());
        assert!(rects.all_inside(area));
    }
    ```
  - [x] Use bounded column widths/minimums, compact fallback, and restore prior
    sizes after maximize/unmaximize; drag/keyboard resizing updates only layout state.
  - [x] Run `cargo test workspace::layout`.

- [x] Task 2: Implement breadcrumbs and editor-aware workspace chrome
  <!-- files: src/components/workspace_chrome.rs, src/components/document_tabs.rs, src/components/status_bar.rs -->

  **Requirements:** FR-4, FR-5, AC-5.
  **Consumes:** Phase 3 document summaries; **produces:** widgets with plain-icon
  fallback, dirty/read-only markers, overflow handling, and document status.
  - [x] Add TestBackend snapshots of duplicate basenames, long Unicode paths,
    cursor/encoding/ending/language fields, inactive/active tabs, and narrow widths.
    ```rust
    assert!(screen_text.contains("config.yaml"));
    assert!(screen_text.contains("LF"));
    assert!(!screen_text.contains("e:edit")); // Not a view-mode hint in an editor.
    ```
  - [x] Keep backgrounds/contrast theme-aware and prioritize context over long
    shortcut hints when space is scarce.
  - [x] Run `cargo test workspace_chrome`, `cargo test document_tabs`, and
    `cargo test status_bar`.

- [x] Task 3: Integrate layout, pane controls, and hit testing
  <!-- files: src/workspace/mod.rs, src/components/mod.rs, src/ui.rs, src/handler.rs, src/app.rs, src/config.rs, src/theme.rs, src/components/settings.rs, src/commands.rs, src/keymap.rs, src/terminal/mod.rs, src/components/terminal.rs, src/main.rs, src/components/document_tabs.rs, src/components/workspace_chrome.rs, src/components/status_bar.rs, src/components/command_menu.rs, src/components/help.rs -->
  <!-- depends: task1, task2 -->

  **Requirements:** FR-5, AC-5, AC-9.
  **Consumes:** WorkspaceRects/chrome; **produces:** layout-controlled render and
  mouse mapping, pane controls, and persistent layout settings.
  - [x] Add application TestBackend resize/maximize/hide/show/drag fixtures.
    ```rust
    let before = app.terminal_state.pty.as_ref().unwrap().is_alive();
    app.workspace.layout.toggle_terminal();
    assert_eq!(app.terminal_state.pty.as_ref().unwrap().is_alive(), before);
    ```
  - [x] Register new modules, update preview/editor usable areas, clamp dialogs,
    and resize PTY/emulator together after layout changes.
  - [x] Preserve no-preview/no-terminal behavior and keyboard-only accessibility.
  - [x] Run layout/widget/mouse tests and PTY resize tests.

- [x] Task 4: Automated checkpoint for adaptive layout
  <!-- files: -->
  <!-- depends: task3 -->

  **Requirements:** AC-5, AC-9, AC-11, AC-12.
  - [x] Run all gates and the complete geometry matrix. Compare meaningful
    screen assertions rather than requiring pixel-identical fonts.

## Phase 6: Background Work, Search, and Mounted Storage

- [x] Task 1: Add bounded generation-aware background scheduling

  **Files:** create `src/background.rs`; modify `src/main.rs`, `src/event.rs`,
  `src/app.rs`; necessary transport/test adapters in `src/handler.rs`,
  `src/commands.rs`, `src/components/command_menu.rs`, `src/ui.rs`,
  `src/fs/watcher.rs`, `src/terminal/pty.rs` (Ruling 16); focused
  `src/background/app_jobs.rs`, `src/fs/tree.rs`, `src/preview_content.rs`
  (existing shallow-summary seam only), `src/s3/backend.rs` (Ruling 17),
  `src/fs/operations.rs` (Ruling 18), `src/workspace/focus.rs` (Ruling 19).
  **Requirements:** FR-7, AC-6.
  **Produces:** `RequestGeneration`, bounded job/result channels, request cancellation,
  stale-result rejection, and dirty redraw scheduling.
  - [x] Test delayed old results after a newer selection and output flooding:
    ```rust
    assert!(!scheduler.accepts(old_generation));
    assert!(scheduler.accepts(current_generation));
    assert!(queued_results <= configured_limit);
    ```
  - [x] Separate blocking workers from async orchestration; coalesce terminal
    output/redraws without dropping ordered user input or necessary completion events.
  - [x] Run `cargo test background`; use injected barriers, not arbitrary sleeps.
  - [x] Migrate all seven App producers (snapshots, summaries, S3, child counts,
    copy, paste) with a final scoped review PASS/APPROVE and an honestly narrowed
    consumer snapshot boundary.

- [x] Task 2: Move preview loading/highlighting out of rendering

  **Files:** create `src/highlighting.rs`; modify `src/main.rs`, `src/app.rs`,
  `src/ui.rs`, `src/preview_content.rs`, `src/components/editor.rs`,
  `src/background/app_jobs.rs` and `src/handler.rs` visibility/tests as needed;
  `src/event.rs` was not required.
  **Requirements:** FR-7, AC-6, AC-11.
  **Consumes:** background scheduler; **produces:** versioned preview loads and
  line/checkpoint syntax caches with bounded invalidation.
  - [x] Test slow old preview, theme-change invalidation, editing before a cached
    viewport, newline-aware parser state, and huge notebooks/embedded outputs.
    ```rust
    assert_eq!(preview.current_path, Some(new_path));
    assert!(render_io_count == 0);
    ```
  - [x] Render prepared state only. Do not rebuild syntax state from line zero
    on every frame; preserve existing large-file streaming limits.
  - [x] Run `cargo test highlighting` and preview/editor rendering tests.
  - [x] Narrowed boundary (review-accepted): syntect parser state is not `Send`,
    so editor syntax preparation is a bounded main-loop step (256 lines/step,
    20 000-line and 4 MiB per-document caps, 32 MiB aggregate with eviction),
    never in `render` or input handlers; requires a `Send` parser engine or a
    worker-local registry to go fully off-thread.

- [x] Task 3: Add incremental filename and project content search

  **Files:** create `src/search.rs`, `src/components/content_search.rs`; modify
  `src/main.rs`, `src/components/mod.rs`, `src/app.rs`, `src/event.rs`,
  `src/handler.rs`, `src/config.rs`, `src/components/search.rs`.
  **Requirements:** FR-7, AC-1, AC-6.
  **Produces:** `SearchQuery`, `SearchHit { path, line, byte, excerpt }`, bounded
  generation-tagged batches, configurable exclusion/size limits.
  - [x] Use a temporary project with matching text, binary/large files, excluded
    directories, unreadable entries, symlink loops, and Unicode names.
    ```rust
    assert_eq!(hits[0].path, root.join("config.yaml"));
    assert_eq!(hits[0].line, 1);
    assert!(!hits.iter().any(|h| h.path.starts_with(root.join(".venv"))));
    ```
  - [x] Implement literal content search and native filename indexing on workers;
    show cap/incomplete status and retain optional secondary actions.
  - [x] Test query replacement/cancel, navigate-to-hit, unsaved active document
    conflicts, and no installed external search command.
  - [x] Run `cargo test search` and result-navigation tests.
  - [x] Fix round 1 (review P2s): `SearchHit.column` in editor-buffer coordinates
    so CRLF hits navigate to the exact column; content queue bounded jointly with
    pending directories and every non-complete path marked incomplete; NUL scan
    over the whole bounded read; genuine FIFO traversal.
  - [ ] Follow-up `FileManagerTUI-5yf` (deferred by Ruling20): a single directory
    listing larger than `max_pending` re-lists and aborts on every continuation,
    returning zero results for a searchable tree. Fix with a resumable listing
    before Phase 6 closes (folded into the Task 5 checkpoint sweep).

- [x] Task 4: Preserve path selections and add document-aware watcher polling

  **Files:** modify `src/fs/tree.rs`, `src/fs/watcher.rs`, `src/app.rs`,
  `src/event.rs`, `src/config.rs`, `src/components/tree.rs`, `src/components/settings.rs`.
  **Requirements:** FR-4, FR-7, AC-3, AC-6.
  **Produces:** path-keyed tree/multi-selection, configurable event/polling watcher
  modes, external-document changes without global editor watch suppression.
  - [x] Test refresh/sort/pagination with multi-selected paths and external edits:
    ```rust
    assert!(tree.selected_paths().contains(&selected_path));
    assert!(documents.get(open_id).unwrap().has_external_change());
    ```
  - [x] Use an injected polling clock/backend for lost/coalesced events and
    deleted roots. Preserve user watcher mode and debounce/flood limits.
  - [x] Run `cargo test fs::tree`, `cargo test fs::watcher`, and document-change tests.
  - [x] Review-accepted deviation: the in-app `Ctrl+R` toggle and
    `watcher.auto_refresh` now gate **tree auto-refresh only** while change
    detection stays live for open documents (required so a focused editor still
    sees external writes); full disable remains `--no-watcher` /
    `watcher.enabled = false`. Help, settings and README state this explicitly.
  - [x] Fix rounds 1-2 (review P1/P2): self-write suppression is evidence-based,
    consuming the marker only on an exact match of length, mtime and an FNV-1a
    digest of the first 64 KiB of the published bytes, recomputed from memory at
    save time; mismatch reports the change as external. The tail beyond the
    window is deliberately not claimed verified.

- [x] Task 5: Automated checkpoint for responsiveness and search

  **Requirements:** AC-6, AC-11, AC-12.
  - [x] Run all gates and injected slow-I/O/output-flood tests. Prove rendering
    does not invoke loaders and old request generations cannot overwrite state.
  - [x] `FileManagerTUI-5yf` fixed and verified: `SearchCursor.pending` holds
    `DirListing { offset }` entries that resume from a saved position, and a
    paused listing consumes its next entry before the bound is re-checked, so
    reaching `max_pending` is a resumable pause rather than a cap. Review round 1
    rejected the first attempt for a livelock on non-empty subdirectories at the
    bound; the fixed loop guarantees progress and the confirmation review
    re-derived the red and probed all-excluded, symlink, unreadable, deep and
    no-deadline shapes with no stall and no lost hits.
  - [x] Accepted soft bound (confirmation P2, corrected): the retained set
    overshoots `max_pending` to roughly `2 * max_pending + children_per_dir`
    once nested directories are discovered after `visited` saturates. It is
    bounded by shape rather than tree size, `search_cursor_bytes` charges the
    real length, and the cursor stays far inside `job_bytes`; the doc comment,
    the report and the brittle `drive_content_to_completion` assertion were
    corrected to the documented slack instead of a hard bound the code does not
    promise.
  - [x] Phase 6 closure audit recorded in the report with explicit gaps
    (Phases 7-12, `.6.6` open, `run_index` single-listing cap accepted as an
    honest separate limitation).

- [x] Task 6: Bound legacy save validation and overwrite revision reads

  **Beads:** FileManagerTUI-cen.6.6 (discovered during Phase 3 Task 1).
  **Files:** modify `src/fs/save.rs`, `src/editor.rs`, `src/app.rs`.
  **Requirements:** FR-2, FR-7, bounded-I/O non-functional requirement.
  - [x] Bound both save-publication validation reads by the known source revision
    size, implemented early as Phase 4 Task 3's config-writer prerequisite.
    Explicit validation budgets use the same guarded publication implementation.
  - [x] Use known loaded revision sizes or configured byte limits to bound
    remaining explicit overwrite revision capture and verify all legacy callers.
    Map externally grown targets to conflicts; never clip or truncate them.
  - [x] Add deterministic reader-budget/growth regressions proving originals
    and dirty buffers survive refusal. Preserve symlink/path identity checks.
  - [x] Rerun full Rust gates, relevant slow-I/O/conflict fixtures, and new-core
    coverage after this discovered fix; Phase 6 cannot close until it passes.
  - [x] Review-accepted scope correction: every save/overwrite target-revision
    capture and document load is bounded, but two genuinely unbounded whole-file
    reads remain in the preview path (`src/preview_content.rs:1174` notebook
    loader reachable from `src/highlighting.rs:230-233`, and `:155` with a TOCTOU
    window). They are pre-existing Phase 3 reads, unchanged here, and filed as
    `FileManagerTUI-8u3` to be fixed under the bounded-I/O mandate.

## Phase 7: Sessions and Private Recovery

- [x] Task 1: Add versioned workspace session serialization

  **Files:** create `src/session.rs`; modify `src/main.rs`, `src/config.rs`,
  `src/workspace/documents.rs`, `src/workspace/layout.rs`.
  **Requirements:** FR-8, AC-3.
  **Produces:** `SessionRecord`, workspace-keyed load/save, schema/version checks;
  stores paths/cursors/layout/recent files, not running processes or executable trust.
  - [x] Test a save/reload round trip plus corrupt, future-version, missing-file,
    moved-root, and unwritable-state cases:
    ```rust
    assert_eq!(restored.active_document_path, saved.active_document_path);
    assert_eq!(restored.layout, saved.layout);
    assert!(!serialized_session.contains("trusted_server_commands"));
    ```
  - [x] Use an injected private state directory and atomic state writes; treat
    unavailable state as a non-fatal, visible persistence failure.
  - [x] Run `cargo test session`.

- [x] Task 2: Add bounded private recovery snapshots

  **Files:** create `src/recovery.rs`; modify `src/main.rs`, `src/config.rs`.
  **Requirements:** FR-8, AC-3.
  **Produces:** document/revision-keyed snapshots, restore/discard records,
  retention limits and explicit disable/clear controls.
  - [x] Test unsaved text, disk changed since snapshot, corruption, retention,
    symlinked/untrusted storage, and Unix owner-only permissions.
    ```rust
    assert_eq!(snapshot.text, "unsaved:\n  value: 1\n");
    assert_eq!(std::fs::read(&original).unwrap(), original_bytes);
    ```
  - [x] Never overwrite the original on restore; recover into a dirty document
    and require ordinary conflict-safe save. Reject unsafe storage locations.
  - [x] Run `cargo test recovery`; only delete owned expired recovery records.

- [x] Task 3: Integrate startup restoration and recovery commands

  **Files:** modify `src/app.rs`, `src/handler.rs`, `src/commands.rs`,
  `src/components/dialog.rs`, `src/components/settings.rs`, `src/main.rs`.
  **Requirements:** FR-8, AC-3.
  **Consumes:** session/recovery APIs; **produces:** non-blocking startup and
  restore/discard/clear interactions preserving explicit CLI workspace choice.
  - [x] Test restart after unsaved edit, user refusal, disabled persistence,
    missing roots, and explicit CLI-root override.
    ```rust
    assert!(restored_document.editor.modified);
    assert!(!serialized_session.contains("trusted_server_commands"));
    ```
  - [x] Throttle snapshots outside rendering; snapshot versions cannot overwrite
    newer recovered text. Bound shutdown work and expose persistence errors.
  - [x] Run `cargo test session`, `cargo test recovery`, and startup-routing tests.

- [x] Task 4: Automated checkpoint for session recovery

  **Requirements:** AC-3, AC-12.
  - [x] Run all gates and crash/restart fixtures with isolated state directories;
    verify no global clipboard or executable-trust persistence remains.

## Phase 8: Read-Only Git State

- [x] Task 1: Add bounded machine-readable Git backend

  **Files:** create `src/git.rs`; modify `src/main.rs`, `src/event.rs`.
  **Requirements:** FR-9, AC-7.
  **Produces:** `GitSnapshot`, branch/entry states, generation-tagged refresh using
  argument-vector `git status --porcelain=v2 -z --branch` and bounded timeouts.
  - [x] Create temporary repositories in tests; cover staged, unstaged, untracked,
    conflict, unborn/detached, rename, Unicode, spaces, and newline paths.
    ```rust
    let snapshot = parse_porcelain_v2(bytes).unwrap();
    assert!(snapshot.entries.iter().any(|e| e.path == unusual_name));
    assert!(snapshot.entries.iter().any(|e| e.is_conflicted()));
    ```
  - [x] Test missing executable/non-repository, cancellation, huge output, and
    stale results. Run Git with optional locks disabled where appropriate; no
    write commands occur in application code.
  - [x] Run `cargo test git`; setup commands that create test commits act only
    in disposable fixtures and use the environment's existing identity.

- [x] Task 2: Render branch/file/directory Git indicators

  **Files:** modify `src/app.rs`, `src/ui.rs`, `src/components/tree.rs`,
  `src/components/status_bar.rs`, `src/theme.rs`, `src/config.rs`,
  `src/components/settings.rs`.
  **Requirements:** FR-9, AC-5, AC-7.
  **Consumes:** GitSnapshot; **produces:** optional decorations and async refresh.
  - [x] Test aggregated directory status, color-independent markers, narrow
    status bars, theme/no-icons behavior, read-only roots, and S3 exclusion.
    ```rust
    assert!(tree_screen.contains("M"));
    assert!(status_screen.contains("main"));
    ```
  - [x] Refresh through background jobs rather than render-time subprocesses;
    events from a prior workspace cannot recolor the current tree.
  - [x] Run `cargo test git` plus tree/status snapshot tests.

- [x] Task 3: Automated checkpoint for read-only Git integration

  **Requirements:** AC-7, AC-11, AC-12.
  - [x] Run all gates. Inspect application Git command invocations and assert
    the backend's whitelist contains only read-only queries.

## Phase 9: Embedded-Terminal Compatibility

- [x] Task 1: Select and adapt a tested terminal-emulation implementation

  **Files:** modify `src/terminal/emulator.rs`, `src/terminal/mod.rs`,
  `src/components/terminal.rs`, `Cargo.toml`, `Cargo.lock`.
  **Requirements:** FR-11, AC-9.
  **Produces:** emulator adapter retaining selection/render-facing operations,
  plus terminal reply bytes returned to the PTY by the coordinator.
  - [x] Add ANSI fixtures for alternate screens, cursor modes, DSR responses,
    scroll regions, wide characters, combining marks, and resizing.
    ```rust
    emulator.process(b"\x1b[6n");
    assert!(!emulator.take_replies().is_empty());
    ```
  - [x] Compare the existing parser and a mature Rust candidate against these
    fixtures, license/MSRV/size constraints; record selection evidence.
    (measured: `.superpowers/sdd/plan/emulator-compare/REPORT.md`)
  - [x] Adapt the selected engine, do not claim nested full-screen compatibility
    based only on colored shell output.
  - [x] Run `cargo test terminal::emulator` and terminal widget tests.

- [x] Task 2: Preserve shell input and bound PTY process lifecycle

  **Files:** modify `src/terminal/pty.rs`, `src/terminal/mod.rs`, `src/handler.rs`,
  `src/event.rs`, `src/app.rs`, `src/main.rs`.
  **Requirements:** FR-6, FR-11, AC-9.
  **Consumes:** emulator replies/keymap; **produces:** ordered paste/key/reply input,
  exit events, bounded output and shutdown, hide/show persistence.
  - [x] Test Tab/Esc/Ctrl+C/Alt-modified keys, multiline bracketed paste,
    shell exit/restart, rapidly changing dimensions, and slow/failed writes.
    ```rust
    assert_eq!(key_event_to_bytes(&escape, false), vec![0x1b]);
    assert_eq!(key_event_to_bytes(&tab, false), vec![b'\t']);
    ```
    (keys incl. Ctrl+C → 0x03 proven byte-exact through a real raw-mode `cat`
    PTY child in `keymap_terminal_keys_reach_pty_unchanged`; DECCKM + bracketed
    paste matrix in `keymap_decckm_selects_ss3_cursor_sequences` /
    `keymap_bracketed_paste_wraps_only_when_child_enabled_it`; exit→fresh-session
    in `terminal_exit_then_restart_spawns_fresh_session`; resize storm +
    hide-keeps-child in `terminal_rapid_resizes_and_hide_keep_child_alive`;
    full/closed budget behavior in `transport_stdin_*` pty tests.)
  - [x] Pass mode-correct input to interactive applications; reserve only explicit
    workspace chords, and never auto-run commands from OSC output.
    (DEC mode 1 → SS3 arrows/Home/End; DEC mode 2004 → `\x1b[200~…\x1b[201~`
    wrap, end-to-end through a real PTY in
    `terminal_bracketed_paste_wraps_when_child_requested`; OSC inertness pinned
    by `fixture_osc_sequences_are_inert_never_executed`.)
  - [x] Use async/bounded writer paths and process cleanup tests; hide does not
    kill the child, but quit cannot wait indefinitely on it.
    (`test_shutdown_of_busy_child_is_bounded_and_reaped`: SIGKILL+reap+joins
    complete < 3 s while output floods and input is queued; emulator DSR/DA
    replies now drain through `process_terminal` into the same ordered PTY
    write queue — `terminal_dsr_reply_drains_to_child_stdin` proves the bytes
    reach a real child blocked on `\x1b[6n`.)
  - [x] Run `cargo test terminal` and terminal input handler tests.
    (57 terminal-filtered + full suite 1363 green; both clippy gates, release
    build, fmt clean.)

- [x] Task 3: Automated checkpoint for terminal compatibility

  **Requirements:** AC-9, AC-11, AC-12.
  - [x] Run full gates plus scripted PTY compatibility fixtures; verify child
    cleanup and ordinary Esc/autocomplete/interrupt delivery.
    (Full suite 1365 green; `cargo clippy --all-targets -- -D warnings` and
    `--bin fm` clean; `cargo fmt --check` clean; release build ok; pyte
    reference PTY fixtures 11/11; strict literal diff coverage vs baseline:
    24/27 added lines hit, 2 doc-comment no-DA, 1 structural `}` 0-hit —
    every executable path covered. Esc/Tab/autocomplete byte-exact through a
    real `cat` PTY; SIGINT propagation proven end-to-end by
    `terminal_ctrl_c_interrupt_reaches_foreground_child` (0x03 → child death
    → TerminalClosed); child cleanup by
    `test_shutdown_of_busy_child_is_bounded_and_reaped`; reply-drain error
    path by `terminal_reply_write_failure_reports_status`.)

## Phase 10: LSP Positions, Transport, Client, and Trust

- [x] Task 1: Define LSP position-encoding adapters

  **Files:** create `src/lsp/mod.rs`, `src/lsp/positions.rs`; modify `src/main.rs`.
  **Requirements:** FR-10, AC-8.
  **Consumes:** text byte/display conversions; **produces:** `PositionEncoding`,
  `byte_to_lsp(text, byte, encoding)` and `lsp_to_byte(text, character, encoding)`.
  - [x] Add UTF-8/16/32 round trips, non-BMP text, invalid ranges, CRLF lines,
    tabs, and combining-character tests:
    ```rust
    assert_eq!(byte_to_lsp("a😀b", 5, PositionEncoding::Utf16).unwrap(), 3);
    assert_eq!(lsp_to_byte("a😀b", 3, PositionEncoding::Utf16).unwrap(), 5);
    ```
    (every char boundary round-trips in all three encodings across
    ASCII/CJK/non-BMP/combining/tab/CR text.)
  - [x] Negotiate encoding; use UTF-16 only as the protocol fallback. Reject
    out-of-range/surrogate-interior edits without corrupting UTF-8.
    (`PositionEncoding::from_capability` maps "utf-8"/"utf-16"/"utf-32" and
    falls back to Utf16 for absent/unrecognized values; `lsp_to_byte` returns
    None on surrogate-interior and overshoot offsets, `byte_to_lsp` returns
    None on non-boundary bytes — no clamping, no mid-scalar slicing.)
  - [x] Run `cargo test lsp::positions`.
    (8 tests green; staged-surface `#[allow(dead_code)]` documented until
    Task 2 transport consumes it.)

- [x] Task 2: Implement bounded stdio JSON-RPC framing

  **Files:** create `src/lsp/transport.rs`; modify `src/lsp/mod.rs`.
  **Requirements:** FR-10, AC-8.
  **Produces:** frame decoder/encoder, bounded read/write channels, message-size
  errors, and executable/argument process spawn without a shell.
  - [x] Test fragmented/coalesced headers and payloads, malformed/oversized
    frames, EOF, Unicode lengths, slow peers, and stderr draining.
    ```rust
    let body = br#"{"jsonrpc":"2.0","id":1,"result":null}"#;
    let framed = encode_frame(body);
    assert_eq!(decode_one(&framed).unwrap(), body);
    ```
    (every split point of a two-frame wire decodes identically; EOF clean at
    boundaries/`EofMidFrame` mid-frame; unicode bodies measured in bytes;
    real-child round trip through `cat` echo; 400-line stderr flood capped
    at the 200-line ring.)
  - [x] Use byte Content-Length, bound buffered partial frames, and separate
    stderr from protocol messages. Keep transport independent of App.
    (`FrameDecoder` bounds headers at 8 KiB and declared bodies at 16 MiB;
    `LspTransport::spawn` execs argv via `Command` with no shell; outbound
    queue 64 slots, `send` reports `WouldBlock`/`BrokenPipe` rather than
    stalling; shutdown drops the queue sender so writer `recv` ends and all
    joins return.)
  - [x] Run `cargo test lsp::transport`.
    (10 tests green incl. real-child echo, stderr drain, bounded send,
    spawn-failure error paths.)

- [x] Task 3: Implement server/request lifecycle with fake peers

  **Files:** create `src/lsp/client.rs`; modify `src/lsp/mod.rs`, `src/event.rs`;
  create `scripts/fake-lsp-server.py`.
  **Requirements:** FR-10, AC-8.
  **Produces:** client initialization/capabilities, request IDs/pending requests,
  cancellation/timeouts, shutdown/exit, server generation and crash cleanup.
  - [x] Use deterministic fake-server modes for success, unsupported method,
    delayed response, crash, malformed frame, oversized output, and ignored exit.
    ```rust
    assert!(client.accepts(server_generation, document_version));
    assert!(!client.accepts(old_server_generation, document_version));
    ```
    (eight `scripts/fake-lsp-server.py` modes — success, unsupported, delayed,
    crash, malformed, oversized, noexit, apply-edit — exercised over real
    pipes; generation/`accepts` staleness contract pinned by unit tests.)
  - [x] Reply explicitly to unsupported server requests; never auto-apply
    workspace edits or run execute-command requests. Limit/restart failures
    without an infinite restart loop.
    (every server-initiated request gets a `-32601` reply — proven end-to-end
    by the `apply-edit` fake reporting our error code back; restart budget
    exhausts at 3.)
  - [x] Run `cargo test lsp::client` against fake peers, not installed real servers.
    (19 tests: 10 scripted-peer unit tests + 9 real-child fake-server tests.)

- [x] Task 4: Add installed-server configuration and project execution trust

  **Files:** create `src/lsp/config.rs`; modify `src/lsp/mod.rs`, `src/config.rs`,
  `src/main.rs`, `src/app.rs`, `src/commands.rs`, `src/components/dialog.rs`,
  `src/components/settings.rs`.
  **Requirements:** FR-10, AC-8, AC-11.
  **Produces:** language mappings, argv validation, workspace-root matching,
  capability/status display, explicit project-command trust.
  - [x] Test global/local precedence, missing executable, refused trust,
    noninteractive startup, modified project argv, and session restore:
    ```rust
    assert!(!project_server.allowed_without_explicit_trust());
    assert!(!restored_session_grants_execution);
    ```
    (Both pinned assertions implemented verbatim in `lsp::config` tests —
    `project_server_never_executes_without_explicit_trust`,
    `session_restore_never_grants_execution`. Provenance flows via
    `lsp_global`/`lsp_local` on `AppConfig`; project `[[lsp.trust]]` is
    stripped at load with a warning; trust keys bind canonicalized
    (root, argv); headless = `event_tx.is_none()` → `Denied("headless start")`;
    a modified project argv re-gates — stale session is shut down boundedly
    first.)
  - [x] Keep startup asynchronous and basic editing available; require no
    automatic server installation. Bind trust to the actual workspace/command,
    not just a language name.
    (All spawning happens on per-session pump threads; `maybe_start_*` only
    resolves + gates + queues the dialog. Nothing is ever downloaded.)
  - [x] Run `cargo test lsp::config` and TOML/trust dialog tests.
    (11 new config tests + 6 manager tests + TOML/strip/provenance tests +
    `LspTrust`/`LspStatus` dialog render & key tests + command dispatch
    tests. Full suite 1438 green; diff coverage 98.9% — residuals are
    defensive catch-alls, test internals, and `main()` wiring.)

- [x] Task 5: Automated checkpoint for LSP foundation

  **Requirements:** AC-8, AC-11, AC-12.
  - [x] Run full gates, fake-server fault suite, position encodings, and process
    cleanup assertions. No optional executable is needed for the default suite.
    (1439 tests green; clippy `-D warnings`, `fmt --check`, release build all
    clean. Fault suite: `fake_unsupported_method`, `fake_delayed_response`,
    `fake_crash_marks_dead`, `fake_malformed_output`, `fake_oversized_reply`,
    `fake_ignored_exit`, `fake_apply_edit_unsupported`, restart-budget
    exhaustion. Encodings: `lsp::positions` 8 tests + negotiated-encoding
    handshake tests. Cleanup: `shutdown()` now captures the reaped
    `ExitStatus` and `transport_shutdown_reaps_the_child_and_records_status`
    asserts it — a real cleanup assertion, not just bounded timing. Every
    fake is the bundled `scripts/fake-lsp-server.py` or a tempdir script;
    no installed server is required. Diff coverage 100% executable.)

## Phase 11: Language Features and Diagnostics

- [x] Task 1: Synchronize open/changed/saved/closed documents

  **Files:** create `src/lsp/features.rs`; modify `src/lsp/mod.rs`,
  `src/workspace/documents.rs`, `src/app.rs`, `src/event.rs`.
  **Requirements:** FR-4, FR-10, AC-8.
  **Produces:** versioned didOpen/didChange/didSave/didClose using negotiated
  full/incremental synchronization and stable file URIs.
  - [x] Assert fake-server transcripts for two unsaved documents, edit/paste/
    undo, save, close, rename, external reload, and server restart:
    ```rust
    assert!(sent_versions.windows(2).all(|v| v[0] < v[1]));
    assert_eq!(server_text, active_document_text);
    ```
    (Implemented verbatim in
    `document_sync_transcript_matches_active_documents_end_to_end`: two
    unsaved docs, insert_text/undo, save+didSave, rename = didClose+didOpen,
    external reload, didClose, restart re-open — per-generation strictly
    increasing versions, `server_text == active_document_text` against the
    fake's mirror.)
  - [x] Skip read-only S3/binary previews; reopen supported active documents
    after server restart without duplicating versions/events.
    (S3/binary/readonly previews never reach DocumentStore so the poll never
    tracks them. Restart re-opens via `SyncedDocuments::reopen_params` — same
    versions, mirror text — asserted `a2` opens exactly twice, not more.)
  - [x] Run `cargo test lsp::features` and document event tests.
    (1454 tests pass incl. 10 lsp::features + 79 lsp::*; E2E transcript test
    deterministic across repeated runs.)
  - [x] Implementation: `src/lsp/features.rs` (TextSync capability parsing,
    `uri_for_path`, `position_at`, char-boundary-safe `incremental_change`,
    mirror-based `SyncedDocuments`); `src/lsp/mod.rs` (poll-based
    `sync_documents` reconcile: open/change/close/rename uniformly via
    content_revision, `document_saved` gated on save capability,
    `close_tracked` silent on dead sessions, Ready-arm `reopen_params`);
    `src/lsp/client.rs` (`ClientEvent::Ready` carries negotiated `TextSync`);
    `src/app.rs` (`sync_lsp_documents` poll after events, `document_saved`
    hook in `finish_editor_save`); `src/main.rs` (reconcile call in the event
    loop); `scripts/fake-lsp-server.py` (`sync` mode: utf-16 range
    application + atomic per-message transcript flushes + restart log
    carry-forward). `documents.rs`/`event.rs` needed no changes — the poll
    reads existing `content_revision`/`Document::path` and `Event::Lsp`
    already existed. Bug found by coverage: `incremental_change`'s suffix
    back-off rechecked stale range ends and would underflow; fixed by
    recomputing inside the loop. Diff coverage 99.9% executable — sole
    residual is the `app.sync_lsp_documents()` call line inside the
    systematically-uncovered `run()` event-loop region in main.rs
    (all neighboring pre-existing lines are 0-hit too).)

- [x] Task 2: Add completion, hover, definition, references, and symbols

  **Files:** modify `src/lsp/features.rs`, `src/app.rs`, `src/handler.rs`,
  `src/commands.rs`, `src/ui.rs`, `src/editor.rs`;
  create `src/components/language_features.rs`; modify `src/components/mod.rs`.
  **Requirements:** FR-10, AC-1, AC-8.
  **Produces:** bounded feature overlays/results, command routes, capability
  fallback, safe completion edit application as one undo transaction.
  - [x] Test completion lists/item defaults, insert/replace forms, additional
    edits, unsupported snippets, stale results, overlapping/invalid edits,
    hover sanitization, and definitions/references in another file.
    ```rust
    apply_completion(&mut document, completion).unwrap();
    document.editor.undo();
    assert_eq!(document.text(), before_completion);
    ```
    (Done — `parse_completion` covers item defaults, Insert/Replace edits,
    additionalTextEdits (incl. malformed-range rejection and unnamed-item
    skip), snippet opt-out, empty/overlapping/out-of-bounds rejection;
    `EditorState::apply_edit_ranges` applies the batch then undoes to the
    exact pre-completion text as pinned; multi-line and disjoint-batch
    edits each revert in one undo; hover sanitization strips ANSI/OSC/C1
    including mid-scalar bounds; cross-file definitions/references open
    the target document and clamp the target position.)
  - [x] Never execute completion commands; unsupported formats fail visibly
    without partial application. Navigation preserves the originating dirty
    document/cursor and rejects unsupported URI schemes.
    (Done — completion `command` payloads are parsed but never dispatched
    (no LspCommand path exists for them); unsupported kinds and invalid
    edit ranges surface a status note with the document untouched;
    navigation keeps the dirty originating document and its cursor,
    refuses non-`file:` URIs with a visible note, and drops busy/stale
    results without clobbering an open overlay.)
  - [x] Run language-feature tests using fake-server transcripts.
    (Done — `feature_request_round_trips_against_fake_server` sends
    completion + string-id workspace/symbol requests through the fake
    stdio server and asserts the transcript contents; 1495 tests green.
    Diff coverage 100% executable (2487/2488) — sole residual is the
    `app.drain_lsp_results()` call line inside the
    systematically-uncovered `run()` event-loop region in main.rs
    (same class as the accepted Task-1 residual; every neighboring
    pre-existing line is 0-hit). Diverging test arms
    (`unreachable!`/`panic!`/`break`-block `}`) that LCOV reports as
    0-hit are either single-lined onto an executing `let` under
    `#[rustfmt::skip]` or restructured to `while`-condition loops so
    `cargo fmt --check` and coverage both stay green.)

- [x] Task 3: Add versioned diagnostics state and navigable panel

  **Files:** create `src/diagnostics.rs`, `src/components/diagnostics.rs`;
  modify `src/main.rs`, `src/components/mod.rs`, `src/app.rs`, `src/event.rs`,
  `src/ui.rs`, `src/handler.rs`, `src/commands.rs`, `src/components/editor.rs`.
  **Requirements:** FR-10, AC-5, AC-8.
  **Produces:** per-document/server diagnostic generations, severity summaries,
  bounded rows/gutter markers, next/previous/navigate commands.
  - [x] Test error/warning/info/hint, empty replacement, unsupported/stale
    versions, close/restart, Unicode ranges, multi-file results, and tiny panels:
    ```rust
    assert_eq!(diagnostics.for_document(id).len(), 1);
    diagnostics.apply(stale_publish);
    assert_eq!(diagnostics.for_document(id).len(), 1);
    ```
    (`src/diagnostics.rs` 18 tests + app/handler/commands/ui integration tests:
    every severity maps and renders; empty `diagnostics: []` replaces a stored
    entry; stale explicit versions are ignored — the pinned assert pair is
    implemented verbatim in `versioned_stale_publish_never_overwrites`;
    `Ready`/`ServerDied` clear a whole server, close removes the document's
    URI from every server (clean-close, discard, stale-decision and
    quit-cancel paths all covered); UTF-16 positions land via
    `goto_lsp_position`; multi-file rows order `file:` before `untitled:`;
    `render_bounds_rows_and_survives_tiny_frames` renders down to a 1x1
    surface.)
  - [x] Treat versionless publishes according to a documented generation policy;
    do not attribute another server's results to the current document version.
    (`Diagnostics::apply` documents the policy in the module docs: an explicit
    version must equal the document's synced version for that server
    (`LspManager::synced_version`), a `null` version always applies, and an
    unsynced document is baseline-less so the publish's own version is stored
    as its baseline — entries are keyed per `(language, uri)` so one server's
    results can never overwrite another's. `Ready`/`ServerDied` queue a
    `DiagnosticEvent::Clear` scoped to that language only.)
  - [x] Run `cargo test diagnostics` and panel/navigation snapshots.
    (27 diagnostics-filtered tests green; full suite 1522 green; both clippy
    gates, release build, fmt clean. Panel renders via
    `DiagnosticsPanel::render` on a clamped centered surface; navigation
    through `DiagnosticsNext`/`DiagnosticsPrev` wraps and reports
    `Diagnostic i/n (marker)`; `LspDiagnostics` toggles the overlay and
    `dispatch_command`'s non-Normal-overlay gate is asserted. Diff coverage
    776/777 = 99.9% — sole residual `main.rs:1530`
    (`app.drain_lsp_diagnostics()` inside the systematically-uncovered
    `run()` loop, same accepted class as Task 2's 1529). `src/event.rs`
    listed in Files needed no change — `DiagnosticEvent` lives in
    `src/diagnostics.rs` and drains through `take_diagnostics()` polled
    beside `take_results()`. Quit-cancel losing diagnostics was a real bug
    found and fixed: `remove_document` now sits inside the `!quitting`
    arm of `advance_document_lifecycle`.)
  - [x] Implementation: `src/diagnostics.rs` (`Severity`, `Publish`,
    `DiagnosticEvent`, `Diagnostics` store over
    `BTreeMap<(language, uri), Entry>` with `revision`; bounded
    MAX_ITEMS_PER_DOCUMENT=256, MAX_DOCUMENTS_PER_SERVER=64,
    MAX_PANEL_ROWS=512, MAX_MESSAGE_CHARS=160), `src/components/diagnostics.rs`
    (`DiagnosticsPanel`: rebuild/move_selection/hit/title/render),
    `AppMode::Diagnostics` overlay (Esc/Enter/j/k/PageUp/Down/Home/End +
    mouse click/drag-to-scroll/dismiss-outside), status-bar severity
    segment, editor-gutter severity styling, `LspDiagnostics`/
    `DiagnosticsNext`/`DiagnosticsPrev` commands gated on `lsp.enabled()`,
    `drain_lsp_diagnostics` in the run loop, and URI removal hooks on
    close/discard/rename/save-as/undo-rename paths.

- [ ] Task 4: Automated checkpoint for language features

  **Requirements:** AC-1, AC-5, AC-8, AC-11, AC-12.
  - [ ] Run full gates and end-to-end fake-server workflows with multiple documents,
    unsupported capabilities, startup failures, and all lifecycle cleanup.

## Phase 12: Automated Terminal/Browser Acceptance and Handoff

- [ ] Task 1: Build isolated automated PTY and browser-terminal fixtures

  **Files:** create `scripts/test-terminal-workspace.py`,
  `tools/terminal-tests/package.json`, `tools/terminal-tests/package-lock.json`,
  `tools/terminal-tests/server.mjs`, `tools/terminal-tests/index.html`,
  `tools/terminal-tests/workspace.spec.mjs`.
  **Requirements:** AC-9, AC-10.
  **Produces:** bounded PTY runner and Playwright/xterm.js-style test fixture
  launching the local binary with temporary workspace/config/state directories.
  Test helpers include `terminalText(page) -> Promise<string>` reading the
  xterm buffer and `readFixtureFile(name) -> Promise<string>` confined to the
  generated fixture root. `fixtureUrl` comes from the server's ready message;
  import `test`/`expect` from Playwright in the test module.
  - [ ] Add a smoke test that launches/closes the binary and verifies child
    cleanup before implementing complete acceptance scenarios.
    ```javascript
    test('web profile has a usable command route', async ({ page }) => {
      await page.goto(fixtureUrl);
      await page.getByRole('button', { name: 'Focus terminal' }).click();
      await expect.poll(async () => await terminalText(page))
        .toContain('Commands');
    });
    ```
  - [ ] Bind test transport only to loopback with an ephemeral port, no saved
    auth, and no external service. Pin development dependencies and keep them
    out of production builds.
  - [ ] Provide `python3 scripts/test-terminal-workspace.py` and
    `npm --prefix tools/terminal-tests ci` /
    `npm --prefix tools/terminal-tests test` as deterministic entry points.

- [ ] Task 2: Automate the complete acceptance and failure matrix

  **Files:** modify `scripts/test-terminal-workspace.py`,
  `tools/terminal-tests/workspace.spec.mjs`, `scripts/fake-lsp-server.py`;
  add fixtures under `tools/terminal-tests/fixtures/`.
  **Requirements:** AC-1 through AC-11.
  **Produces:** automated multi-document/tree/shell/paste/save/restart/Git/LSP
  scenarios, meaningful terminal screen assertions, and exact file-byte checks.
  - [ ] Run the 80x24 workflow, resize to 120x40/60x20, and verify YAML bytes,
    retained dirty document, shell output, tabs, diagnostics, and Git markers.
    ```javascript
    expect(await readFixtureFile('config.yaml'))
      .toBe('training:\n  lr: 0.001\n');
    ```
  - [ ] Cover browser-reserved shortcut exclusions, browser-copy fallback,
    no mouse/icons/watcher/terminal, missing executables, malformed/slow LSP,
    external saves, crashes, and corrupt/private recovery.
  - [ ] Add automated TERM variants and nested tmux transport where configured.
    Record untested live SSH/Jupyter/Kubeflow deployment boundaries explicitly;
    controlled transport evidence is not a production-deployment claim.
  - [ ] Enforce timeouts and process cleanup. A missing mandatory harness is a
    blocked check, not a silent skip or a request for manual verification.

- [ ] Task 3: Document configuration and automate release compatibility checks

  **Files:** modify `README.md`, `PLAN.md`, `conductor/product.md`,
  `conductor/tech-stack.md`, `conductor/product-guidelines.md`;
  add `scripts/check-terminal-workspace.sh`; modify
  `.github/workflows/ci.yml` only to call approved automated checks.
  **Requirements:** FR-5 through FR-11, AC-12.
  **Produces:** matching docs, server argv/trust examples, safe-save limits,
  recovery policy, keymap profiles, browser-test commands, coverage/build evidence.
  - [ ] Document explicit installed-server examples for Python/YAML/Rust while
    keeping tests on fake peers. Avoid implying every server supports every feature.
  - [ ] Add automated Linux/static-musl and available macOS/Windows build checks;
    keep existing tag/manual CI triggers unchanged.
  - [ ] Measure binary size and new-core coverage; document unsupported platform
    behavior and blockers rather than weakening safety to pass a build.
  - [ ] The shell quality runner executes Rust, PTY, browser, coverage, and
    configured build gates with nonzero exit on mandatory failures.

- [ ] Task 4: Final automated acceptance checkpoint and tracking handoff

  **Requirements:** all Functional Requirements and AC-1 through AC-12.
  - [ ] Run `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`,
    `python3 scripts/test-terminal-workspace.py`,
    `npm --prefix tools/terminal-tests test`, and the coverage/build gates.
  - [ ] Produce a requirement-to-test evidence table with failures/blockers
    clearly identified. No criterion is satisfied by a manual test.
  - [ ] Verify Git/LSP/runtime fallback, process cleanup, recovery privacy,
    release artifact size, and cross-platform results from actual output.
  - [ ] Update Beads/track status only after all mandatory checks pass. Capture
    reusable learnings and file any remaining follow-up work without closing
    unrelated archived issues.
  - [ ] Report changed files and Git status. Do not commit/push/sync remotely
    without active authority. This track has no manual completion gate.

## Requirement Coverage

| Requirements | Implementation phases | Acceptance evidence |
|---|---|---|
| FR-1, FR-2 | 1, 3, 7 | AC-2, AC-3 |
| FR-3 | 2, 4, 9 | AC-4, AC-9, AC-10 |
| FR-4 | 3, 6, 7, 11 | AC-1, AC-3, AC-8 |
| FR-5 | 2, 4, 5 | AC-5, AC-11 |
| FR-6 | 4, 9, 12 | AC-1, AC-9, AC-10 |
| FR-7 | 6 | AC-6, AC-11 |
| FR-8 | 2, 7 | AC-3, AC-10 |
| FR-9 | 8 | AC-7 |
| FR-10 | 10, 11 | AC-8, AC-11 |
| FR-11 | 9 | AC-9 |
| Non-functional requirements | All; release evidence in 12 | AC-12 |

## Planning Validation and Execution Boundary

These documents describe future implementation, not implemented features.
All task checkboxes start unchecked, Beads records start open, and the track
starts new. Track creation validates document links, metadata, requirement/task
coverage, dependency acyclicity, disjoint parallel ownership, and Beads mappings.
It does not claim application tests passed or that the runtime redesign exists.
