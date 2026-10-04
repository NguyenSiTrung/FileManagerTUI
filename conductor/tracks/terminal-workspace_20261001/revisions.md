# Track revisions

## 1 - 2026-10-01 - Plan boundary clarification

Current task: Phase 2 Task 4 (text viewports).

The task requires shared visual-row mapping, but its file list omitted the
existing shared text-coordinate module and the UI viewport setup site.
Added `src/text.rs` and `src/ui.rs` to its sequential ownership boundary.
The requirements, task order, dependencies, and behavior are unchanged.

Ruling: Put shared row/byte/display mapping in text.rs and update viewport
dimensions in ui.rs rather than duplicate widget and mouse calculations.
This follows FR-3's consistent mapping requirement. If wrong, the cost is a
local helper-boundary refactor, with no disk-format or external side effect.

## 2 - 2026-10-01 - Document edit identity accessor

Current task: Phase 3 Task 1 (document store).

Added editor.rs to the sequential boundary for a read-only accessor to the
existing content-revision counter. No editor mutation behavior changes.

Ruling: Reuse the existing mutation generation to recognize first edits even
after undo/save, rather than poll dirty state or hash/clone whole buffers.
This supports FR-4's sticky edit-to-pin policy. If wrong, the cost is replacing
the document's edit-observation hook, not changing stored text or save policy.

## 3 - 2026-10-01 - Bounded revision-consistent document load

Current task: Phase 3 Task 1 (document store).

The document worker identified that the existing safe loader uses unbounded
read_to_end. A metadata guard or preliminary bounded read followed by that
loader cannot enforce the memory bound if the file grows between checks.
Added save.rs as a coordinator-owned prerequisite, not concurrent ownership.

Ruling: Add load_document_bounded(path, max_bytes), reading at most the limit
plus one detection byte from the same validated descriptor and retaining
symlink/path/revision checks. Report a typed TooLarge error without writing.
The old unbounded API remains compatible for existing callers. This implements
the approved large-file guards rather than weakening them. If wrong, the cost
is a local read API adjustment; document bytes and save safety remain unchanged.

## 4 - 2026-10-01 - Remaining legacy revision-read bounds

Discovery during Phase 3 Task 1: existing save validation and overwrite
revision capture still use the legacy unbounded loader. Added a Phase 6 Task 6
under FileManagerTUI-cen.6.6, with mandatory full gates after its fix.

Ruling: Address legacy validation/capture budgets with Phase 6's bounded I/O
work rather than silently call all legacy reads bounded or interrupt document
store ownership. The known revision size can bound comparison; growth must
remain a conflict, not truncated content. If wrong, the cost is moving this
in-scope safety fix earlier, with no permission to discard document data.

## 5 - 2026-10-01 - Document-store integration helpers

Current task: Phase 3 Task 2 (focus integration).

Added documents.rs to the sequential boundary for active mutable access and
other minimal integration helpers. No parallel source ownership is introduced.

Ruling: Let App borrow the one owned editor through DocumentStore rather than
copy/move buffers into a compatibility editor_state field. This preserves FR-4
document identity. If wrong, the cost is changing small borrowing helpers; no
buffer clone, eviction, or new persistence is authorized.

## 6 - 2026-10-01 - Sequential focus-integration substeps

Current task: Phase 3 Task 2.

The first full integration attempt returned incomplete without changes. Split
execution into tested focus primitives, ownership migration, and focus/modal
routing cleanup. The task's requirements, ownership and order are unchanged.

Ruling: Permit a temporary global Edit/focus adapter only at the intermediate
ownership stage, never as Task 2's finished implementation. This reduces a large
fixture/borrowing migration while keeping verified boundaries. If wrong, the
cost is reworking the interim adapter; no task closes and no dirty buffer may
be dropped to make it pass.

## 7 - 2026-10-01 - Known-rename revision comparison

Current task: Phase 3 Task 4 (document lifecycle).

Full revision equality includes ctime, which a successful owned rename changes.
The lifecycle worker cannot safely inspect private bytes and inode metadata.
Added a coordinator-only read-only accessor in save.rs, without overlapping
source ownership or changing the ordinary save equality policy.

Ruling: Compare exact bytes, mtime, device/inode, mode, uid/gid and link count,
excluding only ctime/ctime_nsec after a known successful rename. Without Unix
filesystem identity, refuse the inference. Callers must preserve pre-existing
conflict flags and baselines, and retain visible conflict if comparison fails.
This is advisory reconciliation, not filesystem compare-and-swap. If wrong,
the fallback is explicit Reload/Overwrite after rename, never a silent write.

## 8 - 2026-10-01 - Command-menu rendering and modal integration

Current task: Phase 4 Task 1 (command registry).

Its original file list omitted ui.rs and focus.rs, where a new menu must render
and participate in the existing modal stack. Added those sequential integration
sites without changing requirements, task order, or parallel ownership.

Ruling: Use the existing bounded modal/focus-return mechanism rather than a
second dispatch authority or an unreachable widget. Keep document origins and
eight-frame modal cap. If wrong, the cost is a local menu integration refactor,
not changed save semantics, discarded buffers, or new executable permission.

## 9 - 2026-10-01 - Keymap registry integration

Current task: Phase 4 Task 2 (keymaps).

Added commands.rs to the sequential boundary. Menu entry needs a stable command
ID so configurable bindings can resolve it through the same registry rather
than creating a second special-action dispatch mechanism.

Ruling: Keep keymap resolution and execution separate, dispatching known IDs
through the registry. Context validation rejects unsupported or conflicting
bindings; reserved workspace sequences do not reinterpret ordinary shell text.
If wrong, the cost is a local binding/dispatch adjustment, without new file
writes, executable trust, or discarded documents.

## 10 - 2026-10-01 - Live-setting runtime integration sites

Current task: Phase 4 Task 3 (help and settings).

Added App and command metadata integration, plus terminal state/emulator files.
The existing scrollback setting changes only configuration; the emulator has a
private fixed 1,000-line limit and no update API. These sequential boundaries
are needed to deliver the already-approved actual live behavior.

Ruling: Generate command labels from the active resolver, apply settings through
existing runtime ownership, and add a bounded scrollback update that trims only
old history while preserving the grid/process and clamping stale selection.
Do not restart the shell or replace the emulator in this task. If wrong, the
cost is a local setter/label adjustment, with no discarded document or new trust.

## 11 - 2026-10-01 - Command-menu title binding integration

Current task: Phase 4 Task 3 (help and settings).

The menu widget itself hardcodes F8/Esc in its title. Added command_menu.rs to
the sequential boundary rather than paint over private geometry from ui.rs.

Ruling: Feed active menu-context binding labels to the existing renderer while
retaining the native Esc dismissal hint. Overridden/unbound bindings must not
be advertised. If wrong, the cost is a local title/interface adjustment, with
no changed input authority, buffer mutation, or executable permission.

## 12 - 2026-10-01 - Bounded shared config publication prerequisite

Current task: Phase 4 Task 3 review repair.

Review reproduced config permission widening, lost external edits and symlink
replacement in the separate writer. Shared safe saving fixes those policies,
but its two legacy revision checks read without a bound. Added a coordinator-only
bounded publication API in save.rs rather than weakening the 1 MiB config budget.

Ruling: Both shared publication validation reads use the exact known source
length, plus at most one growth-detection byte, and map growth to conflict.
An explicit budget below the captured length also refuses publication. Preserve
the same metadata/symlink/ownership/exclusive-create policy. The old save API
delegates to this implementation. This pulls part of Phase 6 Task 6 forward;
remaining overwrite capture/caller audits and its later full gates stay mandatory.
If wrong, the cost is a local budget API adjustment, never truncated target data
or a silent unconditional replacement.

## 13 - 2026-10-01 - Parallel layout/chrome test-module exposure

Current tasks: Phase 5 Tasks 1 and 2.

Exposed the empty layout and chrome modules before their independent workers,
moving only compile registration forward from Task 3. No layout/render behavior
is enabled. Coordinator owns mod.rs files; workers keep disjoint source files.

Ruling: Compile both pure modules so real regression tests can run, then
integrate behavior only after both reports/reviews. The approved parallel phase
overrides generic sequential-agent advice, with explicit disjoint ownership.
If wrong, the cost is moving two declarations back, not changed application
geometry, buffer eviction, or concurrent editing of a shared source file.

## 14 - 2026-10-01 - Sequential pane integration boundaries

Prepared during Phase 5 Task 2's review fix, before Task 3 begins.

Task 3's original file list omits registry/keymap adapters and TerminalState's
legacy visible/height-percent ownership. Consuming the reviewed layout while
leaving those fields authoritative would preserve conflicting geometry state.

Ruling: Add commands.rs/keymap.rs, terminal/mod.rs, components/terminal.rs and,
only if required for config/event adapters, main.rs to sequential integration.
The reviewed chrome files may remove pending-integration allowances or expose
a necessary consumed API, not receive an unrelated redesign. Pane preferences
have one owner in Workspace layout; rendered content geometry drives mouse and
PTY/emulator sizing. Persist validated layout preferences through existing safe
settings. Default terminal remains hidden; restored/configured visibility alone
does not start a process. Explicit terminal-open must start an absent shell even
when its pane is already visible. Give pane resize commands portable explicit
workspace-prefix routes, without stealing raw shell keys.

Both pure tasks must approve before runtime changes. If wrong, the cost is
local adapter/settings adjustments, not discarded buffers, shell restart,
external writes or increased executable trust.

## 15 - 2026-10-01 - Bounded menu adapter and inherited help fixtures

Current task: Phase 5 Task 3 NEEDS_CONTEXT.

Coordinator independently reproduced all three reported failures. CommandMenu
still renders against the entire host frame, bypassing the new offset viewport.
Its existing private bounded adapter is the smallest fix. Two inherited fixtures
assume focus can return to a hidden terminal and pane commands remain unimplemented.

Ruling: Add command_menu.rs to the sequential renderer/test boundary and help.rs
for that test migration only. Expose/reuse the bounded adapter, seed a visible
terminal in the terminal-origin fixture, and assert implemented availability/
bindings rather than obsolete Phase 5 disable text. Preserve modal origin,
input literalness, disabled recovery and full offset containment assertions.
Capture these unchanged files in the Task 3 review baseline before edits.

PTY fixtures must derive current explorer/document/content geometry, not a fixed
40% split or full-width ordinary terminal. Keep exact disk bytes, shell identity,
fresh stty evidence and cleanup tests. If wrong, the cost is local rendering/
fixture adjustment, not weakened buffer or process guarantees.

## 16 - 2026-10-01 - Bounded event and PTY producer seams

Current task: Phase 6 Task 1, after approved Phase 5 checkpoint.

The four named scheduler files omit typed Event senders in handler/commands/
watcher/test helpers and the separate unbounded PTY reader/bridge. Adding only
a bounded unused scheduler would not bound actual runtime output or event queues.

Ruling: Extend sequential transport scope to handler.rs, commands.rs,
components/command_menu.rs, ui.rs, fs/watcher.rs and terminal/pty.rs for necessary
sender/producer/test adapters only. Use one canonical bounded runtime transport,
preserve ordered critical/input/PTY events and explicit backpressure, and
coalesce redraws with an injected bounded clock/budget. Do not hide an unbounded
compatibility relay or block the UI consumer waiting on its own full queue.
Preview/highlighting/search migrations remain their later tasks; if scheduler
and transport prove too large, return an honest tested partial delivery.

If wrong, the cost is local transport/API adjustment, not dropped user input,
lost operation completion, shell restart or silently unbounded runtime queues.

## 17 - 2026-10-02 - Bounded App job producer prerequisites

Current task: Phase 6 Task 1 after approved scheduler/transport slices.

App's current jobs call snapshot/shallow-summary helpers without cooperative
budgets, and S3 list/head uses unbounded process output; head additionally owns
a shell pipeline. A finite job queue alone cannot bound those retained outputs
or clean up cancelled child I/O. Existing clipboard input uses a detached
per-tool stdin helper.

Ruling: Permit a focused background/app_jobs.rs adapter if needed to avoid
burying concrete job/payload execution in the generic core or growing App.
Extend the sequential App job slice to fs/tree.rs for a cancellable,
entry/byte-budgeted snapshot helper, preview_content.rs for the existing shallow
summary's budget/cancellation seam only, and s3/backend.rs for bounded,
timed/cancellable listing/head argv execution and owned child cleanup. In app.rs
repair native clipboard helper ownership as needed by finite job execution.
Do not redesign tree selection/watching, full previews/highlighting, S3 caching,
or clipboard policy. Reuse the approved scheduler and transport, tag actual
jobs independently, keep metadata bounded, and recover visible state on refusal.
User operation interruption must not invalidate its necessary partial undo/
completion. Preserve successful earlier boundaries and all eight PTYs.

If wrong, the cost is local worker/API adapter rework, not remote S3 writes,
real clipboard changes, discarded document bytes or unbounded hidden workers.

## 18 - 2026-10-02 - Paste operations partial-result and interrupt seam

Current task: Phase 6 Task 1 final producer slice (S3 delivered and verified as
a tested partial; clipboard copy/paste remain).

`paste_clipboard_async`'s blocking recursive work calls
`src/fs/operations.rs::{copy_recursive, copy_dir_recursive, move_item}`, which
return only `Result<PathBuf>`, create the destination before walking children,
can fail after creating files without returning that destination, and accept no
internal interrupt/budget callback. Wrapping the unchanged calls in the existing
scheduler cannot expose the destination/partial lifecycle on failure or bound
their recursive work, so the accepted paste contract (admission before mutation,
per-operation interrupt, preserved partial paths/undo, never abort-as-exit) is
not deliverable within the previous ownership list.

Ruling: Extend the fill-in paste slice to `src/fs/operations.rs` for a focused,
additive interrupt/deadline/budget plus partial-result seam on the recursive
copy/move path only. Existing public functions keep their current behavior for
existing callers (delegate with an unbounded no-op policy); report the created
destination and the entries actually completed on failure, cancellation or
budget refusal, and never claim a clean result on partial work. Do not redesign
collision resolution, symlink refusal, descendant checks, watcher/selection
policy or unrelated helpers, and add no dependencies/config keys. Keep the two
remaining clipboard producers in the existing AppJobs pool with direct main
drainage.

If wrong, the cost is local operations-helper rework and honest partial
reporting, not unreported user file loss or a silently unbounded hidden worker.

## 19 - 2026-10-02 - Identity-targeted overlay/workflow retirement

Current task: Phase 6 Task 1 final producer slice; Ruling 18 authorized the
operations partial-result seam and four compiled clipboard/paste reds are
recorded in `src/app.rs`.

The paste contract requires progress/modal retirement targeted to the
operation/origin so older work cannot close a newer nested workflow. The
existing `src/workspace/focus.rs::retire_progress()` selects the visible or the
latest nested Progress frame and stores no frame identity, so a delayed older
completion demonstrably dismisses a newer nested Progress frame (coordinator
re-ran the red: expected the newer Progress, observed `CopyOverlay`). Equality of
possibly-renewed overlays cannot supply the required exact identity either.

Ruling: Extend the clipboard/paste slice to `src/workspace/focus.rs` for a
focused, additive immutable overlay/workflow identity and identity-targeted
retirement: Progress frames carry the identity captured when opened; expose
reading that identity and retiring only the matching frame, so App can capture
it at admission and retire its own operation's frame. Preserve existing
`retire_progress` caller behavior (delegate or keep current semantics), the
eight-frame bound, document ownership, nested return contexts and existing focus
tests. Do not redesign routing, panels or documents.

If wrong, the cost is a local focus API adjustment, not closed unrelated
workflows or lost document/focus state.

## Ruling 20: Defer the wide-directory content-search stall to a scheduled fix

The Task3 confirmation review (`25249aac`) proved with an unmodified-engine probe
that a single directory listing larger than `max_pending` aborts the content
traversal with `capped=true done=false files_seen=0`, and that every continuation
re-lists the same directory and aborts identically, so a tree whose root has more
than `max_pending` (>4096 at default config) direct subdirectories yields zero
hits while showing only `(incomplete)`. The coordinator confirmed the mechanism in
source (`src/search.rs::enqueue_children` bound return plus the `run_content`
capped break). The reviewer also reconstructed the pre-fix bound logic and found
the same stall, so this is pre-existing, not a Task3 regression; it is bounded
(no hang, no unbounded memory) and honestly labelled.

Ruling: record it as `FileManagerTUI-5yf` and defer the fix to a scheduled slice
folded into the Phase 6 Task 5 checkpoint sweep, where search responsiveness and
honest incompleteness are verified end to end. The fix must make the traversal
progress (drain queued files and/or resume a partially enumerated directory with
a saved position instead of re-listing and aborting) while keeping the
job-envelope bound, and must add a regression with a directory count above
`max_pending` proving hits are eventually returned with honest status. Phase 6
cannot close until `.6.5` accounts for `FileManagerTUI-5yf`.

## Ruling 21: Accept the Task4 watcher-toggle semantics and require truthful surfaces

Task 4 made the watcher backend stay active for the life of its thread while the
in-app `Ctrl+R` toggle and `watcher.auto_refresh` gate only tree auto-refresh.
The alternative - restoring `pause()` on toggle - would reintroduce the global
suppression of change detection that FR-4/AC-3 forbid for open documents, so the
new semantics are accepted. Help (`src/components/help.rs:403-404`), the toggle
status strings, the settings descriptions and the README keybinding/prose/example
surfaces all state this explicitly, and full disable remains `--no-watcher` /
`watcher.enabled = false`. `FsWatcher::pause`/`resume` are retained as a
documented API (they silence a live backend without tearing down watches and are
driveable through `active_flag`), not a test seam. Self-write suppression is
evidence-based: length, mtime and an FNV-1a digest of the first 64 KiB of the
bytes the save published; the tail beyond that window is explicitly not claimed
verified. If this ruling is wrong, the cost is a settings/help wording change,
not lost change detection.

## Ruling 22: Accept the soft retained-set bound for content search

The Task5 confirmation review measured that the resumable content traversal's
retained set overshoots `max_pending` to roughly `2 * max_pending +
children_per_dir` once nested directories are discovered after `visited`
saturates (about 34 entries at `max_pending = 8`, about 4033 at the default
4096). The overshoot is bounded by shape rather than tree size, is charged
honestly by `search_cursor_bytes` (which reads the real retained length), and
stays far inside the default `job_bytes`, so it cannot cause a refused
submission, a dishonest status or unbounded growth. A hard bound is not
adopted because the only designs that keep progress with a full pending set and
an empty file queue are the cap-and-report behavior that `FileManagerTUI-5yf`
forbade or a larger envelope allowance. The code doc, the Task5 report and the
brittle `drive_content_to_completion` assertion were corrected to the documented
slack (coordinator-applied, reviewer-recommended remedy; suite 1207 green,
`src/search.rs` `4bf8649a…`). If this ruling is wrong, the cost is a traversal
bound adjustment, not lost results or a hang.

## Ruling 23: Session records clamp viewport state and cap documents on load

The Phase 7 Task 1 review proved that a syntactically valid session record with an
extreme `scroll_offset` reached `EditorState::ensure_cursor_visible`
(`src/editor.rs:364`) and overflowed on the first render, violating AC-3's
predictable-state guarantee for corrupt records. `restore_cursor` now clamps
`scroll_offset`/`horizontal_offset` and calls `clamp_viewport()`, so a parsed
record cannot reach the render path with an overflowing value. The review also
showed the document cap was enforced only on capture; load now refuses counts over
`MAX_SESSION_DOCUMENTS` with a typed error and the apply path truncates with
explicit skipped accounting (at most 256 opens). Captured sessions never include
transient previews, and a missing private state directory is a visible non-fatal
notice rather than a silent disable. If this ruling is wrong, the cost is a
restore-path clamp adjustment, not a crash or a lost session.

## Ruling 24: Recovery retention age is ceiling-bounded and staged modules may carry one dead_code allow

The Phase 7 Task 2 review and confirmation proved that `recovery_max_age_secs` had
only a lower clamp, so a configured value of 2^63 or more wrapped negative in
`as_secs() as i64`, placed the age cutoff in the future and deleted every owned
snapshot on the next save or load (reviewer repro: a fresh record returned 0 on
`load_all`). The age is now clamped to `1 hour..=100 years` in the config accessor
and re-clamped in `RecoveryPolicy`, and the confirmation verified across
`{i64::MAX-1, i64::MAX, 2^63, u64::MAX/2, u64::MAX}` that no cutoff can land in
the future. The confirmation also accepted staging `mod recovery;` behind a single
`#[allow(dead_code)]`, matching the existing `mod background;` precedent, because
Task 2 delivers and reviews a tested store while Task 3 owns the wiring; the
attribute must be removed in Task 3. If this ruling is wrong, the cost is a
retention-window adjustment or an earlier wiring step, not lost snapshots.

## Ruling 25: Live surfaces share one entry point and prompts count what they act on

The Phase 7 Task 3 review proved two honesty defects that share a cause. Settings
-> Recovery wrote `app.config` without updating the live `context.policy.enabled`,
so disabling persistence in Settings left snapshots running until the next launch;
applied settings now route through the same `set_recovery_enabled` entry point as
the commands, so config, live policy, retained records and the command surface
cannot diverge. Second, the recovery prompt counted retained records while the
offer pass advanced per document, so one document with several revision snapshots
overstated the remaining offers; the prompt now carries the specific document,
counts distinct unhandled `document_path`s, offers each in turn bounded by that
set, and its wording says "documents" rather than "records". If this ruling is
wrong, the cost is a revert to restart-scoped settings and a record-count
disclosure, which the review already flagged as misleading.

## Ruling 26: Idle snapshots are timer-scheduled, and acceptance fixtures need inert sentinels

The Phase 7 Task 4 checkpoint proved a genuine capture gap: the throttled snapshot
pass ran only on input-driven loop iterations, so a user who edited once and then
left the editor idle was never captured. The main loop now folds a bounded,
strictly-positive wait (`SnapshotThrottle::remaining` plus
`App::recovery_snapshot_wait`) into its existing timer, so a pending dirty
document is captured within `min_interval_ms`; the confirmation review measured
about 1% idle CPU with a dirty document, proved no wake exists when persistence is
disabled or nothing is dirty, and confirmed the pass stays outside render. The
fix-round correction that moves the enabled/dirty admission checks ahead of
`throttle.due` belongs to the same guarantee, so an idle event cannot consume the
write window. The capture-loss window is therefore bounded by the configured
interval. The checkpoint's menu-fixture failure was root-caused to a name
collision rather than a product defect: the fixture's sentinel query
`recovery.clear` became a real enabled command once a record exists, so the
fixture now uses `zzz.no-such-command`, and acceptance fixtures must not use
plausible future command ids as inert sentinels. Finally, within the private
owner-only records directory the exact `snap-<16hex>-<16hex>-<16hex>.json` name
shape is the ownership claim for retention and clear; payload validation there is
optional hardening, not a requirement. If this ruling is wrong, the cost is a
throttle-interval adjustment, a fixture-sentinel change, or added
defense-in-depth validation.
