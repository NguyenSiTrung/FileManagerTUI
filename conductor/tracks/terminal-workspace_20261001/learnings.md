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

## 2026-10-01 - Phase 1 Task 1: Application baseline

- **Implemented:** Resolved the locked dependencies and established executable
  automated gates without changing Cargo.toml or Cargo.lock.
- **Toolchain:** rustc 1.94.0 (4a4ef493e), cargo 1.94.0 (85eff7c80),
  native target aarch64-unknown-linux-gnu; cargo-llvm-cov 0.9.1.
- **Verification:** `cargo test --locked`: 570 passed, zero failed.
  `cargo clippy --locked -- -D warnings` and `cargo fmt --check`: passed.
  Tests emit a pre-existing unused-variable warning at handler.rs:4173.
- **Coverage:** `cargo llvm-cov --locked --summary-only`: 570 passed;
  overall line coverage 72.26%, region coverage 73.71%, function execution
  82.53%. Editor line coverage 65.74%; operations 89.63%. These are baseline
  numbers, not evidence that future core logic meets the 80% target.
- **Artifact:** `cargo build --release --locked`: passed; native GNU release
  binary 2,842,752 bytes. This is not a static-musl/macOS/Windows result.
  Only the native Rust target is currently installed.
- **Authority:** Implement in the repository's configured master workflow.
  User requested implementation of the approved design, not commits, pushes,
  or remote Dolt sync; none performed.
- **Beads gotcha:** Planning assigned issues to `conductor`. Use
  `bd --sandbox --actor conductor` for this workflow's writes/closures instead
  of force-reassigning them. This is not a Git author identity change.
- **Parallel boundaries:** Task 2 owns operations.rs and Cargo manifests;
  Task 3 owns save.rs, fs/mod.rs, editor.rs. Coordinator alone owns track
  state and later integration. No concurrent ownership overlaps.

## 2026-10-01 - Phase 1 Tasks 2–5: File-safety checkpoint

- **Implemented:** Exclusive new-file creation; Linux `renameat2(RENAME_NOREPLACE)`
  and macOS `renamex_np(RENAME_EXCL)`; typed save revisions/errors; exclusive
  same-directory temporary writes; permission preservation and safe refusal;
  saved undo-revision identity; LF/CRLF/trailing-newline preservation.
- **UI:** Conflict offers Reload (explicitly discard buffer), Save As (create
  only), Overwrite (second explicit confirmation), or Cancel. Confirmed overwrite
  checks the revision captured when the dialog opened. A changed target during
  confirmation is rejected. Mixed endings require explicit LF normalization.
  Failed reload, save, Save As collision, and cancelled close retain dirty text.
- **Files:** Cargo.toml/lock, fs/operations.rs, new fs/save.rs, fs/mod.rs, editor.rs,
  app.rs, handler.rs, components/dialog.rs. No commits or remote writes.
- **TDD:** Operations worker observed 9 failures before implementation.
  Editor worker observed CRLF, undo-to-saved, external-conflict, and normalization
  regressions fail. Coordinator observed conflict/cancel, watcher restoration,
  reload, Save As, explicit overwrite, dialog choices, xattr loss, and owner-write
  regressions fail before fixes.
- **Final gates:** `cargo test --locked --quiet`: 615 passed, no failures/ignored
  tests. `cargo clippy --locked -- -D warnings`, `cargo fmt --check`, and
  `git diff --check`: passed. The pre-existing unused test variable was fixed.
- **Coverage:** LLVM run with 612 tests before the final three handler
  characterization tests: overall 74.12% lines; operations 91.86%; save 87.94%.
  New-core scope, added executable production lines from the diff plus all new
  save.rs production lines, excluding unit-test modules: 529/584 = **90.58%**.
  Per-file added lines: app 117/135, handler 35/37, dialog 70/71, editor 74/75,
  operations 32/32, save 201/234. Evidence comes from
  `cargo llvm-cov report --text` and diff line ranges, not whole-repository
  coverage being mislabeled as new-core coverage.
- **Artifact:** Native aarch64 GNU release binary: 2,908,288 bytes, +65,536 bytes
  from baseline, below 10 MB. Static-musl/macOS/Windows not yet built.
- **Review fixes:** Refuse original xattrs/ACLs and inherited temporary-file ACLs
  instead of silently dropping or broadening permissions. Verified actual Linux
  default-ACL fixture. Require owner-write permission under the same-owner policy,
  including a 0460 file that `Permissions::readonly()` incorrectly accepts.
- **Limits:** Linux/macOS replacement only; other platforms refuse replacement.
  Windows/other-platform rename refuses until a no-replace implementation exists.
  Attribute inspection errors fail closed. Symlinks (including ancestors), hard
  links, unpreservable ownership, owner-read-only targets, and extended attributes
  are refused; Save As is non-overwriting. Parent directories must be trusted and
  stable. Revision-check/rename race remains advisory, not compare-and-swap.
  File data is synced; parent-directory crash durability is not guaranteed.
- **Discovered work:** Extra `cargo clippy --locked --all-targets -- -D warnings`
  fails with 43 pre-existing test-only lints. Required Clippy passes. Tracked in
  **FileManagerTUI-8sy**, not silently waived or fixed with an unrelated refactor.
- **Next:** Phase 2 Task 1 converts mixed char/byte editor coordinates to
  UTF-8-byte/grapheme/display-column semantics.

## 2026-10-01 - Phase 2 Task 1: Text coordinates

- **Implemented:** New text.rs with TextPosition, byte/display conversion,
  grapheme navigation, four-column tab stops, and matching rendering/mouse
  coordinates. Editor buffers/history/find/selection use UTF-8 bytes consistently.
  Vertical navigation preserves preferred display columns.
- **Files:** text.rs, main.rs, editor.rs, handler.rs, components/editor.rs,
  Cargo.toml/lock. Direct Unicode dependencies reuse locked versions:
  unicode-segmentation 1.12.0, unicode-width 0.2.0.
- **TDD:** Behavioral reds covered conversion, grapheme edits/grouping/joining,
  selection history, replacement boundaries, differing-length replace-all,
  find-input backspace, stale selections, and combining-mark indentation.
- **Verification:** Independently rerun full suite: 635 passed.
  Required Clippy, formatting, and diff checks passed.
- **Gotcha:** Find results already use byte offsets; converting those again as
  character positions corrupts Unicode replacements. Mouse positions are cells,
  so convert once at the input boundary.

## 2026-10-01 - Phase 2 Task 2: Literal bracketed paste

- **Implemented:** Typed paste events reach the active input context without
  normal-mode command dispatch. Editor paste inserts/replaces multiline text
  as one compound range transaction without auto-indent. CRLF paste converts
  to logical LF without changing the document's saved line-ending policy.
  Empty paste is a no-op; 1 MiB payload and configured document-size/line guards
  reject before selection mutation. Repeated paste creates separate undo steps.
- **Files:** editor.rs, handler.rs, event.rs, main.rs, tui.rs.
- **History:** ReplaceText retains only removed/inserted text, not whole-buffer
  snapshots, and stores before/after byte positions for undo/redo.
- **Terminal state:** Enable bracketed paste on setup/resume; disable on
  restore/suspend/panic/error cleanup. Tui Drop covers event/render errors.
  Cleanup attempts remaining terminal modes even after an output failure.
  Blocking Crossterm polling moved to spawn_blocking.
- **TDD:** Five paste-buffer regressions, three event/lifecycle regressions,
  and three routing regressions failed before implementation. Native-mode
  clipboard/file-operation semantics are still separate.
- **Verification:** Full suite 649 passed; required Clippy, formatting, and diff
  checks passed. Real cat PTY test proves terminal routing without editor/quit
  commands. Isolated automated 80x24 PTY subprocess fixture delivered actual
  bracketed-paste `q`, observed paste rejection in normal tree context (no quit),
  then ordinary `q` exit; verified zero exit, disabled bracketed paste,
  left alternate screen, exact original termios flags, and child exit.
- **Boundary:** Embedded terminal mode-negotiated wrappers and bounded async
  writers remain Phase 9; bounded queues/redraws remain Phase 6. Single-line
  modal/find input rejects multiline paste rather than interpreting it as keys.
- **Next:** Private async text-copy outcomes, browser overlay focus restoration,
  selection-vs-line clipboard behavior, and removal of shared clipboard files.

## 2026-10-01 - Phase 2 Task 3: Private text clipboard

- **Implemented:** One asynchronous text-copy backend for editor, preview,
  terminal and path copies. Native success is acknowledged; native failure
  requests OSC52 only for recognizable terminals and exposes a manual-copy
  overlay. OSC52 is explicitly unconfirmed. Clipboard text stays in memory.
- **Clipboard semantics:** Selection copy preserves trailing newlines and uses
  literal insert_text for inline/replacement paste, with one undo. Whole-line
  copy/cut remains linewise, including empty and final-line handling. File
  operation clipboard remains separate.
- **Native commands:** argv/stdin only, no shell interpolation or temporary
  text file. 1 MiB payload bound; blocked stdin and process duration bounded
  to two seconds per optional native tool. Tests inject fake backends.
- **Overlay:** Bounded multiline unwrapped viewport with scrolling. Dismissal
  restores the original mode/focus, including dirty editor state. Only restore
  mouse capture suspended by this overlay; honor disabled mouse mode.
- **TDD:** Nine genuine behavior reds covered inline paste/replacement, native
  outcomes/focus, multiline overlay, empty/linewise clipboard, mouse transport,
  and configured document paste guards.
- **Verification:** Clipboard 21, preview 116 and terminal 69 tests passed.
  Independently rerun full suite: 658 passed; required Clippy, format, and diff
  checks passed. No test wrote the native clipboard or emitted OSC52 externally.
- **Limits:** Browser selection copies visible text only and may normalize
  whitespace/trailing newlines; the overlay states this limitation. Browser
  clipboard reads are not assumed. Controlled browser acceptance remains
  Phase 12. Native fallback tests do not exercise an installed clipboard tool.
- **Next:** Shared visual-row/byte/display mapping for scroll/wrap rendering,
  cursor tracking, highlights, and mouse input.

## 2026-10-01 - Phase 2 Tasks 4–5: Viewports and input checkpoint

- **Implemented:** Streaming shared visual rows/graphemes, logical tab stops,
  cell clipping/painting, styled editor/preview wrap, independent horizontal
  and vertical offsets. Cursor/find/selection/mouse share byte/display mapping.
  Preview copy expands selected tab cells, retains whole selected graphemes,
  emits blanks for partial wide cells, and inserts only logical newlines.
  Exact-width wrapped editor rows retain an EOL cursor slot.
- **Controls:** Alt+W toggles editor/focused-preview wrapping; Left/Right scroll
  unwrapped preview four cells. Future command registry exposes these helpers.
  Gutters and find rows use bounded actual code viewport dimensions.
- **TDD:** 14 initial behavior reds; 29 initial fixtures added. Task review
  reproduced P1 Down traps on split tabs/oversized glyphs and same-byte Delete
  reflow hiding the cursor. Fix round 1 added seven fixtures, with four initial
  reds plus an expanded width-3 tab red. Scoped re-review approved both fixes.
- **Fix pattern:** Validate a proposed cursor's canonical visual row and scan
  past unrepresentable continuation positions, including same-row right
  boundaries. Constant-size content revisions invalidate wrapped cursor layout
  after grouped mutations and undo/redo; unchanged frames preserve manual scroll.
- **Final verification:** Independently rerun 694 tests, required Clippy,
  formatting, diff checks, debug/release builds: passed. Review's seven scoped
  regressions independently passed. Native GNU release: 2,908,288 bytes.
- **Coverage:** Final 694-test LLVM JSON is target/phase2-coverage.json.
  Initial new mapping helpers 511/524 = 97.52% code regions; changed round-1
  helpers 190/200 = 95.00%. Before final seven fixes, cumulative Phase 1+2
  added executable production lines were 1792/2040 = 87.84%. These source scopes
  exclude unit-test modules and merge duplicate generic/closure regions;
  whole-repository coverage is not mislabeled as new-core coverage.
- **PTY evidence:** Repeatable isolated decoded-screen fixture validates real
  bracketed `q` rejection (no quit), exact Unicode/YAML paste/save, single undo
  and redo, long-line End tracking, Alt+W and actual 60x20/120x40/80x24 resize,
  fake native clipboard success and failure to unconfirmed OSC52/manual copy,
  editor focus restoration, no mouse/icons/watcher/terminal, zero child exit,
  exact termios state, alternate-screen exit and bracketed-paste disable.
  Harness is .superpowers/sdd/plan/phase2-pty-fixture.py with isolated pinned
  pyte 0.8.2/wcwidth 0.2.13, not a production dependency or the complete Phase 12
  browser acceptance suite. No native test-runner clipboard was touched.
- **Fixture gotcha:** Ratatui differential output need not contain contiguous
  status text. Decode terminal screen state rather than grep raw ANSI bytes;
  markers may also straddle soft wraps. Early fixture failures were these
  assertion assumptions, not application paste failures, and were corrected.
- **Limits:** Wrapped counts/mapping still scan loaded lines; no whole-document
  glyph/row allocation. Indexed layout and syntax-prefix caches remain Phase 6.
  Embedded terminal protocol, browser acceptance and platform builds remain
  later gates. Phase 2 is complete, not the full track.
- **Next:** Stable document IDs/store, safe preview lifetimes, and independent
  focus/overlays without dropping dirty buffers.

## 2026-10-01 - Phase 3 Task 1 prerequisite: Bounded safe loads

- **Gap found:** Safe load_document uses read_to_end without a byte budget.
  A preflight metadata check or bounded read followed by from_file still permits
  unbounded allocation when the file grows. The document worker stopped before
  editing outside its boundary; plan revision 3 records this prerequisite.
- **Implemented:** load_document_bounded(path, max_bytes) returns exact bytes
  and FileRevision from the same descriptor with existing symlink, regular-file,
  identity and pathname checks. Metadata rejects known oversize files early;
  the reader consumes at most the limit plus one detection byte. Typed TooLarge
  carries path and byte limit. Old load_document remains API-compatible.
- **TDD:** Oversize-file refusal and actual reader-budget tests failed before
  implementation. Exact-limit/empty-file revisions and symlink/directory
  refusals also pass. Four focused tests pass; independently run full suite
  698 passes and required Clippy passes. Formatting initially failed on one
  assertion, then cargo fmt, fmt/diff and focused checks passed.
- **Scope:** Coordinator alone edited save.rs. Document worker resumed with
  the bounded API; store/focus integration remains in progress. This does not
  claim every legacy save/overwrite revision capture is now memory-bounded.
- **Follow-up:** FileManagerTUI-cen.6.6 / Phase 6 Task 6 must bound legacy
  save-validation and overwrite revision reads and rerun gates before Phase 6
  closes. Current file-save safety is not a memory-bound claim.

## 2026-10-01 - Phase 3 Task 1: Stable document ownership

- **Implemented:** Opaque checked, process-wide DocumentId allocation;
  DocumentStore owns each EditorState. Canonical lexical aliases deduplicate,
  while symlink components are refused before canonicalization. Dirty or edited
  documents never silently evict. New admission succeeds before removing a clean,
  untouched temporary preview; failed admission retains the prior active ID.
- **Pinning:** A read-only content_revision accessor observes every editor
  mutation, including unflushed groups and undo/redo. Sticky first-edit pinning
  therefore survives save/undo before any store operation. No buffer hashes or
  clones are needed. Direct public buffer assignment is not the mutation API.
- **Policies:** Bounded consistent load assigns its exact FileRevision to the
  editor and preserves constructor line endings/normalization. Typed failures
  reject S3, directories, binary controls, invalid UTF-8, known read-only modes,
  and byte/line limits (defaults 10 MiB/100k). No admission write or guaranteed
  writable-mount claim. Explicit external-change marking is sticky, retains text
  and loaded revision, and does not poll disk from getters.
- **Lifecycle:** Per-document cursor/viewports/wrap/find/selection/clipboard/
  history/saved state persist unchanged across activation. Guarded close refuses
  dirty buffers; explicit discard/rekey/reload acknowledgment belong to Task 4.
  Opening path/title remain stable, independent of App preview and Save As until
  that later rekey integration.
- **TDD/verification:** Nine compiling contract-stub tests failed at runtime.
  Eleven document tests plus accessor test added; independently run 710 total
  tests and required Clippy/fmt/diff pass. Production core 200/213 = 93.90%
  executable-line coverage. Native release remains 2,908,288 bytes.
  Task review passes specification and quality with no actionable findings.
- **Next:** Replace App's single editor owner with explicit workspace documents,
  panel focus and overlay routing. No focus/UI integration is claimed yet.

## 2026-10-01 - Phase 3 Task 2: Integration execution refinement

- **Incomplete attempt:** The initial integration worker returned without
  delivered source/test changes. It reproduced watcher disabling on edit, then
  removed that temporary fixture. The coordinator verified the unchanged
  710-test baseline. This is an execution shortfall, not a repository blocker
  or completion evidence.
- **Focus primitives:** Added independent panel focus, modal-only InputOverlay
  (no Edit variant), explicit input targets, originating document IDs and
  bounded nested return contexts. Three genuine behavior reds covered routing,
  nested modal targeting/restoration and the eight-overlay cap. All three now
  pass; full 713 tests, required Clippy/fmt/diff pass.
- **Sequential recovery:** An ownership-only App migration runs before the
  separate input-routing cleanup. Old global adapters may exist only during
  that intermediate stage; Task 2 remains in progress until they are removed,
  modal targets/focus are integrated, and complete tests/review pass.

## 2026-10-01 - Phase 3 Task 2a: App ownership migration

- **Implemented:** App.workspace.documents is now the sole editor owner;
  App.editor_state and watcher_before_edit are removed. App.editor returns a
  borrowed active editor; DocumentStore.active_mut enables disjoint field
  borrowing. Admission uses configurable bounded byte/line guards. Reactivation
  and leaving editor scope retain each document and its history/unsaved bytes.
  UI title uses the owned document path instead of transient preview path.
- **Reload:** reload_active replaces only after successful bounded admission;
  a failed explicit reload retains the editor. This is a conflict-resolution
  action, never automatic activation or watcher reload.
- **Watcher:** Enter/exit leave enabled/disabled preference unchanged. Exact
  owned opening/save path notifications mark external change without replacing
  text or source revision. Tree refresh remains independent.
- **TDD/verification:** Two runtime reds proved exit previously lost the editor
  and A/B/A previously reconstructed A. Three tests added, including stable
  owned-path UI title. Independently rerun 716 tests and required Clippy/fmt/
  diff checks pass. Existing fixtures now admit temporary owned documents.
- **Not finished:** Global AppMode::Edit, focused_panel and active-only modal
  targeting remain interim adapters. Quit remains unconditional in this stage.
  Task 2 is not complete. Stage 2b must remove the adapters, capture modal origin
  IDs, route by focus and guard all retained dirty documents before quit.

## 2026-10-01 - Phase 3 Task 2b: Explicit focus integration completed

- **Implemented:** Workspace.focus is the sole panel/modal state. AppMode and
  FocusedPanel are compatibility type reexports only. Global Edit, App.mode and
  App.focused_panel are removed. Editor, tree, preview and terminal input route
  independently; retained find/paste/mouse state cannot steal another panel's
  input. Owned document presentation is independent of input focus.
- **Modal identity:** Origin targets survive save/conflict/overwrite/Save As/
  reload and nested copy. Return contexts reactivate the same owned document,
  without loading or moving buffers. Reload(id) replaces only after bounded
  admission succeeds. Follow-up dialogs replace their workflow, not its origin.
- **Keys/quit:** Ctrl+Arrow changes panel focus even with find active. Tree Tab/
  Right returns to a retained editor; editor Tab still indents/find-switches,
  and terminal Tab remains PTY input. Quit guards all retained dirty documents;
  save/cancel/no-save returns to the workspace without silently discarding or
  exiting. Full multi-document Save/Discard choreography remains Task 4.
- **Review fixes:** OperationComplete previously popped nested manual-copy
  overlays and restored stale Progress, skipping copy cleanup. retire_progress
  now addresses only its own workflow, preserving copy payload/capture and
  unrelated modal origins. Editor status now truthfully advertises Ctrl+Arrow,
  not Tab focus. Four genuine reds and seven regressions covered these fixes;
  scoped re-review passed seven tests and the original P2 reproduction.
- **Final evidence:** Independently rerun 739 tests, required Clippy/fmt/diff,
  rebuilt debug and genuine decoded-screen PTY fixture: passed. Native GNU
  release build passed, 2,908,296 bytes. Raw full coverage is
  target/phase3-focus-coverage.json. Production FocusState methods/regions have
  100% execution coverage, not a branch-coverage claim.
- **Next:** Visible document tabs, direct tree/Quick Open opening, temporary
  preview distinction, pin/activate/reveal and keyboard open-document navigation.

## 2026-10-01 - Phase 3 Task 3: Tabs and direct navigation

- **Implemented:** Stable document summaries, bounded active-visible overflow,
  duplicate-basename disambiguation, matching tab mouse geometry and code row.
  Tree/Quick Open Enter opens directly; Alt+Enter/F2 keeps secondary actions.
  Alt+O/P/B/N/R provides list/pin/previous/next/reveal without mouse.
- **Review fix:** Unsupported single-click previews were hidden behind pinned
  editors after restoring Tree focus. Explicit center presentation now chooses
  selected preview independently of Workspace input focus. Eight clean/dirty
  binary/notebook/read-only/large cases retain the owned document and history;
  reactivation and mouse routing follow actual rendered content. Scoped re-review
  approved the correction. No global Edit input flag was added.
- **Evidence:** Independently 755 tests, required Clippy/fmt/diff/debug gates and
  full LLVM run pass. Tab production regions: 207/207 executed. Native GNU release
  remains 2,908,296 bytes, not a static-musl or cross-platform result.
  Isolated decoded-screen PTYs cover paste/save/undo/resize/copy fallback, direct
  and secondary Quick Open, dirty A/B retention, cycle/list/reveal, exact save,
  controlled /bin/sh output and verified child reaping. Actual SGR mouse proves
  binary single-click and Enter fallback display correctly; termios restored.
- **Fixture gotcha:** Direct Enter replaces the old Tab/e opening assumption.
  Keep exact file-byte/undo assertions and account for the new tab row instead
  of weakening assertions after navigation changes.
- **Next:** Full document-scoped Save/Discard/Cancel lifecycle and path rekeying.

## 2026-10-01 - Phase 3 Task 4 prerequisite: Known rename revisions

- **Implemented:** `FileRevision::matches_after_known_rename` preserves exact
  bytes, mtime, inode/device, mode/owner/group/link count and ignores only ctime.
  Non-Unix comparisons fail closed without a filesystem identity.
- **TDD:** Two genuine assertion reds with the conservative full-equality shim,
  including an actual filesystem rename. Three focused tests pass, covering
  deterministic ctime changes, all retained metadata checks and inode replacement.
  Independently 758 tests and required Clippy/fmt/diff checks pass.
- **Policy:** Use only after a known successful owned rename; callers must not
  clear pre-existing conflict flags or baselines. Revision validation remains
  advisory and trusted-parent limitations are unchanged. Ordinary save comparison
  is not relaxed. Task 4 remains in progress; worker resumed with this API.

## 2026-10-01 - Phase 3 Tasks 4–5: Lifecycle and workspace checkpoint

- **Implemented:** Alt+Q closes explicitly; captured-ID dirty close/quit offers
  Save/Discard/Cancel, stops on failure/cancel and checks newly dirty documents
  before quitting. Bound 1,024 dirty decisions without partial work on overflow.
  Esc focus-back stays separate from buffer removal. Save As preflights ownership
  before writing, then rekeys the same editor/ID. File/folder rename and undo
  preserve histories; deleted/changed paths retain buffers and sticky conflict
  flags. Successful explicit disk decisions acknowledge only their origin.
- **Review:** Scoped specification PASS and quality APPROVE, no actionable bugs.
  Reviewer independently passed 26 lifecycle tests. Generic clipboard cut/move
  was not redesigned; it does not gain transactional owned-rename guarantees.
- **Final gates:** Independently 784 tests; required Clippy/fmt/diff and full LLVM
  run passed, target/phase3-coverage.json. Actual lifecycle production-method
  coverage is 268/299 = 89.63%; including save/reload integrations, 456/498 = 91.57%.
  Native GNU release remains 2,908,296 bytes. Cross-platform and browser checks
  remain future mandatory evidence, not claims from native fixtures.
- **PTY:** Prior paste/resize/copy, two-dirty-document navigation/shell cleanup,
  and SGR unsupported-preview fixtures passed again. Added an isolated 80x24
  lifecycle fixture: close/cancel retains A, quit saves A, external B conflict
  halts quit, reactivation retains dirty B, explicit B-only discard leaves
  external disk bytes unchanged, and terminal flags/modes restore.
- **Limits:** Advisory fingerprints and stable trusted parents remain unchanged.
  Coarse/late watcher notifications can conservatively flag own saved/renamed
  paths again. Non-Unix rename comparison fails closed. Phase 6 Task 6 remains
  mandatory for legacy save validation/overwrite read budgets.
- **Next:** Stable command registry/menu, configurable keymaps and web profiles.

## 2026-10-01 - Phase 4 Task 1: Commands and bounded menu

- **Implemented:** One declaration generates 26 stable typed/string IDs, labels
  and descriptions. Availability is pure in-memory state; menu captures owned
  document/path/panel/presentation, never a tree index. Save labels clearly target
  the retained document, not a selected binary preview. Missing targets do not
  fall back to another buffer. Deferred pane/recovery controls give disabled
  reasons rather than successful no-ops.
- **Menu:** Unmodified F8 or rendered status-button entry. Bounded query paste,
  256-byte UTF-8-safe query, fixed registry search, scrolling, disabled feedback
  and shared render/hit rectangles. Cancel restores find/history/focus; follow-up
  dialogs retain captured origins and nested copy/progress behavior.
- **Review/gates:** Scoped spec PASS/quality APPROVE with no findings.
  Independently 806 tests, commands17/menu10, required Clippy/fmt/diff/debug pass.
  Fresh full LLVM target/phase4-menu-coverage.json. Actual new-file production
  executable lines: commands133/137, menu141/142, combined274/279 = 98.21%.
- **PTY/artifact:** All four prior isolated PTYs pass. New 80x24 F8 menu fixture
  proves dirty open, literal stable-ID query, disabled activation/cancel with
  unchanged bytes, captured A save while B remains dirty, navigation and shell
  child cleanup. Native GNU release 2,973,832 bytes, below10MB. F8 is temporary,
  not a claim of configurable web transport or actual browser acceptance.
- **Next:** Explicit standard/web keymaps and ordinary shell-key forwarding.

## 2026-10-01 - Phase 4 Task 2: Standard/web keymaps

- **Implemented:** Explicit CLI/TOML Standard/Web profiles, bounded checked
  configuration and transactional live apply, merged command/context overrides
  with unbind support, conflict/portable-encoding validation and registry dispatch.
  Both profiles expose Alt+G prefix routes; Web does not depend on Ctrl+P/T/S/R.
- **Input:** Terminal ordinary q/Tab/Esc/Ctrl+C/A/E/U/K/W forwards, including
  Ctrl+C with selected text. Unreserved Alt chords preserve ESC. Registered
  removed defaults cannot fall through to old handlers or destructive plain
  tree operations. Native editor/find/modal/menu input remains context-scoped.
  Pending sequences use a bounded first-chord deadline and fake-clock tests;
  reset/context changes quarantine one delayed suffix rather than replay it.
- **Review/gates:** Scoped spec PASS/quality APPROVE. Independently835 tests,
  keymap32, required Clippy/fmt/diff/debug and fresh full LLVM pass.
  Actual keymap executable production lines357/359 = 99.44%, excluding tests.
  Raw coordinator evidence target/phase4-keymap-coverage.json.
- **PTY/artifact:** All five existing real-screen fixtures pass. New explicit
  web-profile80x24 fixture uses no Ctrl+P/T/S/R and proves prefix/menu opening,
  two dirty documents, cycle/list/focus/reveal/save, literal q/undo, shell output
  and child/termios cleanup. This is controlled PTY evidence, not browser delivery.
  Native GNU release remains2,973,832 bytes; cross/static/browser gates pending.
- **Next:** Generated help/status labels and actual live settings effects.

## 2026-10-01 - Phase 4 Task 3 review prerequisite: Bounded safe publication

- **Review:** The separate config writer broadened 0600 to 0644, overwrote
  concurrent external changes and replaced symlinks. Task 3 remains open for
  repair and re-review; passing coverage did not establish these guarantees.
- **Prerequisite:** Coordinator added `save_document_bounded`, sharing existing
  metadata/ownership/ACL/hardlink/symlink and exclusive-creation policies. Initial
  and final validation now cap reads at captured source length plus one detection
  byte; explicit budget below that length or external growth is a conflict.
  Ordinary save delegates to the same implementation. Output admission stays
  the caller's responsibility.
- **TDD/gates:** Two runtime reds with the old unbounded-delegation shim.
  Four bounded-save tests pass, including final-validation external growth with
  preserved bytes and own-temp cleanup; all21 save tests and878 full tests plus
  required Clippy/fmt/diff pass. Worker resumed same-descriptor config snapshot
  and shared-policy publication repair.
- **Follow-up:** Phase 6 Task 6 remains mandatory for remaining overwrite
  revision capture and caller audit/full gates; no whole-task completion claim.

## 2026-10-01 - Phase 4 Tasks 3–4: Help/settings and accessibility checkpoint

- **Implemented:** Active keymap/registry help and entry/dismiss/status labels,
  transactional typed settings and actual preview/wrap/view-cycle/watch/history
  effects. Configured terminal history trims only oldest rows, rebases/clamps
  selection and leaves the shell/grid intact. README reflects current behavior.
- **Review repairs:** The new config writer widened permissions, overwrote
  external changes and destroyed symlinks. It now captures bounded same-descriptor
  bytes/revision before merge and publishes through shared metadata/revision-safe
  saving, applying live values only after success. Thirteen regressions cover
  permissions/owner/xattrs/ACLs, links, external changes, creation collisions,
  bounds and fault retention. Test-only platform guards were corrected; portable
  failure coverage remains enabled. Final scoped spec PASS/quality APPROVE.
- **Final evidence:** Independently891 tests, writer13, required Clippy/fmt/diff
  and native release3,104,904 bytes pass. Seven isolated PTYs pass, including
  custom F9 entry/F10 dismissal, generated Web help and controlled shell cleanup.
  Raw full LLVM target/phase4-coverage.json; revised Task3 core567/577 = 98.27%;
  writer92/93 = 98.92%, excluding tests. Actual Windows target not installed;
  conditional helper projection is not a Windows build.
- **Build isolation gotcha:** Compiler repro copies must use unique target dirs.
  Sharing a target can reuse another copy's stale test binary. Rejected4/882
  evidence and verified real native13/891 after a forced package rebuild.
- **Next:** Pure adaptive geometry and workspace chrome, disjoint parallel files,
  followed by coordinator-owned UI/focus/PTY integration. Remaining overwrite
  initial read budget and final cross/static/browser checks stay mandatory.

## 2026-10-01 - Phase 5 Task 1: Pure adaptive layout

- **Implemented:** Bounded column/row preferences, pane visibility, temporary
  maximize/restore, compact fallback and actual-geometry splitter resizing in
  `workspace/layout.rs`. No application geometry changed yet.
- **Evidence:** Independent 15 focused tests and 922 combined tests, required
  Clippy/fmt/diff, debug build and navigation/help PTYs passed. Scoped spec
  PASS/quality APPROVE, no findings. Production 221/221 executable lines covered,
  tests excluded; 16,848 geometry combinations include tiny and nonzero origins.
- **Integration contract:** Saved visibility is not rendered availability.
  Compact fallback must not overwrite preferred sizes or visibility. Interactive
  resize starts from actual clamped geometry. Parent and leaf rectangles differ;
  consumers must use the correct content rectangle for mouse and PTY dimensions.
- **Review lesson:** Chrome's passing tests missed duplicate long-name
  disambiguation after clipping and combining-only fallback packing. Both
  independently reproduced findings remain in Task 2's first fix round.
- **Git:** Uncommitted local work; no publication or remote sync.

## 2026-10-01 - Phase 5 Task 2: Reviewed document-aware chrome

- **Implemented:** Pure breadcrumbs with label-only prefix hits, themed/plain
  tabs, full-order navigation and explicit document-aware status priorities.
  Context wins over hints; menu reservations/messages retain priority. Controls
  are sanitized and clipping uses graphemes/display cells.
- **Review fixes:** Duplicate long basenames lost their parent suffix; a first
  delimiter-based fix still collapsed legitimate delimiter-containing parents.
  Fitting now receives the owned basename during layout, preparing bounded tab
  labels without new required public fields. Combining-only breadcrumb labels
  normalize to a one-cell fallback before packing, not after width allocation.
- **Evidence:** Real red/green regressions, final scoped spec PASS/quality
  APPROVE. Independent 11 tab/926 full tests, required Clippy/fmt/diff/debug and
  navigation/custom-help PTYs passed. Fresh new/replaced core401/401 executable
  production lines covered; all-owned514/514 is a separate denominator.
  Neither percentage claims branch coverage or runtime workspace acceptance.
- **Next:** Sequential integration consumes models, preserving one layout owner,
  saved-vs-rendered availability, exact hit/PTY content geometry and no automatic
  shell start from layout preferences. Prepared pane PTY genuinely fails at the
  still-disabled explorer command; later assertions are not yet verified.

## 2026-10-01 - Phase 5 Tasks 3–4: Runtime layout and geometry checkpoint

- **Implemented:** Sole Workspace layout preferences and validated settings,
  effective compact/disabled-pane geometry, exact content/hit/PTY caches,
  breadcrumbs/tabs/status, captured split dragging and portable pane routes.
  Default/restored visible shell placeholders do not start a process; explicit
  open starts missing shells, while hide/maximize/resize preserves a live PID.
- **Boundary repairs:** Bounded menu rendering needed its existing adapter,
  while inherited help/terminal-origin fixtures needed semantic migrations.
  Keep full offset containment, literal input and modal identity checks.
- **Review regressions:** Effective terminal gating used a toggle that also
  restored editor maximization. Updating an empty hidden editor viewport erased
  intentional scrolling. S3 advertised a terminal command its lifecycle refused.
  Minimal integration fixes and three genuine root red/green regressions now
  pass; final scoped spec PASS/quality APPROVE and four independent reproductions
  pass. Pure model approvals remain unchanged.
- **Evidence:** Fresh946 full tests and required Clippy/fmt/diff/debug gates,
  all8 isolated PTYs, full passing LLVM and independently changed production
  core583/627=92.98%. Native GNU release3,104,904 bytes. No actual Windows/musl/
  macOS or browser certification is inferred. Extra all-targets lint debt remains.
- **PTY lesson:** Derive explorer/document geometry from actual decoded cells.
  Ordinary terminal is center-only; maximized terminal spans the viewport.
  Mode setup precedes first usable render, so input fixtures wait for that frame.
  Retain exact saved bytes, shell PID/start-time/stty and restoration assertions.
- **Coverage provenance:** Producer final Phase5 raw core is
  `/tmp/phase5-task3-round1-core-coverage.json`; earlier failing/pre-fix profiles
  are historical. Controller raw `target/phase5-coverage.json`/LCOV and precise
  production-only `target/phase5-core-summary.json` independently match.
- **Next:** Sequential bounded scheduling and transport, then prepared-only
  preview/highlighting, search and watchers. Remaining overwrite capture/
  legacy save caller audit `.6.6` is mandatory before Phase6 closes.

## 2026-10-02 - Phase 6 Task 1: Approved core and transport slices

- **Core:** Generic scheduler has 16 queued jobs/results, two workers, 32
  independent domains, per-job 1 MiB input/8 MiB result budgets, cooperative
  cancellation, stale-result rejection and panic recovery. Core review passed;
  31 new tests, 977 total at that checkpoint, production303/305=99.34%.
  This was an honest partial delivery, not runtime integration.
- **Transport:** One canonical Event FIFO, 128 slots/32 MiB retained payload,
  16 reserved input/completion slots (not reserved bytes); async/blocking
  backpressure, conservative allocation accounting and owned input/watcher
  shutdown. PTY output chunks4096/aggregation65536, session-tagged lifecycle,
  dirty redraw16ms/64events, clean-idle/status deadlines and actual mouse
  geometry. Consumer clipboard dismissal no longer self-enqueues.
- **Late defect:** Ordinary1006 tests/eightPTYs passed, but a valid1MiB paste
  deadlocked synchronous consumer stdin against its own bounded output queue.
  Coordinator independently reproduced the specific assertion in1.07s.
  One owned writer now admits whole FIFO packets without consumer waiting:
  64 packets/2 MiB including active/completion-pending input, max1MiB packet.
  Whole rejection and asynchronous known-prefix failures are visible; no
  silent dropping, replay, unlimited write tasks or hidden pending queue.
- **Ownership:** Linux/macOS readiness checks and shared nonblocking master
  descriptors let native workers stop; shutdown reaps only the owned child
  and joins both handles. Foreign blocking backends may delay joins; native
  Linux evidence is not portable OS-preemption or cross-platform certification.
- **Evidence:** Independently42 transport/1017full tests, requiredClippy/fmt/
  diff/debug gates and all8 original decoded-screenPTYs passed. Fresh unique
  controller LLVM target also passed1017 and8 assertion-equivalent instrumented
  PTYs. Exact changed-production coverage791/888=89.08%; producer793/888=89.30%
  is a separate run with minor branch variation. Controller artifacts:
  `target/phase6-controller-{coverage.lcov,coverage.json,core-summary.json,
  evidence.json}`. Tests and unchanged scheduler excluded from this denominator.
- **Review:** Full transport/redraw slice specPASS/qualityAPPROVE, no findings.
  ParentTask1 remains open: actual App job admission/domains, rejection recovery
  and blocking operation work still need scheduler integration. Preview/render
  I/O is Task2; no global bounded-runtime/FR-7 acceptance claim yet.

## 2026-10-02 - Phase 6 Task 1: Native App foundation checkpoint

- **Delivered partial:** Root/subdirectory snapshots and child counts share
  actual bounded scheduler admission, independent generation/target slots,
  fixed blocking workers and direct main result drainage, not an Event relay.
  Snapshot construction accounts fixed Vec/nested name capacities before
  retention; counts/scans enforce entry/deadline/cancellation budgets.
- **Review findings:** Removing the selected Loading row before collapse
  shifted an expanded sibling into the selected index. Incomplete counts
  cleared TreeNode state but left a copied renderer-facing FlatItem exact
  badge. Coordinator independently reproduced both assertions in0.07s.
- **Fixes:** Reselect the captured directory after row removal, before the
  existing tree action. Clear only matching incomplete badges in place, without
  flattening or losing unrelated selection/multi-selection/scroll. Two genuine
  root reds became green; scoped re-review passed both unchanged original
  oracles and a rejection control, no new findings.
- **Evidence:** Independently16App/1033full tests, requiredClippy/fmt/diff,
  debug and8 originalPTYs pass. Fresh worker LLVM core336/375=89.60% and
  fix7/7 independently recomputed from raw LCOV; this is scope verification,
  not a second controller instrumented build. Native GNU release3,104,920bytes,
  not static-musl/macOS/Windows evidence.
- **Still NOT DONE:** Both summaries, S3 listing/head, clipboard and operations
  remain unmigrated. Native consumer snapshot sorting/page-stat work and broader
  invalidation remain. Task1 stays open until the complete App integration and
  its final review pass.

## 2026-10-02 - Phase 6 Task 1: summaries and prepared native results

- **Delivered:** both directory-summary producers now use the single AppJobs
  pool (shared preview domain, immutable generations, one fixed coalesced
  progress cell, no `dir_scan_cancel` token, no Tokio/blocking spawn). Snapshot
  sorting and first-page preparation moved to the worker (`PreparedSnapshot`)
  with option capture and rejection of stale sort/page/depth; the consumer only
  installs. `handle_dir_count_complete` patches the FlatItem badge in place;
  the inherited "renderer reads the node" comment was false. Deep traversal has
  documented internal caps (2048 stack, 4096 visited, 4096 B/path, 8 MiB stack)
  with cancellation/deadline checks between syscalls and retained symlink-loop
  handling.
- **Process:** the implementer worker was cancelled twice before filing a
  report. The coordinator froze the already-green source and ran the entire
  evidence set itself rather than accept an unverified claim; slice status is
  DONE-for-slice with parent still NOT DONE.
- **Evidence:** 19 new tests (1033 → 1052); 1052 plain and instrumented;
  production Clippy/fmt/diff/debug clean; release 3,170,456 bytes; eight
  original and eight instrumented PTYs pass (instrumented copies differ by
  exactly two substitutions, verified by exact reverse round-trip); fresh
  unique LLVM target gives changed-production 519/552 = 94.02%.
- **Still open:** S3 initial/expand/head, `copy_text_async`,
  `paste_clipboard_async`, final invalidation audit, then Task1 acceptance.

## 2026-10-02 - Phase 6 Task 1: Complete App integration and acceptance

- **Delivered:** the final producer slice migrated S3 listing/expand/head and
  clipboard copy/paste into the single AppJobs pool with direct main drainage.
  Ruling 18 added an additive interrupt/budget/partial-result seam on the
  recursive copy/move path (existing callers preserved; partial destination and
  completed entries reported honestly). Ruling 19 added immutable `WorkflowId`
  frames and identity-targeted retirement, so a delayed older completion can no
  longer dismiss a newer nested Progress workflow. Invalidation is covered by
  `invalidate_search_cache`, `reconcile_job_root` and `retire_tree_jobs_under`;
  the legacy consumer-side snapshot sort is explicitly narrowed and has no
  production sender.
- **Evidence:** 1,089 unfiltered tests; Clippy/fmt/diff clean; release
  3,104,920 bytes; eight original and eight instrumented PTY fixtures pass with
  exact two-substitution provenance; twenty clipboard and twenty transfer
  repetitions; fresh final LLVM target gives 1,090/1,161 = 93.88% changed
  production (coordinator re-extraction 1,091/1,162 = 93.89%, a one-line
  app.rs `cfg(test)` boundary difference).
- **Review:** final scoped review PASS/APPROVE with no P0/P1/P2 findings; it
  independently re-ran the suite, gates, six of eight PTYs, the manifest
  round-trip and the coverage scope. Two non-defect observations recorded
  (copy preflight charges `result_bytes` while `submit` still enforces
  `job_bytes`; the shutdown drain invariant holds for the audited paste paths).
- **Outcome:** Phase 6 Task 1 is accepted and closed. Remaining Phase 6 work:
  preview/highlighting (Task 2), search (Task 3), path selections/watcher polling
  (Task 4), checkpoint (Task 5), Task 6 `.6.6` legacy overwrite/load bounds, and
  the separate all-targets Clippy debt `FileManagerTUI-8sy`.

## 2026-10-02 - Phase 6 Task 2: Preview/highlighting off the render path

- **Delivered:** new `src/highlighting.rs` with immutable `PreviewKey`
  identities, worker-side preview loading/classification on the existing AppJobs
  pool, checkpointed syntax caches, and a thread-local render-I/O probe wired
  into the real loaders. Rendering consumes prepared runs only; unprepared lines
  render an explicit pending style. Both `+`/`-` keys and the preview view-mode
  command route through versioned preview jobs instead of reading on the input
  thread (review round 1 caught that they still did).
- **Review:** initial scoped review REQUESTCHANGES (P1 input-path reads + five
  P2s), fix round 1 addressed all, confirmation found the checkpoint-charge test
  insensitive, fix round 2 (coordinator) made the charge and eviction tests
  genuinely defeating. Final verdict PASS/APPROVE; `.6.2` closed.
- **Accepted narrowed boundary:** syntect `ParseState`/`HighlightState` are not
  `Send`, so editor syntax preparation is a bounded main-loop step (256
  lines/step, 20 000-line and 4 MiB per-document caps, 32 MiB aggregate with
  eviction) that never runs inside `render` or input handlers; going fully
  off-thread needs a `Send` parser engine or worker-local parser registry.
- **Evidence pattern worth reusing:** reconstruct the reviewed state from the
  prior patch and hash-check it before producing a fix-only delta; capture the
  red by temporarily removing the guarded behavior, then restore byte-identical
  and re-run green.
- **Review (PASS/APPROVE, no P0/P1):** one real P2 regression fixed — the
  prepared-snapshot status line had dropped the legacy pagination hint. A root
  red (`left: "Directory loaded: 3 entries"`) drove restoring
  `📂 Loaded N entries (showing first P)`; the other two P2s are a documented
  pre-existing expanded-empty failure state (not a regression, not waived) and
  an informational sanctioned dead legacy arm. Post-fix: 1053 tests, gates,
  eight original plus eight instrumented PTYs, fresh coverage 521/554 = 94.04%.

## 2026-10-02 - Phase 6 Task 3: Incremental filename and project content search

- **Delivered:** `src/search.rs` (resumable literal content search and filename
  indexing with explicit cursors, generation identity, judicial exclusion by
  directory component, NUL binary skip, dev/inode loop protection, name-sorted
  FIFO traversal) and `src/components/content_search.rs`; filename indexing moved
  off the input path onto the single AppJobs pool with incremental continuation
  and a synchronous fallback only when the prepared pipeline is off; eight
  configurable search keys with clamping and safe exclusion defaults;
  `navigate_to_hit` reuses a dirty open document instead of replacing it.
- **Review:** first pass PASS/APPROVE with four P2s; two were genuine defects
  (CRLF column mapping because the stored byte counts the `\r` the editor buffer
  strips, and a status-honesty gap where a refused continuation showed a bare
  match count). Fix round 1 fixed all four and the confirmation review verified
  each by restoring the defect and capturing the red (`left: 1, right: 0` for
  CRLF; `0 matches`/`0 results` for the status arms) before hash-verified
  restores.
- **Lesson:** when a widget derives its own display text, a status field that is
  only refreshed on one path silently shows a stale/bare value; route the text
  through one status function and assert the rendered string, not just the flag.
- **Pre-existing debt found by review, filed not waived:** `FileManagerTUI-5yf`
  (a directory listing larger than `max_pending` re-lists and aborts on every
  continuation, returning zero results) - Ruling20 defers the fix to the Task 5
  checkpoint sweep; Phase 6 cannot close until `.6.5` accounts for it.

## 2026-10-02 - Phase 6 Task 4: Path selections and document-aware watcher polling

- **Delivered:** path-keyed single/multi selection that survives refresh, sort,
  pagination and re-scans (`TreeState::selected_paths`, `flatten`/
  `restore_selection_by_path`); `WatcherMode { Event, Polling }` with
  `watcher.poll_interval_ms` (250..=60000, default 2000), unknown spellings ->
  Event, settings round-trip; a `PollingWatcher` with an injected `pump` clock and
  test-constructed snapshots that repairs lost/coalesced events and
  deleted/re-created roots; document change tracking independent of the tree
  refresh policy; evidence-based self-write suppression.
- **Review:** PASS/APPROVE, but the review earned its keep twice - it showed the
  self-write marker could be consumed by a genuine external write, and then that
  the "content" component of the replacement identity was inert (the bytes were
  read and discarded, so only length was compared). Both were fixed with
  reproduced reds; the final identity is length+mtime+a 64 KiB FNV-1a digest
  computed from the in-memory published bytes.
- **Lesson:** a guard is only as strong as the observation that defeats it. Twice
  in this slice a test passed with its intended safety term removed (content
  clause, then digest term) until a reviewer deleted the term. Always stage the
  defeat check before believing a suppression or identity check.
- **Accepted semantics (Ruling21):** the in-app toggle gates tree refresh only;
  change detection stays live for open documents. Every user-visible surface must
  say so.

## 2026-10-02 - Phase 6 Task 5: Checkpoint, and the search stall that came back

- **Delivered:** `FileManagerTUI-5yf` fixed (resumable `DirListing { offset }`
  listings; reaching `max_pending` is a pause, not a cap), a composite checkpoint
  (wide-directory completion, generation isolation under slow flood, worker panic
  isolation, render-never-loads, dirty-redraw coalescing, bounded transport
  flood), and the Phase 6 closure audit with named gaps.
- **The review earned its rejection twice:** the first fix made the bead's shape
  pass but livelocked whenever the pending set was full of *non-empty*
  directories and the file queue was empty - popping did not free capacity and
  the bound check refused before consuming anything. Worker path: a 2 s CPU burn
  then an honest-looking zero-result `(incomplete)`. Fallback path: an indefinite
  hang. Every shipped regression used empty subdirectories, which drain the
  pending set, so 1203 tests passed over a livelock.
- **Lesson:** test the shape that stresses the mechanism, not the shape that
  demonstrates the feature. For bounds and pauses, the adversarial case is the
  one where nothing can free capacity.
- **Second lesson:** a soft bound is fine; claiming it is hard is not. The
  retained set overshoots to roughly `2 * max_pending + children_per_dir` once
  `visited` saturates; charge the real length and document the slack instead of
  pinning an invariant the code never promised (Ruling22).

## 2026-10-02 - Phase 6 Task 6: Bounding legacy save reads

- **Delivered:** `load_document`/`load_document_with_bound`/`read_document_bytes`
  all take an explicit budget (no unbounded branch), a new
  `overwrite_document_bounded` plus `save_confirmed_overwrite_bounded` route
  explicit overwrite capture through the same guarded publication, and growth now
  maps to `Conflict` with the original bytes and dirty buffer intact. 9 new
  deterministic regressions; 1216 tests, coverage 38/38 = 100%.
- **Lesson:** "no unbounded read remains" needs a stated scope. The review
  accepted the save/overwrite claim but found two unbounded whole-file reads in
  the preview path (notebook loader with no size guard, and a TOCTOU window after
  a metadata check) and filed them separately. Absolute claims invite a
  counterexample; scope them and file what falls outside.

## 2026-10-02 - FileManagerTUI-8sy: All-targets Clippy debt

- Cleared 60 test lints (field_reassign_with_default, bool_assert_comparison,
  redundant_field_names, needless_borrows, unnecessary_sort_by) across 10 files
  with no production line changed and no `#[allow]`. 1216 tests, both Clippy
  gates clean, eight PTYs pass. Lesson: lint rewrites are safe only if the
  assertion semantics are preserved; verify polarity mechanically (54
  conversions), not by eyeballing a sample.

## 2026-10-02 - Phase 7 Task 1: Session serialization

- **Delivered:** `SessionRecord` (workspace root, active document, per-document
  cursors/viewport, layout, recent files; nothing resembling processes, terminal
  buffers or executable trust), per-workspace keying with embedded root
  validation, typed refusals (Corrupt/TooLarge/UnknownSchema/FutureVersion/
  UnsupportedVersion/TooManyDocuments/WorkspaceMismatch/Unavailable), atomic
  create_new+rename writes with cleanup, clamp_viewport on restore, pinned-only
  capture and a visible notice when no private state directory exists.
- **Lesson (again):** a record parsed from disk is untrusted input; clamp every
  derived field before it reaches arithmetic on the render path. The review's
  overflow repro (`editor.rs:364`) was reachable from a syntactically valid file,
  not just a corrupt one.
- **Lesson:** enforce a cap symmetrically - on capture and on load - and make the
  negative serialization assertion structural (key whitelist), because substring
  checks miss any differently-named field.

## 2026-10-04 - Phase 7 Task 2: Bounded private recovery snapshots

- **Delivered:** `RecoveryRecord`/`RecoveryStore` keyed by workspace root plus
  canonical document path plus revision identity (exact size and ns mtime), atomic
  owner-only writes under `<state>/recovery/`, count (1..=1024, default 32) and age
  (1 hour..=100 years, default 7 days) retention that deletes only strictly
  matched `snap-<16 hex>-<16 hex>-<16 hex>.json` files it owns, typed refusals
  (corrupt/too-large/unknown/future/unsupported/unsafe/disabled/disk-changed/
  document-not-open/unavailable), restore into a dirty document that never writes
  the original, explicit discard/clear and a snapshot throttle.
- **Lesson:** a numeric config bound needs both ends. The missing upper clamp let
  `u64::MAX` wrap `as_secs() as i64` negative and delete every snapshot; a fresh
  record returned 0 on `load_all`. Clamp at the config accessor and again in the
  policy so a directly constructed extreme `Duration` is safe too.
- **Lesson:** if a sibling module (`session.rs`) has an owning test for atomic-write
  cleanup, port it. Its absence here was invisible to 16 green tests until the
  reviewer defeated the cleanup line.
- **Lesson:** deletion scope should be the exact name shape the app writes, not a
  prefix; `clear()` using `snap-` could delete a same-owner foreign file that
  `discard`/`prune` would never touch.
- **Review practice:** symlink rejection was defense-in-depth - removing only the
  explicit `is_symlink()` checks did not turn the symlink test red because
  `is_file()`/`is_dir()` are false for symlinks; the test only went red once the
  mode/regular-file guards were also removed.

## 2026-10-04 - Phase 7 Task 3: Startup restoration and recovery commands

- **Delivered:** startup session/recovery wiring in `main.rs`, a per-document
  recovery prompt (`DialogKind::RecoveryPrompt { document, remaining }`) with a
  bounded re-offer pass, restore/discard/clear commands and enable/disable, live
  settings sync through `set_recovery_enabled`, throttled snapshots outside
  render, bounded shutdown flush with visible failures, and visible notices for
  missing roots. 1272 tests, both Clippy gates, fmt, eight PTYs, coverage
  295/336 = 87.80%.
- **Lesson:** a settings entry that only writes config is a silent no-op for
  anything the running app reads from live state. Route the apply path through the
  same entry point the commands use so the two surfaces cannot diverge
  (`task3_applying_settings_disables_recovery_live_not_only_after_restart`).
- **Lesson:** user-visible counts must count what the operation acts on. The
  prompt counted retained records while the pass offered documents, so one
  document with several snapshots overstated the remaining offers; the disclosure
  now counts distinct unhandled document paths and says "documents", pinned by a
  3-records/2-documents test.
- **Lesson (evidence method):** changed-production coverage must use exact-line
  matching against the baseline. A whitespace-insensitive matcher inflates the
  changed set on repeated-line-heavy files (`app.rs` 277 vs 176) because identical
  brace-only lines scatter the alignment; the coordinator's first extraction read
  57.6% where exact-line methods read 87.8%.
- **Review practice:** after every fix round, re-derive the defect against the
  current tree. Both Task 3 confirmation passes did (R1-R4), which is what makes
  each guard provably load-bearing rather than merely present.

## 2026-10-04 - Phase 7 Task 4: Recovery checkpoint

- **Delivered:** two crash/restart fixtures with isolated state directories and
  the closure audit for FR-8/AC-3/AC-12; a genuine capture gap was found and
  fixed - the throttled snapshot pass ran only on input-driven loop iterations,
  so an idle dirty document was never captured (`Timeout: idle snapshot capture`
  3/3). The main loop folds a bounded strictly-positive wait into its existing
  timer (`SnapshotThrottle::remaining`, `App::recovery_snapshot_wait`), measured
  at ~1% idle CPU with a dirty document, with no wake when disabled or clean and
  the pass outside render. 1275 tests, all ten fixtures, coverage 32/36 = 88.89%
  unit / 35/36 = 97.22% merged.
- **Lesson:** a fix can change *when* side effects happen, and a fixture can
  depend on their absence. Making the first snapshot immediate meant a record
  existed during the Phase 4 menu fixture, whose sentinel query was the literal
  `recovery.clear` - which Phase 7 had turned into a real enabled command. The
  Enter activated it, closing the menu, and the queued Esc opened the Leave
  Editor modal the later F8 step landed on. Acceptance fixtures must not use
  plausible future command ids as inert sentinels.
- **Lesson (process):** a worker's "pre-existing failure" claim is a hypothesis,
  not evidence. The A/B here (revert only the fix on the same binary: pass 2/2 vs
  fail 6/6) plus a screen dump at the failing step located the true cause in
  minutes, and the confirmation review reproduced both directions from scratch.
- **Review practice:** when the root cause is wrong in a report, correct the
  report rather than only the bead note; the confirmation called the correction
  "required, not optional" because an accepted report otherwise carries a false
  claim forward.

## 2026-10-04 - FileManagerTUI-8u3 and Phase 8 Task 1: bounded reads and a read-only Git backend

- **8u3:** preview reads now open once and read at most budget+1 bytes from the
  same descriptor; the notebook path takes an explicit 5 MiB budget; head/tail/
  shebang reads are bounded; `max_editor_bytes` is clamped to 256 MiB with a
  compile-time fit assert. Lesson repeated from Task 6: a metadata check followed
  by a read is not a bound, and "no unbounded read remains" needs a read-by-read
  bound table, not a claim.
- **Phase 8.1:** read-only Git is enforced by resolving the production argument
  vector through the whitelist at *runtime* (typed refusal, no spawn). A
  debug-only assertion and a unit test that both live outside release builds are
  not enforcement; the confirmation proved the difference by building release
  with a write verb injected and showing no process spawned plus a reached
  refusal branch. If a guarantee matters in production, its guard must compile
  into production.
- **Review practice:** the confirmation's stderr-deadlock hypothesis was
  withdrawn after OS-level probes; the worker had already disproved it with its
  own probes and pinned the real behavior. A reviewer that reproduces the
  scenario and withdraws is worth more than one that restates a plausible claim.
- **Fixtures are code:** an acceptance fixture's inert sentinel became a real
  command (Phase 7's `recovery.clear`), and a load-sensitive 10 s deadline failed
  1/40 runs. Sentinel queries must be impossible command ids, and fixture waits
  should be barrier-based; when a fixture fails, A/B the suspect change on the
  same binary before believing any "pre-existing" claim.

## 2026-10-04 - Phase 8 Task 2: Git indicators

- **Delivered:** color-independent ASCII markers (U/M/S/?) with theme tinting
  only, directory aggregation from the in-memory snapshot, a reserved status
  branch budget, startup/FsChange-only background refresh with stale-generation
  and prior-workspace refusals, S3 exclusion, and a live `[git] enabled` setting
  with `--no-git`. 1325 tests, eleven PTYs, coverage 218/226 = 96.46%.
- **Lesson:** a guard is only as good as its state reset. `GitState::begin`
  updated the root but kept the retained snapshot, so a root change could render
  the old repository's state until the new result arrived; unreachable today, but
  the fix (clear on root change) is being folded into the checkpoint with a red
  rather than filed, because it sits inside the guard chain this phase built.
- **Lesson (test quality):** the plan's requirement was a substring assertion
  (`tree_screen.contains("M")`), which a path or temp-dir name can satisfy by
  accident. The delivered tests assert the marker on the changed file's own row
  plus a negative control after live disable; when a fixture "strengthens" a
  substring test, read the diff and check for the negative control - that is the
  difference between strengthening and softening.

## 2026-10-04 - Coverage extraction: exclude test modules by attribute, not name

- The Phase 8 Task 3 fix round exposed a flaw in the coordinator's coverage
  extractor: it excluded test code by looking for `mod tests`, which misses
  `#[cfg(test)] mod transport_loop_tests`/`keymap_cli_tests` and modules that are
  interleaved *before* production code (`src/main.rs` has test regions at lines
  194-637 and 769-831 with `fn main` at 835). A name-based exclusion counted 55
  changed instrumented lines in `main.rs` where the true production set is 7; a
  "cut at the first `#[cfg(test)]` attribute" heuristic then wrongly zeroed the
  file. Region-based exclusion (scan from each `#[cfg(test)]` attribute to the
  module's closing brace) reproduces the implementer's numbers.
- Direction of the error matters: including test lines inflates the denominator
  (deflates the percentage, conservative), while cutting at the first attribute
  *deletes* production lines from the denominator (could inflate the percentage
  and hide an uncovered line). Use region exclusion, and check the file's
  structure before trusting any heuristic.

## 2026-10-04 - Phase 8 Task 3: the watcher can feed itself

- **The defect only appeared at runtime.** `git status` opens working-tree
  directories to read repository state; notify 7 subscribes to OPEN and
  debouncer-mini maps those opens to a generic change event, so the app's own
  refresh produced the `FsChange` that requested the next refresh - a
  self-sustaining loop at roughly 25 spawns/second, invisible to unit tests
  because each individual step is correct. The fix is a 750 ms coalescing floor
  plus a bounded deferred wake, and the runtime fixture must discriminate
  (un-coalesced: 171 invocations vs a budget of 32; coalesced: 13).
- **Lesson:** when a watcher triggers work that itself touches the watched tree,
  assume a feedback loop until a runtime count says otherwise. A fixture that
  only checks "refresh happened" cannot see this class; the check has to bound
  the *rate*.
- **Test fidelity: a test that recomposes production logic does not own it.**
  The first loop-fold test built an equivalent wait locally, so deleting the
  production fold left it green; the red only appeared when the accessor itself
  was neutered. The fix extracted `loop_wait`/`wait_for_loop` so the test drives
  the production expression, and added a structural source-text guard for the
  bypass class a unit test cannot reach (the reviewer neutered the loop itself
  and the guard caught it, while the behavioural test stayed green - exactly the
  division of labour that justifies a structural pin). Residual gap recorded:
  a future differently-named wait primitive would evade the token list.
- **Coalescing semantics: drop vs defer.** A floor that silently drops the
  window's own event leaves the indicator stale until unrelated activity; the
  fix records the deadline and lets the existing bounded timer wake issue one
  follow-up, re-arming the floor so a burst still cannot storm.
- **Coverage basis and reindentation.** Wrapping the loop's `match` reindented
  four pre-existing statements, which a literal diff counts as changed and which
  are uncovered in the instrumented subset: literal basis 78/82 = 95.12%,
  new-semantics basis 54/54 = 100%. Report both, strict basis first, and name
  the reindented lines - otherwise a 100% figure reads as inflation.

## 2026-10-04 - Phase 9 Task 1 fix round: measured oracles and dead-code honesty

- **Asserted-vs-measured reference rows.** The original compare report wrote
  the alacritty row from API knowledge; rebuilding it as real code surfaced two
  corrections a prose claim could never catch: alacritty's DA reply is
  `\x1b[?6c` not our `\x1b[?1;2c` (both VT102-family, not a defect), and vt100
  has no reply channel whatsoever — it consumes `?6n` silently. The harness
  (`emulator-compare/`, `vt100-probe/`) is committed under `.superpowers/` this
  time; `include!` cannot carry `//!` so `build.rs` strips module docs to `//`.
- **Version pins create sibling crates, not conflicts to 'fix'.** vt100 0.16
  requires `unicode-width ^0.2.1`; ratatui 0.29 pins `=0.2.0`. Rather than
  loosen a pin, the vt100 probe lives in its own crate — report both sides.
- **Coverage test parked at the boundary never iterates the loop.** The ED
  mode-1 arm was reachable yet uncovered solely because the existing test
  parked the cursor at row 0 (`0..0`). A dedicated fixture at row 2 plus a
  neuter-to-red control (`0..cursor_row` → `0..0` → fail) earns the coverage
  claim; checking the arm exists is not the same as iterating it.
- **0x0 guards: enumerate the hazard set before testing.** The degenerate
  geometry panic hid in `min(rows - 1)` across nine sites — the fix listed
  every underflow/index site first (already-safe guards named) so the red test
  hit real production panics (`emulator.rs:675`) rather than neutered code.
- **pyte is not a terminal.** pyte 0.8.2 parses the same streams a real
  terminal renders, but has no alt-buffer (1049 ignored) and no reply channel;
  a PTY fixture can only verify byte-stream integrity + the observable halves
  (TIOCSWINSZ → `stty size`, SIGWINCH delivery). Also: fixtures must make the
  PTY *child* emit the stream — `cat` echoes input back as caret notation, so
  writing escapes to cat's stdin proves nothing.
- **`cargo clippy --fix` for unrelated lint debt is fine mid-task when the gate
  requires it.** 20 pre-existing `byte_char_slices` warnings in test code
  blocked the `--all-targets` gate; mechanical auto-fix + retest beats a
  documented-red gate. Zero of them touched the changed file.

## 2026-10-04 - Phase 9 Task 2: reply drain, mode-correct input, lifecycle bounds

- **The reply queue is worthless until something drains it.** Task 1 staged
  `take_replies()` behind `#[allow(dead_code)]` with a documented stall caveat;
  the Task-2 wiring is three lines in `process_terminal` (`take_replies()` →
  `pty.write(&replies)`) — the *test* is where the work went: a real PTY child
  that emits `\x1b[6n` then hex-dumps 6 stdin bytes (`dd|od|tr`) proves the
  reply crossed back, not just that the queue filled.
- **Mode-correct input needs exactly two modes.** DEC mode 1 (DECCKM) → SS3
  `\x1bO*` arrows/Home/End; DEC mode 2004 → bracketed-paste wrap. Both are
  tracked on the emulator (`set_private_mode` arms + RIS reset) and read at
  the input seam (`handle_terminal_keys`, `handle_paste_event`) — the
  observable path is the only honest wiring point.
- **Hyperlink OSC display text IS printable.** `OSC 8;;url BEL text` renders
  `text` — the inertness test asserts the payload is consumed (no replies,
  no commands) while its display text lands in the grid, not that nothing
  prints.
- **`assert!(!timeout)` is not a bounded-wait test.** A `spawn_blocking`
  shutdown measured after `.await` hangs forever instead of failing; wrap the
  join itself in `tokio::time::timeout` so a real regression reports red.
- **Much of Task 2 already existed.** The ordered/bounded input queue, exit
  events, session-keyed output, hide-keeps-child, and restart-on-dead were
  built in earlier phases — the delta was the reply drain + two mode arms +
  pinning tests (9 new: 2 emulator fixtures, 2 handler unit, 4 main-loop PTY
  integration, 1 pty busy-shutdown).

## 2026-10-04 - Phase 9 Task 3 checkpoint: terminal compatibility closed

- **LCOV `SF:` paths are absolute** — the strict-basis script must key by
  `os.getcwd()`-prefixed paths or every diff line silently lands in "no-DA".
- **A 0-hit closing `}` is structural, not a miss.** Report it honestly
  (24/27 hit, 2 doc no-DA, 1 `}` 0-hit) rather than claiming "100%".
- **SIGINT delivery needs ISIG on.** `stty raw` disables it — a raw-mode cat
  receives 0x03 as a literal byte; the interrupt fixture uses `stty -echo`
  (echo off for clean output, ISIG preserved) so ^C kills the foreground
  process group and produces TerminalClosed.

## 2026-10-04 - Phase 10 Task 2: bounded LSP stdio transport

- **A writer thread blocked on `recv()` outlives a killed child.** Shutdown
  joined the writer while its `SyncSender` was still alive in `self` → the
  recv never ended and every real-child test hung >60 s. The fix is dropping
  the sender (`Option::take`) before joining — channel close ends the loop
  and drops stdin → clean child EOF.
- **Bound tests must out-fill the OS pipe, not the queue.** Asserting
  `admitted <= slots + k` is timing luck: the writer drains into a ~64 KiB
  pipe, so tiny messages never reject deterministically. 4 KiB bodies fill
  the pipe in ~16 frames; only the channel's 64-slot bound then caps
  in-flight packets, and `WouldBlock` is guaranteed.
- **`recv()` in a deadline loop defeats the deadline.** `Receiver::recv`
  blocks past the wall-clock bound; test helpers take `recv_timeout`.
- **Channel-close shutdown ordering:** drop sender → kill/wait child → join
  writer, reader, stderr. Any earlier join can block on the live channel.
