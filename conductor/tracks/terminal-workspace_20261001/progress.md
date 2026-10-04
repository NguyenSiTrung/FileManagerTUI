
Task3 (incremental filename + content search) accepted and closed. Scoped review
25249aac: PASS/APPROVE, no P0/P1, four P2s; fix round 1 addressed all four
(CRLF `SearchHit.column` in editor-buffer coordinates, content queue bounded
jointly with pending directories plus `(incomplete)` on every non-complete path,
NUL scan over the whole bounded read, genuine FIFO) and the confirmation review
reproduced each defeat red and hash-verified restores: PASS/APPROVE. Final
evidence: 1171 tests, production Clippy/fmt/diff clean, eight original PTYs,
independent coverage 1060/1121 = 94.56% (coordinator and reviewer identical).
Pre-existing follow-up filed as `FileManagerTUI-5yf` (wide-directory listing
stall) and deferred by Ruling20 to the Task 5 sweep. Next: Task4 path selections
and document-aware watcher polling.

Task4 (path selections + document-aware watcher polling) accepted and closed.
Scoped review PASS/APPROVE with a P1 (self-write marker could swallow a genuine
external write), a documentation/ semantics item, and a P2; fix round 1 made
consumption evidence-based and aligned every user-visible surface, and the
confirmation revealed the content identity was inert, so fix round 2 replaced it
with an FNV-1a digest of the first 64 KiB of the published bytes computed from
memory. Confirmation round 2 reproduced both defeat reds (length+mtime-only fails
two tests; the round-1 `utimensat` probe now reports the change) and proved
`serialized_content()` byte-identical to what is written: PASS/APPROVE. Final:
9 src files, 1195 tests, production gates clean, eight PTYs, merged coverage
407/439 = 92.71%. Ruling21 records the accepted toggle semantics.

Task5 (automated checkpoint for responsiveness and search) accepted and closed,
and with it `FileManagerTUI-5yf`. The first review rejected the slice for a P0
livelock: the resumable listing paused by requeueing itself while the bound
check refused whenever pending+queue >= max_pending, so a full pending set of
non-empty directories with an empty file queue cycled forever (worker path burned
the job timeout and reported an incomplete search with zero results; the
deadline-free fallback hung). Fix round 1 made the popped listing consume its
next entry unconditionally before re-checking the bound; the confirmation review
re-derived the stall red, verified termination on the reviewer's exact
4096-non-empty-subdirectory shape with strict per-continuation advance, probed
all-excluded/symlink/unreadable/deep/no-deadline shapes with no stall and no lost
hits, and confirmed the direct stale-generation test fails without the guard.
Its one P2 (a false hard-bound claim) was corrected by the coordinator per the
reviewer's recommendation (Ruling22). Final: 4 files (app.rs/main.rs test-only),
1207 tests, production gates clean, eight PTYs, independent coverage 65/67 =
97.01% / reviewer 76/80 = 95.00%. The closure audit is in the report with
explicit gaps.

Task6 (.6.6 legacy save bounds) accepted and closed, completing the Phase 6 task
list. Two files (src/fs/save.rs, src/editor.rs; app.rs needed no change): document
loads and overwrite revision captures now all take an explicit byte budget
(default derived from DEFAULT_MAX_EDITOR_BYTES), growth maps to Conflict with no
clipping or partial publication, and refusal preserves original bytes, dirty
buffer, permissions, atomicity and symlink identity. Scoped review PASS/APPROVE
with two independently re-derived compiled defeats and a caller inventory verified
complete; 1216 tests, gates, eight PTYs, coverage independently recomputed
38/38 = 100%. The review's scope correction exposed two pre-existing unbounded
whole-file preview reads (notebook loader and a TOCTOU-window read), filed as
their own bead under the bounded-I/O mandate and recorded in the report and plan.

FileManagerTUI-8sy (all-targets Clippy test lint debt) accepted and closed: 60
warnings to zero across 10 files, all changes inside test code (0
production-changed lines), no #[allow] added, assertion polarity preserved
(assert_eq!(x, true/false) -> assert!(x)/assert!(!x), 54 converted assertions),
1216 tests, production gate and all-targets gate clean, fmt/diff clean, eight
PTYs pass. Phase 6 is complete: all six tasks plus 5yf closed; 8u3 (unbounded
preview reads) and the two manual-verification beads remain open by design.

Phase 7 Task 1 (.7.1, versioned workspace session serialization) accepted and
closed. Deliverable: new src/session.rs plus config.rs, main.rs,
workspace/documents.rs, workspace/layout.rs. Scoped review REQUESTCHANGES for a
render-path overflow (extreme scroll offset from a parsed record) and a missing
load-time document cap; fix round 1 clamped viewport state through
clamp_viewport, made the cap a typed load refusal plus explicit apply truncation,
replaced the substring negative assertion with a key whitelist, stopped capturing
transient previews, and made a missing state directory a visible notice. The
confirmation review re-derived the panic red (with and without each clamp),
verified the cap boundaries deterministically, and confirmed the whitelist fails
on an unexpected field: PASS/APPROVE. Final: 1238 tests, both Clippy gates clean,
fmt/diff clean, eight PTYs, independent coverage 261/281 = 92.88% (exact match;
main() wiring the documented remainder).

Phase 7 Task 2 (.7.2, bounded private recovery snapshots) accepted and closed.
Deliverable: new src/recovery.rs plus config.rs, main.rs. Document/revision-keyed
snapshot records (canonical path plus exact size and ns-mtime identity), atomic
owner-only writes under <state>/recovery/, retention bounded by per-workspace
count (default 32, clamp 1..=1024) and age (default 7 days, 1 hour..=100 years),
typed refusals for corrupt/oversized/unknown/future/unsafe/disabled/disk-changed/
document-not-open/unavailable, restore into a dirty document that never writes the
original and requires the ordinary conflict-safe save, explicit discard/clear and
a SnapshotThrottle. The scoped review PASS/APPROVE defeat-checked every safety
property (revision refusal, retention, symlink/permission rejection, owner-only
modes, restore-never-writes); fix round 1 fixed a destructive missing upper clamp
on recovery_max_age_secs (u64::MAX wrapped negative and deleted every snapshot),
added the missing atomic-write temp-cleanup test, tightened clear() to the strict
record-name check, and made the mtime-failure revision identity an explicit
sentinel. Confirmation PASS/APPROVE, no new findings. Final: 1258 tests, both
Clippy gates clean, fmt/diff clean, eight PTYs, independent coverage 472/505 =
93.47% (recovery.rs 92.86%).

Phase 7 Task 3 (.7.3, startup restoration and recovery commands) accepted and
closed after two fix rounds. Deliverable: startup wiring in main.rs, a recovery
prompt carrying the specific document plus a remaining count with bounded
re-offer, restore/discard/clear commands, an enable/disable surface, live
settings sync through set_recovery_enabled, throttled snapshots outside render,
a bounded shutdown flush with visible failures, and visible notices for missing
roots. Scoped review PASS/APPROVE (11 independent defeats load-bearing, coverage
method verified honest); fix round 1 fixed the settings live desync, the
"Discard all" label and the newest-only offer; fix round 2 made the "further
snapshots" disclosure count distinct documents. Both confirmation rounds
re-derived R1-R4 defeats and reproduced coverage exactly. Final: 1272 tests, both
Clippy gates clean, fmt/diff clean, eight PTYs, independent coverage 295/336 =
87.80% (exact-line matching; the whitespace-insensitive matcher over-counts on
repeated-line-heavy files and must not be used).

Phase 7 Task 4 (.7.4, automated checkpoint for session recovery) accepted and
closed; Phase 7 is complete. The checkpoint added two fixtures
(phase7-restart-pty, phase7-recovery-pty) proving save/reload, kill -9 recovery
with the original byte-identical, refusal inert and re-offered, disabled
persistence writing nothing, missing/foreign roots, and corrupt records
non-fatal; it also proved structurally that copied text never reaches
/tmp/.fm_clipboard and that session/recovery records carry no process, trust or
secret fields. It found and fixed a genuine capture gap: idle dirty edits were
never snapshotted because the throttled pass ran only on input-driven loop
iterations; the main loop now folds a bounded strictly-positive wait into its
timer (measured ~1% idle CPU with a dirty document, no wake when disabled or
clean, pass outside render), and a fix-round correction moved the enabled/dirty
admission checks ahead of the throttle window so idle events cannot consume it.
Final: 1275 tests, both Clippy gates, fmt/diff, all ten PTYs. A fixture failure
during the fix round was root-caused by coordinator A/B and a screen dump to a
name collision, not a product defect: the fixture's sentinel query
`recovery.clear` became a real enabled command once records exist, so the
fixture now uses `zzz.no-such-command`; the report's earlier "pre-existing"
claim was withdrawn and corrected, and the real UX finding (clear runs without
confirmation) is filed as FileManagerTUI-9kk (P3). FileManagerTUI-8u3 (preview
read bounds) is scheduled next; the review-only helper
phase3-task3-review-pty.py fails pre-existing by its own helper design and is
not one of the ten acceptance fixtures.

FileManagerTUI-8u3 (bound the unbounded preview reads) delivered and closed. Both
whole-file preview paths now open once and read at most budget+1 bytes from the
same descriptor (the src/fs/save.rs pattern), the notebook path takes an explicit
5 MiB budget, auxiliary head/tail/shebang reads are bounded, and max_editor_bytes
is clamped to 256 MiB with a lossless bounded usize accessor and a compile-time
fit assert. Scoped review PASS/APPROVE with both bounds defeat-checked (removing
detection gave four failures; removing take showed 131072 consumed vs 16385
allowed; removing the clamp reproduced u64::MAX) and the bound-table audit
agreeing with the report. Final: 1282 tests, both Clippy gates, fmt/diff, ten
PTYs, coverage 64/64 = 100%. Two informational P2s filed as P3 cleanup beads
(FileManagerTUI-e0y, FileManagerTUI-nux); the report's line spans were corrected.

Phase 8 Task 1 (.8.1, bounded machine-readable Git backend) accepted and closed
after two fix rounds. Deliverable: new src/git.rs plus main.rs/event.rs wiring.
Strictly read-only: a single whitelisted status query, argument-vector execution
with --no-optional-locks and GIT_OPTIONAL_LOCKS=0/LC_ALL=C, a 4 MiB output cap
enforced while reading, a 3 s timeout, AtomicBool cancellation with process-group
SIGKILL and wait (no zombies), typed executable-missing/not-a-repository refusals,
exact NUL-delimited porcelain v2 parsing including renames, conflicts and paths
with spaces/newlines/Unicode, and a generation contract that refuses stale and
duplicate results and clears retained state on non-snapshots. Review evidence
included a real-run fake-git argv/environment trace. Fix round 1 made the
read-only guarantee release-enforced (production argv resolves through the
whitelist at runtime, refusing with a typed error; confirmed by a release-mode
no-spawn probe) and tightened a missing branch.oid to a typed error; fix round 2
pinned the stderr bound with an owning test and corrected a doc overclaim. The
confirmation withdrew its own stderr-deadlock hypothesis after OS-level probes
showed an exited flooder reports its real status. Final: 1310 tests, both Clippy
gates, release build, fmt/diff, ten PTYs, coverage 423/454 = 93.17% (git.rs
407/438). Fixture flakiness in phase4-help-pty (1/40, load-sensitive) filed as a
P3 bead.

Phase 8 Task 2 (.8.2, branch/file/directory Git indicators) accepted and closed.
Deliverable over nine files: color-independent ASCII markers (U/M/S/?) tinted
only by theme, directory aggregation from the same in-memory snapshot, a status
branch with a reserved budget (`branch`/`(unborn)`/`HEAD (detached)`) that no
longer vanishes and truncates safely, refresh only at startup and on FsChange
through the background jobs with stale generations refused and prior-workspace
results re-validated against the current work-tree root, S3/virtual-root
exclusion, and a `[git] enabled` setting with merge/settings/`--no-git` and live
disable clearing decorations. A new additive fixture (phase8-git-pty) covers a
real repository with branch, M/S/? entries, a directory aggregate and a
watcher-driven refresh. Scoped review PASS/APPROVE: eight neuter-to-red defeats
all restored hash-verified, color independence proven by a Color::Reset identity
probe, render confirmed to spawn nothing, and the two "strengthened" tests judged
stronger than the plan's substring assertions with negative controls. Final:
1325 tests, both Clippy gates, fmt/diff, eleven PTYs, coverage 218/226 = 96.46%
(worker) and 229/237 = 96.62% (coordinator extraction). Two informational P3s
were folded into the Task 3 checkpoint brief as binding rather than filed:
clearing the retained snapshot when the Git state's root changes (latent,
unreachable today) and caching the per-frame .git ancestor stat walk.

Phase 8 Task 3 (.8.3, automated checkpoint for read-only Git integration)
accepted and closed. The checkpoint fixed its two folded items (clearing the
retained snapshot when the Git state's root changes; a memoized work-tree
resolution) and found the phase's most consequential defect by running the real
binary: `git status` opens working-tree directories, notify maps those opens to
a generic change, and each refresh therefore fed the next one, a self-sustaining
loop of roughly twenty-five invocations per second. The fix is a 750 ms
coalescing floor, which the runtime fixture now discriminates both ways
(un-coalesced release: 171 > 32 FAIL; coalesced: 13 PASS), plus a bounded
deferred wake so a change dropped inside the window converges with no further
filesystem event. The checkpoint's own review caught a test-fidelity gap - the
loop-fold test recomposed the wait locally instead of driving the production
expression, so deleting the fold left it green; the fix extracted
`loop_wait`/`wait_for_loop` and added a structural guard that fails when the
loop bypasses the fold, with the reviewer's original defeat now red. Final:
1333 tests, both Clippy gates, release build, fmt/diff, eleven canonical PTYs,
invocation fixture modes no-watcher/normal/injected all passing, changed-
production coverage 78/82 = 95.12% on the strict literal basis (54/54 = 100% on
the new-semantics basis; the difference is four pre-existing statements
reindented by the loop rewrite), repository Git state byte-identical around the
fixture runs. Phase 8 epic (FileManagerTUI-cen.8) closed; recorded gaps: AC-12
cross-platform checks not re-run on Linux, stale-generation refusal unit-proven
rather than runtime-proven, and the deferred-refresh deferral discriminated by
unit test because the watcher's own events mask it in the PTY.

Phase 12 (automated terminal/browser acceptance and handoff) — all four tasks
closed, completing the track (12/12 phases, 52/52 tasks):

- .12.1 built the isolated PTY + browser-terminal fixtures
  (scripts/test-terminal-workspace.py stdlib runner; tools/terminal-tests
  node-pty → ws → vendored xterm.js server driven by Playwright).
- .12.2 automated the full AC-1..AC-11 matrix — 13 PTY scenarios +
  7 browser tests. The governing discovery: ratatui diff-renders only
  changed cells and emits styled spans separately, so screen assertions
  must match ANSI-stripped text and use only freshly-emitting content
  (candidate `▸` rows, `Fuzzy Finder` title, `! [pin]` markers), never
  already-rendered chrome. Also shipped a product fix found red-first:
  CR-only line endings in bracketed paste now normalize to LF.
- .12.3 documented the shipped surface (README Git/LSP/recovery sections
  + config blocks + harness commands; PLAN.md §11.3/11.4; conductor
  product/tech-stack/guidelines), added the `release-builds` CI matrix
  (linux-musl/macos/windows --release + size report, existing
  tag/dispatch triggers only), and shipped
  scripts/check-terminal-workspace.sh — the single quality runner with
  nonzero exit on mandatory FAIL/BLOCK. Local musl toolchain installed
  (rustup + musl-tools): 4,158,072-byte statically-linked binary vs the
  4,034,400-byte gnu build, both under the 10 MiB NFR.
- .12.4 produced acceptance-evidence.md (gate table + FR-1..FR-11 +
  AC-1..AC-12 → test map + recorded boundaries), closed cen.12.4/cen.12/
  cen, and flipped the track to completed. Unrelated earlier-phase
  follow-up beads (9kk, e0y, nux, bte, 652.*) left open untouched.

Final mandatory gates: 1524/1524 cargo tests, clippy (lib +
all-targets) clean, fmt clean, PTY 13/13, browser 7/7, llvm-cov 94.6%
lines / 94.8% functions, static musl build green — quality runner exit 0.
No manual verification was used anywhere in the track.
