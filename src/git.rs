//! Bounded, strictly read-only Git status backend.
//!
//! The only Git process this application may ever start is assembled from the
//! [`READ_ONLY_ARGUMENTS`] whitelist and executed with an argument vector, never
//! a shell string. Optional locks are disabled (`--no-optional-locks` plus
//! `GIT_OPTIONAL_LOCKS=0`) so a status refresh can never write to the user's
//! repository, and no Git write verb appears in application code.
//!
//! The backend is bounded on three axes: captured output, wall-clock time, and
//! cooperative cancellation. Missing executables and non-repository
//! directories are typed refusals rather than panics. Parsing of
//! `git status --porcelain=v2 -z --branch` is NUL-delimited and exact, so
//! whitespace, newlines and Unicode in paths survive verbatim.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The Git executable name resolved through the process `PATH`.
pub const GIT_PROGRAM: &str = "git";

/// Maximum bytes of git's stderr the backend drains. stderr exists only to keep
/// the child's pipe from blocking and to carry a short failure diagnostic; it is
/// bounded here and is never a kill trigger (the wall-clock timeout is). The
/// owning test `stderr_pump_is_bounded_at_eight_kibibytes` pins this value; a
/// call-site mis-wire is caught by review and the Clippy-deny debug profiles.
const STDERR_BOUND_BYTES: usize = 8192;

/// The single source of truth for invocations the backend is allowed to
/// execute. Every vector here is a read-only query; no stage/commit/push/
/// checkout/reset/stash/clean verb may ever be added.
pub const READ_ONLY_ARGUMENTS: &[&[&str]] = &[&["status", "--porcelain=v2", "-z", "--branch"]];

/// Array form of the whitelist entry the production refresh uses. Kept beside
/// [`READ_ONLY_ARGUMENTS`] and pinned to it by
/// [`whitelist_is_the_single_source_of_truth`](tests::whitelist_is_the_single_source_of_truth),
/// so the constant-evaluated [`STATUS_ARGV`] is provably built from the
/// whitelisted query in release builds as well as test builds.
const STATUS_QUERY: [&str; 4] = ["status", "--porcelain=v2", "-z", "--branch"];

/// Global flags prepended to every query. `--no-optional-locks` stops status
/// from taking `index.lock`, so a refresh can never write to the repository.
const GLOBAL_ARGUMENTS: [&str; 1] = ["--no-optional-locks"];

/// Exact read-only argument vector (program name excluded) used by
/// [`status_bounded`]. The leading global flag plus the whitelisted status
/// query are expanded element-by-element, so no literal in the production
/// vector can drift from [`STATUS_QUERY`]; the global `--no-optional-locks`
/// flag precedes the subcommand so status cannot take `index.lock`.
pub const STATUS_ARGV: &[&str] = &[
    GLOBAL_ARGUMENTS[0],
    STATUS_QUERY[0],
    STATUS_QUERY[1],
    STATUS_QUERY[2],
    STATUS_QUERY[3],
];

/// Whether a subcommand argument vector (without the program name and without
/// global flags) is one of the permitted read-only queries.
pub fn is_read_only_invocation(args: &[&str]) -> bool {
    READ_ONLY_ARGUMENTS.contains(&args)
}

/// Output and time budget for one Git query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitLimits {
    /// Maximum bytes of `stdout` captured before the child is killed and
    /// [`GitResult::OutputTooLarge`] is reported.
    pub output_bytes: usize,
    /// Maximum wall-clock time before the child is killed and
    /// [`GitResult::TimedOut`] is reported.
    pub timeout: Duration,
}

impl Default for GitLimits {
    fn default() -> Self {
        Self {
            output_bytes: 4 * 1024 * 1024,
            timeout: Duration::from_secs(3),
        }
    }
}

/// Typed refusal to produce a snapshot. These are honest, non-panicking
/// answers for the environment being unable to satisfy the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitRefusal {
    /// The Git executable was not found on `PATH`.
    ExecutableMissing,
    /// The working directory is not inside a Git repository.
    NotARepository,
    /// Git ran but failed for another reason. `detail` is bounded stderr.
    Failed { code: i32, detail: String },
}

/// The result of one bounded read-only Git query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitResult {
    /// A successfully parsed status snapshot.
    Snapshot(GitSnapshot),
    /// A typed environmental refusal.
    Refused(GitRefusal),
    /// Git produced output that could not be parsed as porcelain v2.
    Malformed(GitParseError),
    /// The output budget was exceeded.
    OutputTooLarge { limit: usize },
    /// The wall-clock timeout elapsed.
    TimedOut,
    /// Cancellation was requested before completion.
    Cancelled,
}

impl GitResult {
    /// Conservative retained-byte accounting for the event transport.
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>().saturating_add(match self {
            Self::Snapshot(snapshot) => snapshot.retained_bytes(),
            Self::Malformed(error) => error.retained_bytes(),
            Self::Refused(GitRefusal::Failed { detail, .. }) => detail.capacity(),
            Self::Refused(_) | Self::OutputTooLarge { .. } | Self::TimedOut | Self::Cancelled => 0,
        })
    }
}

/// Branch classification derived from the `# branch.*` headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchState {
    /// An ordinary branch with a name and commit object id.
    Symbolic { name: String, oid: String },
    /// A detached HEAD; there is no branch name.
    Detached { oid: String },
    /// A repository with no commits yet; the branch name exists but the oid is
    /// `(initial)`.
    Unborn { name: String },
}

impl BranchState {
    /// A human-readable short label for status surfaces.
    pub fn label(&self) -> &str {
        match self {
            Self::Symbolic { name, .. } | Self::Unborn { name } => name,
            Self::Detached { .. } => "HEAD (detached)",
        }
    }

    /// Whether HEAD is detached.
    pub fn is_detached(&self) -> bool {
        matches!(self, Self::Detached { .. })
    }

    /// Whether the repository has no commits yet.
    pub fn is_unborn(&self) -> bool {
        matches!(self, Self::Unborn { .. })
    }

    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>().saturating_add(match self {
            Self::Symbolic { name, oid } => name.capacity().saturating_add(oid.capacity()),
            Self::Detached { oid } => oid.capacity(),
            Self::Unborn { name } => name.capacity(),
        })
    }
}

/// The porcelain v2 record family for an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitEntryKind {
    /// `1` — ordinary changed entry.
    Ordinary,
    /// `2` — renamed or copied entry carrying an original path.
    Renamed,
    /// `u` — unmerged/conflicted entry.
    Unmerged,
    /// `?` — untracked entry.
    Untracked,
    /// `!` — ignored entry.
    Ignored,
}

/// One parsed status entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitEntry {
    /// Current path, exactly as Git emitted it (no trimming or quoting).
    pub path: String,
    /// Original path for renamed/copied entries (`2` records).
    pub original_path: Option<String>,
    /// Record family.
    pub kind: GitEntryKind,
    /// Two-character status code: `[index, worktree]`. Untracked entries use
    /// `??` and ignored entries use `!!`.
    pub status: [u8; 2],
}

impl GitEntry {
    /// Whether this entry is unmerged (has conflicts).
    pub fn is_conflicted(&self) -> bool {
        self.kind == GitEntryKind::Unmerged
    }

    /// Whether this entry is untracked.
    pub fn is_untracked(&self) -> bool {
        self.kind == GitEntryKind::Untracked
    }

    /// Whether this entry is ignored.
    pub fn is_ignored(&self) -> bool {
        self.kind == GitEntryKind::Ignored
    }

    /// Whether the index (staged) side carries a change.
    pub fn is_staged(&self) -> bool {
        match self.kind {
            GitEntryKind::Ordinary | GitEntryKind::Renamed => self.status[0] != b'.',
            GitEntryKind::Unmerged => true,
            GitEntryKind::Untracked | GitEntryKind::Ignored => false,
        }
    }

    /// Whether the worktree (unstaged) side carries a change.
    pub fn is_unstaged(&self) -> bool {
        match self.kind {
            GitEntryKind::Ordinary | GitEntryKind::Renamed => self.status[1] != b'.',
            GitEntryKind::Unmerged => true,
            GitEntryKind::Untracked | GitEntryKind::Ignored => false,
        }
    }
}

/// A parsed `--porcelain=v2 --branch` snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSnapshot {
    /// Branch/detached/unborn classification.
    pub branch: BranchState,
    /// Changed entries in Git's output order.
    pub entries: Vec<GitEntry>,
}

impl GitSnapshot {
    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.branch.retained_bytes())
            .saturating_add(
                self.entries
                    .capacity()
                    .saturating_mul(std::mem::size_of::<GitEntry>()),
            )
            .saturating_add(self.entries.iter().fold(0usize, |bytes, entry| {
                bytes.saturating_add(entry.path.capacity()).saturating_add(
                    entry
                        .original_path
                        .as_ref()
                        .map_or(0, |path| path.capacity()),
                )
            }))
    }
}

/// Structural parse failure; never a panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitParseErrorKind {
    /// The final record was not NUL-terminated.
    Unterminated,
    /// No `# branch.head` header was present.
    MissingHeader,
    /// A record began with an unrecognised type byte.
    UnknownRecord,
    /// A header record was malformed.
    MalformedHeader,
    /// An entry record had too few fields or an invalid status code.
    MalformedEntry,
}

/// A typed porcelain parse error with a bounded human-readable detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitParseError {
    /// Failure family.
    pub kind: GitParseErrorKind,
    /// Bounded explanation.
    pub detail: String,
}

impl GitParseError {
    fn new(kind: GitParseErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>().saturating_add(self.detail.capacity())
    }
}

impl std::fmt::Display for GitParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "git porcelain v2 parse error ({:?}): {}",
            self.kind, self.detail
        )
    }
}

impl std::error::Error for GitParseError {}

/// One generation-tagged refresh result together with the workspace identity
/// that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRefresh {
    /// Monotonic generation issued by [`GitState::begin`].
    pub generation: u64,
    /// Workspace root the query ran against.
    pub root: PathBuf,
    /// The bounded result.
    pub result: GitResult,
}

impl GitRefresh {
    /// Conservative retained-byte accounting for the event transport.
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.root.capacity())
            .saturating_add(self.result.retained_bytes())
    }
}

/// Generation-tagged refresh coalescer.
///
/// [`begin`](Self::begin) issues a monotonically increasing generation and
/// records the workspace root. [`accept`](Self::accept) refuses any refresh
/// whose generation is not the most recently issued one, so a late result from
/// an earlier workspace or an earlier request can never overwrite newer state.
#[derive(Debug, Default)]
pub struct GitState {
    issued: u64,
    accepted: Option<u64>,
    root: Option<PathBuf>,
    snapshot: Option<GitSnapshot>,
}

#[allow(dead_code)] // Accessors are consumed by rendering (Phase 8 Task 2).
impl GitState {
    /// Create an empty state with no issued requests.
    pub fn new() -> Self {
        Self::default()
    }

    /// Issue the next refresh generation for `root`.
    ///
    /// When `root` differs from the previously recorded root, the retained
    /// snapshot is dropped immediately. A render that races the in-flight
    /// result would otherwise pass the caller's work-tree guard (the requested
    /// and current root are both the new root) and recolor the tree with the
    /// previous repository's snapshot. A same-root refresh keeps the snapshot
    /// so ordinary re-requests do not flicker.
    pub fn begin(&mut self, root: PathBuf) -> u64 {
        self.issued = self.issued.saturating_add(1);
        if self.root.as_deref() != Some(root.as_path()) {
            self.snapshot = None;
        }
        self.root = Some(root);
        self.issued
    }

    /// The most recently issued generation.
    pub fn issued(&self) -> u64 {
        self.issued
    }

    /// The workspace root of the most recent request or accepted refresh.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// The most recently accepted snapshot, if any.
    pub fn snapshot(&self) -> Option<&GitSnapshot> {
        self.snapshot.as_ref()
    }

    /// Attempt to install a refresh. Returns `false` and leaves state untouched
    /// for a stale generation (an earlier request or workspace) or for a
    /// duplicate delivery of the current generation. Any non-snapshot result
    /// clears the retained snapshot so decorations cannot linger after the
    /// repository disappears.
    pub fn accept(&mut self, refresh: GitRefresh) -> bool {
        if refresh.generation != self.issued || self.accepted == Some(refresh.generation) {
            return false;
        }
        self.accepted = Some(refresh.generation);
        self.root = Some(refresh.root);
        self.snapshot = match refresh.result {
            GitResult::Snapshot(snapshot) => Some(snapshot),
            _ => None,
        };
        true
    }
}

/// Parse NUL-delimited `git status --porcelain=v2 -z --branch` bytes.
///
/// The grammar implemented here is:
///
/// ```text
/// stream   := record* NUL          (every record is NUL-terminated)
/// record   := header | entry | renamed-original
/// header   := "# " "branch.oid " value
///           | "# " "branch.head " value
///           | "# " other-value     (upstream/ab/future headers, ignored)
/// entry    := "1 " XY " " sub " " mH " " mI " " mW " " hH " " hI " " path
///           | "2 " XY " " sub " " mH " " mI " " mW " " hH " " hI " " xscore " " path
///           | "u " XY " " sub " " m1 " " m2 " " m3 " " mW " " h1 " " h2 " " h3 " " path
///           | "? " path
///           | "! " path
/// ```
///
/// A `2` record's path is followed by a second NUL-terminated record holding
/// the original path. Fields are space-separated; the path is the remainder of
/// the record and is therefore preserved verbatim, including leading/trailing
/// whitespace, newlines and Unicode.
pub fn parse_porcelain_v2(bytes: &[u8]) -> Result<GitSnapshot, GitParseError> {
    let records = split_records(bytes)?;
    let mut oid: Option<String> = None;
    let mut head: Option<String> = None;
    let mut entries: Vec<GitEntry> = Vec::new();

    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        let Some(&tag) = record.first() else {
            return Err(GitParseError::new(
                GitParseErrorKind::MalformedEntry,
                "empty record",
            ));
        };
        match tag {
            b'#' => {
                parse_header(record, &mut oid, &mut head)?;
                index += 1;
            }
            b'1' => {
                entries.push(parse_ordinary(record)?);
                index += 1;
            }
            b'2' => {
                let mut entry = parse_renamed(record)?;
                index += 1;
                let Some(original) = records.get(index) else {
                    return Err(GitParseError::new(
                        GitParseErrorKind::MalformedEntry,
                        "renamed entry missing original path record",
                    ));
                };
                entry.original_path = Some(lossy(original));
                entries.push(entry);
                index += 1;
            }
            b'u' => {
                entries.push(parse_unmerged(record)?);
                index += 1;
            }
            b'?' => {
                entries.push(parse_simple(record, GitEntryKind::Untracked)?);
                index += 1;
            }
            b'!' => {
                entries.push(parse_simple(record, GitEntryKind::Ignored)?);
                index += 1;
            }
            other => {
                return Err(GitParseError::new(
                    GitParseErrorKind::UnknownRecord,
                    format!("unrecognised record type byte {:?}", other as char),
                ));
            }
        }
    }

    let branch = resolve_branch(oid, head)?;
    Ok(GitSnapshot { branch, entries })
}

/// Split a byte stream into NUL-terminated record bodies. The final record
/// must be terminated.
fn split_records(bytes: &[u8]) -> Result<Vec<&[u8]>, GitParseError> {
    let mut records = Vec::new();
    let mut start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == 0 {
            records.push(&bytes[start..index]);
            start = index + 1;
        }
    }
    if start != bytes.len() {
        return Err(GitParseError::new(
            GitParseErrorKind::Unterminated,
            "stream did not end with NUL",
        ));
    }
    Ok(records)
}

fn parse_header(
    record: &[u8],
    oid: &mut Option<String>,
    head: &mut Option<String>,
) -> Result<(), GitParseError> {
    let Some(rest) = record.strip_prefix(b"# ") else {
        return Err(GitParseError::new(
            GitParseErrorKind::MalformedHeader,
            "header record missing '# ' prefix",
        ));
    };
    let rest = lossy(rest);
    if let Some(value) = rest.strip_prefix("branch.oid ") {
        *oid = Some(value.to_string());
    } else if let Some(value) = rest.strip_prefix("branch.head ") {
        *head = Some(value.to_string());
    }
    // branch.upstream, branch.ab and any future header are informational.
    Ok(())
}

fn resolve_branch(oid: Option<String>, head: Option<String>) -> Result<BranchState, GitParseError> {
    let Some(head) = head else {
        return Err(GitParseError::new(
            GitParseErrorKind::MissingHeader,
            "missing # branch.head",
        ));
    };
    // A `--branch` stream always emits branch.oid; without it the stream is
    // truncated/malformed, and the unborn state must be inferred only from a
    // real `# branch.oid (initial)` value.
    let Some(oid) = oid else {
        return Err(GitParseError::new(
            GitParseErrorKind::MissingHeader,
            "missing # branch.oid",
        ));
    };
    if head == "(detached)" {
        return Ok(BranchState::Detached { oid });
    }
    if oid == "(initial)" {
        Ok(BranchState::Unborn { name: head })
    } else {
        Ok(BranchState::Symbolic { name: head, oid })
    }
}

fn parse_ordinary(record: &[u8]) -> Result<GitEntry, GitParseError> {
    let fields = split_fields(record, 9, "ordinary entry")?;
    Ok(GitEntry {
        path: require_path(fields[8])?,
        original_path: None,
        kind: GitEntryKind::Ordinary,
        status: validate_status(fields[1])?,
    })
}

fn parse_renamed(record: &[u8]) -> Result<GitEntry, GitParseError> {
    let fields = split_fields(record, 10, "renamed entry")?;
    Ok(GitEntry {
        path: require_path(fields[9])?,
        original_path: None,
        kind: GitEntryKind::Renamed,
        status: validate_status(fields[1])?,
    })
}

fn parse_unmerged(record: &[u8]) -> Result<GitEntry, GitParseError> {
    let fields = split_fields(record, 11, "unmerged entry")?;
    Ok(GitEntry {
        path: require_path(fields[10])?,
        original_path: None,
        kind: GitEntryKind::Unmerged,
        status: validate_status(fields[1])?,
    })
}

fn parse_simple(record: &[u8], kind: GitEntryKind) -> Result<GitEntry, GitParseError> {
    let Some(rest) = record.strip_prefix(&[record[0], b' ']) else {
        return Err(GitParseError::new(
            GitParseErrorKind::MalformedEntry,
            "simple entry missing space after type byte",
        ));
    };
    let status = match kind {
        GitEntryKind::Untracked => *b"??",
        GitEntryKind::Ignored => *b"!!",
        _ => *b"..",
    };
    Ok(GitEntry {
        path: require_path(rest)?,
        original_path: None,
        kind,
        status,
    })
}

/// Split a record into exactly `count` space-separated fields, where the last
/// field absorbs the remainder (so paths with spaces stay intact).
fn split_fields<'a>(
    record: &'a [u8],
    count: usize,
    what: &str,
) -> Result<Vec<&'a [u8]>, GitParseError> {
    let fields: Vec<&[u8]> = record.splitn(count, |byte| *byte == b' ').collect();
    if fields.len() != count {
        return Err(GitParseError::new(
            GitParseErrorKind::MalformedEntry,
            format!("{what} expected {count} fields, found {}", fields.len()),
        ));
    }
    Ok(fields)
}

fn validate_status(xy: &[u8]) -> Result<[u8; 2], GitParseError> {
    if xy.len() != 2
        || !xy
            .iter()
            .all(|byte| *byte == b'.' || byte.is_ascii_uppercase())
    {
        return Err(GitParseError::new(
            GitParseErrorKind::MalformedEntry,
            "invalid two-character status code",
        ));
    }
    Ok([xy[0], xy[1]])
}

fn require_path(path: &[u8]) -> Result<String, GitParseError> {
    if path.is_empty() {
        return Err(GitParseError::new(
            GitParseErrorKind::MalformedEntry,
            "entry path is empty",
        ));
    }
    Ok(lossy(path))
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Run the whitelisted read-only status query against `root`.
///
/// The vector is re-checked against [`READ_ONLY_ARGUMENTS`] at runtime (a real
/// check, not a `debug_assert!`) before execution, so the whitelist gates the
/// production invocation in release builds as well as test builds. A vector
/// that is not whitelisted is refused with [`GitRefusal::Failed`] and never
/// spawned.
pub fn status_bounded(root: &Path, limits: GitLimits, cancelled: &AtomicBool) -> GitResult {
    let Some(argv) = whitelisted_status_argv() else {
        return GitResult::Refused(GitRefusal::Failed {
            code: -1,
            detail: "git invocation refused: argument vector not whitelisted".to_string(),
        });
    };
    run_bounded(GIT_PROGRAM, argv, root, limits, cancelled)
}

/// Resolve the production status argument vector, re-validating it against
/// [`READ_ONLY_ARGUMENTS`] at runtime. Returns `None` (refusing to spawn) when
/// the constant vector has drifted out of the whitelist, so a future edit that
/// adds a write verb fails closed in release instead of executing.
///
/// The constant linkage (`STATUS_ARGV` expanded from `STATUS_QUERY`, and that
/// entry present in `READ_ONLY_ARGUMENTS`) is enforced by the test
/// `whitelist_is_the_single_source_of_truth_for_the_production_argv`, which is
/// compiled out of release binaries; the release guarantee therefore rests on
/// this fail-closed runtime gate, not on the test.
fn whitelisted_status_argv() -> Option<&'static [&'static str]> {
    if is_read_only_invocation(&STATUS_ARGV[1..]) {
        Some(STATUS_ARGV)
    } else {
        None
    }
}

/// Spawn a bounded read-only refresh on a blocking task and deliver the
/// generation-tagged result through the canonical event transport. The child
/// is reaped by [`status_bounded`] even when the application cancels or quits.
pub fn spawn_refresh(
    tx: crate::event::EventSender,
    root: PathBuf,
    generation: u64,
    limits: GitLimits,
    cancelled: Arc<AtomicBool>,
) {
    let task_root = root.clone();
    drop(tokio::task::spawn_blocking(move || {
        let result = status_bounded(&task_root, limits, &cancelled);
        let _ = tx.blocking_send(crate::event::Event::GitState(GitRefresh {
            generation,
            root,
            result,
        }));
    }));
}

/// Execute `program` with `args` (argument vector only, never a shell string)
/// under the output/time budget and cooperative cancellation, then reap it.
///
/// This is the only process spawn used by the Git backend. The production entry
/// point is [`status_bounded`], which pins `program`/`args` to the whitelist;
/// the generic form exists so the bound/cancel machinery has direct tests.
fn run_bounded(
    program: &str,
    args: &[&str],
    cwd: &Path,
    limits: GitLimits,
    cancelled: &AtomicBool,
) -> GitResult {
    if cancelled.load(Ordering::Acquire) {
        return GitResult::Cancelled;
    }
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && cwd.is_dir() => {
            return GitResult::Refused(GitRefusal::ExecutableMissing);
        }
        Err(error) => {
            return GitResult::Refused(GitRefusal::Failed {
                code: -1,
                detail: error.to_string(),
            });
        }
    };

    let Some(stdout) = child.stdout.take() else {
        reap(&mut child);
        return GitResult::Refused(GitRefusal::Failed {
            code: -1,
            detail: "git stdout pipe unavailable".to_string(),
        });
    };
    let Some(stderr) = child.stderr.take() else {
        reap(&mut child);
        return GitResult::Refused(GitRefusal::Failed {
            code: -1,
            detail: "git stderr pipe unavailable".to_string(),
        });
    };
    let output_limit = Arc::new(AtomicBool::new(false));
    let stdout_reader = pump(stdout, limits.output_bytes, output_limit.clone());
    // stderr is drained only to keep the child's pipe from blocking, and is
    // bounded at `STDERR_BOUND_BYTES` plus the wall-clock timeout; it is never a
    // kill trigger. When the bound is reached the reader returns and drops the
    // read end, so a child that keeps writing gets EPIPE/SIGPIPE and exits; the
    // independently polled `try_wait` below then still reports its real status.
    // A child that both ignores SIGPIPE and never exits is reported as
    // `TimedOut`, which is honest: there is no real exit status to report, and
    // the outcome remains bounded by the timeout and the kill+reap below.
    let stderr_reader = pump(stderr, STDERR_BOUND_BYTES, Arc::new(AtomicBool::new(false)));

    let deadline = Instant::now() + limits.timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if cancelled.load(Ordering::Acquire) {
            break None;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            break None;
        }
        if output_limit.load(Ordering::Acquire) {
            break None;
        }
        std::thread::sleep(Duration::from_millis(1));
    };

    reap(&mut child);
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();

    if cancelled.load(Ordering::Acquire) {
        return GitResult::Cancelled;
    }
    if timed_out {
        return GitResult::TimedOut;
    }
    if output_limit.load(Ordering::Acquire) {
        return GitResult::OutputTooLarge {
            limit: limits.output_bytes,
        };
    }
    match status {
        Some(status) if status.success() => match parse_porcelain_v2(&stdout) {
            Ok(snapshot) => GitResult::Snapshot(snapshot),
            Err(error) => GitResult::Malformed(error),
        },
        Some(status) => {
            let detail = String::from_utf8_lossy(&stderr);
            let detail = detail.trim();
            if detail.contains("not a git repository") {
                GitResult::Refused(GitRefusal::NotARepository)
            } else {
                GitResult::Refused(GitRefusal::Failed {
                    code: status.code().unwrap_or(-1),
                    detail: detail.chars().take(512).collect(),
                })
            }
        }
        None => GitResult::Refused(GitRefusal::Failed {
            code: -1,
            detail: "git did not report an exit status".to_string(),
        }),
    }
}

/// Kill (with descendants) and reap a child. Never leaves a zombie.
fn reap(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Bounded pipe reader. Sets `limit` and returns early once more than `bytes`
/// have arrived, so a runaway writer can never grow memory without bound.
fn pump(
    mut pipe: impl Read + Send + 'static,
    bytes: usize,
    limit: Arc<AtomicBool>,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut output = Vec::with_capacity(bytes.min(64 * 1024));
        let mut buffer = [0u8; 8192];
        loop {
            let read = match pipe.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(_) => {
                    limit.store(true, Ordering::Release);
                    break;
                }
            };
            let room = bytes.saturating_sub(output.len());
            if read > room {
                output.extend_from_slice(&buffer[..room]);
                limit.store(true, Ordering::Release);
                return output;
            }
            output.extend_from_slice(&buffer[..read]);
        }
        output
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn porcelain(branch: &str, body: &[&[u8]]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"# branch.oid 1111111111111111111111111111111111111111\0");
        bytes.extend_from_slice(format!("# branch.head {branch}\0").as_bytes());
        for record in body {
            bytes.extend_from_slice(record);
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn parse_porcelain_v2_handles_unusual_names_and_conflicts() {
        let bytes = porcelain(
            "main",
            &[
                b"? line\nbreak.txt",
                b"u UU N... 100644 100644 100644 100644 aaa bbb ccc conflicted.txt",
            ],
        );
        let snapshot = parse_porcelain_v2(&bytes).unwrap();
        let unusual_name = "line\nbreak.txt";
        assert!(snapshot.entries.iter().any(|e| e.path == unusual_name));
        assert!(snapshot.entries.iter().any(|e| e.is_conflicted()));
    }

    #[test]
    fn parse_porcelain_v2_classifies_staged_unstaged_and_untracked() {
        let bytes = porcelain(
            "main",
            &[
                b"? untracked.txt",
                b"1 A. N... 000000 100644 100644 aaa bbb staged.txt",
                b"1 .M N... 100644 100644 100644 aaa bbb modified.txt",
                b"1 MM N... 100644 100644 100644 aaa bbb both.txt",
            ],
        );
        let snapshot = parse_porcelain_v2(&bytes).unwrap();
        let staged = snapshot
            .entries
            .iter()
            .find(|e| e.path == "staged.txt")
            .unwrap();
        assert!(staged.is_staged() && !staged.is_unstaged() && !staged.is_untracked());
        let modified = snapshot
            .entries
            .iter()
            .find(|e| e.path == "modified.txt")
            .unwrap();
        assert!(modified.is_unstaged() && !modified.is_staged());
        let both = snapshot
            .entries
            .iter()
            .find(|e| e.path == "both.txt")
            .unwrap();
        assert!(both.is_staged() && both.is_unstaged());
        let untracked = snapshot
            .entries
            .iter()
            .find(|e| e.path == "untracked.txt")
            .unwrap();
        assert!(untracked.is_untracked() && !untracked.is_staged() && !untracked.is_unstaged());
    }

    #[test]
    fn parse_porcelain_v2_handles_rename_with_original_path() {
        let bytes = porcelain(
            "main",
            &[
                b"2 R. N... 100644 100644 100644 aaa bbb R100 renamed.txt",
                b"original.txt",
            ],
        );
        let snapshot = parse_porcelain_v2(&bytes).unwrap();
        let entry = snapshot.entries.first().unwrap();
        assert_eq!(entry.kind, GitEntryKind::Renamed);
        assert_eq!(entry.path, "renamed.txt");
        assert_eq!(entry.original_path.as_deref(), Some("original.txt"));
        assert!(entry.is_staged() && !entry.is_unstaged());
    }

    #[test]
    fn parse_porcelain_v2_preserves_whitespace_and_unicode() {
        let bytes = porcelain(
            "main",
            &[
                b"?  leading.txt",
                b"? trailing.txt ",
                b"? na\xc3\xafve\xf0\x9f\x98\x80.txt",
            ],
        );
        let snapshot = parse_porcelain_v2(&bytes).unwrap();
        assert!(snapshot.entries.iter().any(|e| e.path == " leading.txt"));
        assert!(snapshot.entries.iter().any(|e| e.path == "trailing.txt "));
        assert!(snapshot.entries.iter().any(|e| e.path == "naïve😀.txt"));
    }

    #[test]
    fn parse_porcelain_v2_reports_symbolic_detached_and_unborn() {
        let mut symbolic = Vec::new();
        symbolic.extend_from_slice(b"# branch.oid abc123\0# branch.head main\0");
        assert_eq!(
            parse_porcelain_v2(&symbolic).unwrap().branch,
            BranchState::Symbolic {
                name: "main".to_string(),
                oid: "abc123".to_string(),
            }
        );

        let mut detached = Vec::new();
        detached.extend_from_slice(b"# branch.oid abc123\0# branch.head (detached)\0");
        assert!(matches!(
            parse_porcelain_v2(&detached).unwrap().branch,
            BranchState::Detached { .. }
        ));

        let mut unborn = Vec::new();
        unborn.extend_from_slice(b"# branch.oid (initial)\0# branch.head trunk\0");
        assert_eq!(
            parse_porcelain_v2(&unborn).unwrap().branch,
            BranchState::Unborn {
                name: "trunk".to_string(),
            }
        );
    }

    #[test]
    fn parse_porcelain_v2_rejects_malformed_input_without_panicking() {
        let cases: &[(&[u8], GitParseErrorKind)] = &[
            (b"# branch.head main", GitParseErrorKind::Unterminated),
            (b"? orphan.txt\0", GitParseErrorKind::MissingHeader),
            (
                b"# branch.head main\0z junk\0",
                GitParseErrorKind::UnknownRecord,
            ),
            (
                b"# branch.head main\0#branch.head main\0",
                GitParseErrorKind::MalformedHeader,
            ),
            (
                b"# branch.head main\x001 X\x00",
                GitParseErrorKind::MalformedEntry,
            ),
            (
                b"# branch.head main\x002 R. N... 100644 100644 100644 aaa bbb R100 new\x00",
                GitParseErrorKind::MalformedEntry,
            ),
            (
                b"# branch.head main\0u UU N... 100644 100644 100644 100644 a b c \0",
                GitParseErrorKind::MalformedEntry,
            ),
            (b"# branch.head main\0\0", GitParseErrorKind::MalformedEntry),
            (
                b"# branch.head main\0?noseparator\0",
                GitParseErrorKind::MalformedEntry,
            ),
            (
                b"# branch.head main\x001 X N... 100644 100644 100644 aaa bbb p\x00",
                GitParseErrorKind::MalformedEntry,
            ),
        ];
        for (bytes, kind) in cases {
            let error = parse_porcelain_v2(bytes).unwrap_err();
            assert_eq!(error.kind, *kind, "for input {bytes:?}");
        }
    }

    #[test]
    fn parse_porcelain_v2_handles_ignored_entries() {
        let bytes = porcelain("main", &[b"! ignored.log"]);
        let snapshot = parse_porcelain_v2(&bytes).unwrap();
        let entry = snapshot.entries.first().unwrap();
        assert!(entry.is_ignored() && !entry.is_staged() && !entry.is_unstaged());
        assert_eq!(entry.status, [b'!', b'!']);
    }

    /// P2-b: an unborn branch must come from a real `# branch.oid (initial)`,
    /// not from an absent oid header.
    #[test]
    fn parse_porcelain_v2_requires_branch_oid_header() {
        let bytes = b"# branch.head main\0";
        let error = parse_porcelain_v2(bytes).unwrap_err();
        assert_eq!(error.kind, GitParseErrorKind::MissingHeader);
        assert!(
            error.detail.contains("branch.oid"),
            "detail should name the missing header: {}",
            error.detail
        );
        // A genuine unborn repository still parses.
        let mut real = Vec::new();
        real.extend_from_slice(b"# branch.oid (initial)\0# branch.head main\0");
        assert_eq!(
            parse_porcelain_v2(&real).unwrap().branch,
            BranchState::Unborn {
                name: "main".to_string()
            }
        );
    }

    #[test]
    fn accessors_and_retained_bytes_cover_every_shape() {
        let symbolic = GitSnapshot {
            branch: BranchState::Symbolic {
                name: "main".to_string(),
                oid: "abc".to_string(),
            },
            entries: vec![GitEntry {
                path: "renamed.txt".to_string(),
                original_path: Some("original.txt".to_string()),
                kind: GitEntryKind::Renamed,
                status: *b"R.",
            }],
        };
        assert_eq!(symbolic.branch.label(), "main");
        assert!(!symbolic.branch.is_detached());
        assert!(!symbolic.branch.is_unborn());
        assert!(symbolic.retained_bytes() > 0);

        let detached = BranchState::Detached {
            oid: "abc".to_string(),
        };
        assert_eq!(detached.label(), "HEAD (detached)");
        assert!(detached.is_detached());
        let unborn = BranchState::Unborn {
            name: "trunk".to_string(),
        };
        assert_eq!(unborn.label(), "trunk");
        assert!(unborn.is_unborn());

        let ignored = GitEntry {
            path: "ignored.log".to_string(),
            original_path: None,
            kind: GitEntryKind::Ignored,
            status: *b"!!",
        };
        assert!(ignored.is_ignored() && !ignored.is_conflicted());
        let unmerged = GitEntry {
            path: "conflict.txt".to_string(),
            original_path: None,
            kind: GitEntryKind::Unmerged,
            status: *b"UU",
        };
        assert!(unmerged.is_conflicted() && unmerged.is_staged() && unmerged.is_unstaged());

        assert!(GitResult::Snapshot(symbolic).retained_bytes() > 0);
        assert!(
            GitResult::Malformed(GitParseError {
                kind: GitParseErrorKind::MalformedEntry,
                detail: "detail".to_string(),
            })
            .retained_bytes()
                > 0
        );
        assert!(
            GitResult::Refused(GitRefusal::Failed {
                code: 3,
                detail: "boom".to_string(),
            })
            .retained_bytes()
                > 0
        );
        assert_eq!(
            GitResult::TimedOut.retained_bytes(),
            std::mem::size_of::<GitResult>()
        );
        let parse_error = GitParseError {
            kind: GitParseErrorKind::UnknownRecord,
            detail: "detail".to_string(),
        };
        assert!(parse_error.to_string().contains("UnknownRecord"));
        assert!(parse_error.retained_bytes() > 0);
        assert_eq!(GitState::new().issued(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn run_bounded_classifies_malformed_failed_and_pre_cancelled_outcomes() {
        let dir = tempfile::tempdir().unwrap();
        let malformed = run_bounded(
            "/bin/echo",
            &["not porcelain"],
            dir.path(),
            GitLimits::default(),
            &AtomicBool::new(false),
        );
        assert!(matches!(malformed, GitResult::Malformed(_)));
        let failed = run_bounded(
            "/bin/sh",
            &["-c", "echo boom >&2; exit 3"],
            dir.path(),
            GitLimits::default(),
            &AtomicBool::new(false),
        );
        match failed {
            GitResult::Refused(GitRefusal::Failed { code, detail }) => {
                assert_eq!(code, 3);
                assert!(detail.contains("boom"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        let cancelled = run_bounded(
            "/bin/echo",
            &["x"],
            dir.path(),
            GitLimits::default(),
            &AtomicBool::new(true),
        );
        assert_eq!(cancelled, GitResult::Cancelled);
    }

    #[test]
    fn pump_truncates_at_the_output_limit() {
        let limit = Arc::new(AtomicBool::new(false));
        let output = pump(std::io::repeat(b'x'), 32, limit.clone())
            .join()
            .unwrap();
        assert!(limit.load(Ordering::Acquire));
        assert_eq!(output.len(), 32);
    }

    /// P2-new-a: the stderr pump's 8 KiB bound has its own owner. Defeating the
    /// bound (for example to `usize::MAX`) must fail this test, not only the
    /// stdout path asserted above.
    #[test]
    fn stderr_pump_is_bounded_at_eight_kibibytes() {
        // Pin the production budget exactly: the call site uses this constant.
        assert_eq!(STDERR_BOUND_BYTES, 8192);
        let limit = Arc::new(AtomicBool::new(false));
        let output = pump(std::io::repeat(b'e'), STDERR_BOUND_BYTES, limit.clone())
            .join()
            .unwrap();
        assert_eq!(
            output.len(),
            STDERR_BOUND_BYTES,
            "stderr drain must stop at the {STDERR_BOUND_BYTES}-byte budget"
        );
        assert!(
            limit.load(Ordering::Acquire),
            "reaching the stderr budget must set the truncation flag"
        );
    }

    #[test]
    fn whitelist_allows_only_read_only_status_query() {
        assert!(is_read_only_invocation(&[
            "status",
            "--porcelain=v2",
            "-z",
            "--branch"
        ]));
        assert!(!is_read_only_invocation(&["status"]));
        assert_eq!(STATUS_ARGV[0], "--no-optional-locks");
        assert!(is_read_only_invocation(&STATUS_ARGV[1..]));
        for forbidden in [
            &["commit"][..],
            &["push"][..],
            &["checkout", "--", "."][..],
            &["reset", "--hard"][..],
            &["stash"][..],
            &["add", "."][..],
            &["rm", "file"][..],
            &["clean", "-f"][..],
        ] {
            assert!(!is_read_only_invocation(forbidden), "{forbidden:?}");
        }
        assert!(!STATUS_ARGV
            .iter()
            .any(
                |arg| ["commit", "push", "checkout", "reset", "stash", "add", "rm"].contains(arg)
            ));
    }

    /// P2-a pin: the production argument vector is derived from the whitelist,
    /// so this fails if `STATUS_ARGV` ever leaves the whitelisted query (for
    /// example, if a write verb is added to the status query source).
    #[test]
    fn whitelist_is_the_single_source_of_truth_for_the_production_argv() {
        assert_eq!(STATUS_ARGV[0], GLOBAL_ARGUMENTS[0]);
        assert_eq!(&STATUS_ARGV[1..], &STATUS_QUERY[..]);
        assert!(
            is_read_only_invocation(&STATUS_ARGV[1..]),
            "production argv {STATUS_ARGV:?} must be whitelisted"
        );
        assert!(
            READ_ONLY_ARGUMENTS.contains(&&STATUS_QUERY[..]),
            "STATUS_QUERY must be an entry of READ_ONLY_ARGUMENTS"
        );
    }

    /// P2-a fail-closed: the same runtime gate `status_bounded` applies must
    /// refuse a write vector in all build profiles (this exercises the exact
    /// predicate, not a `debug_assert!`, so it holds in release too).
    #[test]
    fn non_whitelisted_vectors_are_refused_by_the_runtime_gate() {
        let write_vector = ["commit", "--porcelain=v2", "-z", "--branch"];
        assert!(!is_read_only_invocation(&write_vector));
        assert!(is_read_only_invocation(&STATUS_QUERY));
        // The gate is a real runtime branch, not a proof-only assertion.
        let gate = |args: &[&str]| {
            if is_read_only_invocation(args) {
                Some(())
            } else {
                None
            }
        };
        assert!(gate(&write_vector).is_none());
        assert!(gate(&STATUS_QUERY).is_some());
    }

    #[test]
    fn status_never_executes_an_unwhitelisted_vector() {
        let executed = whitelisted_status_argv().expect("production argv must be whitelisted");
        assert_eq!(executed, STATUS_ARGV);
        assert!(is_read_only_invocation(&executed[1..]));
        assert_eq!(
            executed,
            [
                "--no-optional-locks",
                "status",
                "--porcelain=v2",
                "-z",
                "--branch"
            ]
        );
    }

    #[test]
    fn stale_results_can_never_overwrite_newer_state() {
        let mut state = GitState::new();
        let first = state.begin(PathBuf::from("/workspace/one"));
        let second = state.begin(PathBuf::from("/workspace/two"));
        assert!(second > first);
        let newer = GitRefresh {
            generation: second,
            root: PathBuf::from("/workspace/two"),
            result: GitResult::Snapshot(GitSnapshot {
                branch: BranchState::Symbolic {
                    name: "newer".to_string(),
                    oid: "b".to_string(),
                },
                entries: Vec::new(),
            }),
        };
        assert!(state.accept(newer.clone()));
        let stale = GitRefresh {
            generation: first,
            root: PathBuf::from("/workspace/one"),
            result: GitResult::Snapshot(GitSnapshot {
                branch: BranchState::Symbolic {
                    name: "stale".to_string(),
                    oid: "a".to_string(),
                },
                entries: Vec::new(),
            }),
        };
        assert!(!state.accept(stale));
        assert_eq!(state.root(), Some(Path::new("/workspace/two")));
        assert_eq!(state.snapshot().unwrap().branch.label(), "newer");
        // A duplicate delivery of the accepted generation is also refused.
        assert!(!state.accept(newer));
    }

    #[test]
    fn non_snapshot_results_clear_retained_state() {
        let mut state = GitState::new();
        let generation = state.begin(PathBuf::from("/workspace"));
        assert!(state.accept(GitRefresh {
            generation,
            root: PathBuf::from("/workspace"),
            result: GitResult::Snapshot(GitSnapshot {
                branch: BranchState::Unborn {
                    name: "main".to_string(),
                },
                entries: Vec::new(),
            }),
        }));
        assert!(state.snapshot().is_some());
        let next = state.begin(PathBuf::from("/workspace"));
        assert!(state.accept(GitRefresh {
            generation: next,
            root: PathBuf::from("/workspace"),
            result: GitResult::Refused(GitRefusal::NotARepository),
        }));
        assert!(state.snapshot().is_none());
    }

    /// Folded from the Phase 8 Task 2 review: issuing a generation for a
    /// *different* workspace root must drop the retained snapshot immediately.
    /// Otherwise a render between the new `begin` and the arrival of its result
    /// would compare the new root against itself in the App-level guard and
    /// recolor the tree with the previous repository's snapshot. A refresh for
    /// the same root keeps the retained snapshot so ordinary re-requests do not
    /// flicker.
    #[test]
    fn begin_clears_retained_snapshot_when_the_root_changes() {
        let mut state = GitState::new();
        let first = state.begin(PathBuf::from("/workspace/one"));
        assert!(state.accept(GitRefresh {
            generation: first,
            root: PathBuf::from("/workspace/one"),
            result: GitResult::Snapshot(GitSnapshot {
                branch: BranchState::Symbolic {
                    name: "one".to_string(),
                    oid: "a".to_string(),
                },
                entries: Vec::new(),
            }),
        }));
        assert!(state.snapshot().is_some());

        // A new root supersedes the old root's retained snapshot at once.
        let _ = state.begin(PathBuf::from("/workspace/two"));
        assert!(
            state.snapshot().is_none(),
            "a root change must not retain the previous repository's snapshot"
        );

        // A refresh for the same root keeps the retained snapshot.
        let second = state.begin(PathBuf::from("/workspace/two"));
        assert!(state.accept(GitRefresh {
            generation: second,
            root: PathBuf::from("/workspace/two"),
            result: GitResult::Snapshot(GitSnapshot {
                branch: BranchState::Symbolic {
                    name: "two".to_string(),
                    oid: "b".to_string(),
                },
                entries: Vec::new(),
            }),
        }));
        let _ = state.begin(PathBuf::from("/workspace/two"));
        assert!(
            state.snapshot().is_some(),
            "a same-root refresh must not clear the snapshot"
        );
    }

    fn git(dir: &Path, args: &[&str]) -> std::process::Output {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("LC_ALL", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("git must be installed for fixtures")
    }

    fn git_ok(dir: &Path, args: &[&str]) {
        let output = git(dir, args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git_ok(dir.path(), &["init", "-q"]);
        dir
    }

    fn commit_all(dir: &Path, message: &str) {
        git_ok(dir, &["add", "-A"]);
        git_ok(dir, &["commit", "-q", "-m", message]);
    }

    fn snapshot_of(dir: &Path) -> GitSnapshot {
        match status_bounded(dir, GitLimits::default(), &AtomicBool::new(false)) {
            GitResult::Snapshot(snapshot) => snapshot,
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    fn entry<'a>(snapshot: &'a GitSnapshot, path: &str) -> &'a GitEntry {
        snapshot
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .unwrap_or_else(|| panic!("missing entry {path:?} in {:?}", snapshot.entries))
    }

    #[test]
    fn status_bounded_reads_a_temporary_repository() {
        let dir = repo();
        std::fs::write(dir.path().join("tracked.txt"), b"one").unwrap();
        commit_all(dir.path(), "base");
        std::fs::write(dir.path().join("tracked.txt"), b"two").unwrap();
        std::fs::write(dir.path().join("staged.txt"), b"new").unwrap();
        git_ok(dir.path(), &["add", "staged.txt"]);
        std::fs::write(dir.path().join("untracked.txt"), b"new").unwrap();

        let snapshot = snapshot_of(dir.path());
        assert!(matches!(snapshot.branch, BranchState::Symbolic { .. }));
        let staged = entry(&snapshot, "staged.txt");
        assert!(staged.is_staged() && !staged.is_unstaged() && !staged.is_untracked());
        let modified = entry(&snapshot, "tracked.txt");
        assert!(modified.is_unstaged() && !modified.is_staged());
        let untracked = entry(&snapshot, "untracked.txt");
        assert!(untracked.is_untracked() && !untracked.is_staged() && !untracked.is_unstaged());
    }

    #[test]
    fn status_bounded_detects_conflict_and_rename() {
        let dir = repo();
        std::fs::write(dir.path().join("a.txt"), b"base").unwrap();
        commit_all(dir.path(), "base");
        git_ok(dir.path(), &["checkout", "-q", "-b", "side"]);
        std::fs::write(dir.path().join("a.txt"), b"side").unwrap();
        commit_all(dir.path(), "side");
        git_ok(dir.path(), &["checkout", "-q", "-"]);
        std::fs::write(dir.path().join("a.txt"), b"main").unwrap();
        commit_all(dir.path(), "main");
        let merge = git(dir.path(), &["merge", "side"]);
        assert!(!merge.status.success(), "merge should conflict");

        let conflicted = snapshot_of(dir.path());
        assert!(conflicted.entries.iter().any(GitEntry::is_conflicted));

        git_ok(dir.path(), &["merge", "--abort"]);
        git_ok(dir.path(), &["mv", "a.txt", "renamed.txt"]);
        let renamed = snapshot_of(dir.path());
        let entry = entry(&renamed, "renamed.txt");
        assert_eq!(entry.kind, GitEntryKind::Renamed);
        assert_eq!(entry.original_path.as_deref(), Some("a.txt"));
        assert!(entry.is_staged());
    }

    #[test]
    fn status_bounded_reports_unborn_and_detached() {
        let dir = repo();
        assert!(matches!(
            snapshot_of(dir.path()).branch,
            BranchState::Unborn { .. }
        ));
        std::fs::write(dir.path().join("a.txt"), b"base").unwrap();
        commit_all(dir.path(), "base");
        assert!(matches!(
            snapshot_of(dir.path()).branch,
            BranchState::Symbolic { .. }
        ));
        git_ok(dir.path(), &["checkout", "-q", "--detach", "HEAD"]);
        assert!(matches!(
            snapshot_of(dir.path()).branch,
            BranchState::Detached { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn status_bounded_reads_real_unicode_spaces_and_newline_paths() {
        let dir = repo();
        for name in [
            "naïve😀.txt",
            "sp ace.txt",
            " lead trail ",
            "line\nbreak.txt",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let snapshot = snapshot_of(dir.path());
        for name in [
            "naïve😀.txt",
            "sp ace.txt",
            " lead trail ",
            "line\nbreak.txt",
        ] {
            assert!(entry(&snapshot, name).is_untracked(), "missing {name:?}");
        }
    }

    #[test]
    fn status_bounded_refuses_missing_executable() {
        let dir = tempfile::tempdir().unwrap();
        let result = run_bounded(
            "fm-git-missing-executable-xyz",
            &["status"],
            dir.path(),
            GitLimits::default(),
            &AtomicBool::new(false),
        );
        assert_eq!(result, GitResult::Refused(GitRefusal::ExecutableMissing));
    }

    #[test]
    fn status_bounded_refuses_non_repository() {
        let dir = tempfile::tempdir().unwrap();
        let result = status_bounded(dir.path(), GitLimits::default(), &AtomicBool::new(false));
        assert_eq!(result, GitResult::Refused(GitRefusal::NotARepository));
    }

    #[test]
    fn status_bounded_bounds_a_verbose_repository() {
        let dir = repo();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        commit_all(dir.path(), "base");
        std::fs::write(dir.path().join("a.txt"), b"y").unwrap();
        let result = status_bounded(
            dir.path(),
            GitLimits {
                output_bytes: 16,
                timeout: Duration::from_secs(3),
            },
            &AtomicBool::new(false),
        );
        assert_eq!(result, GitResult::OutputTooLarge { limit: 16 });
    }

    /// P3-new-a honesty pin: a child that floods stderr past the 8 KiB drain
    /// bound and then exits must still report its real exit status rather than
    /// `TimedOut`. `try_wait` is polled independently of the pipes, and the
    /// stderr reader dropping its read end surfaces EPIPE to the child.
    #[cfg(unix)]
    #[test]
    fn stderr_flood_then_clean_exit_reports_the_real_status() {
        let dir = tempfile::tempdir().unwrap();
        let result = run_bounded(
            "/bin/sh",
            &["-c", "head -c 204800 /dev/zero | tr '\\0' 'e' >&2; exit 7"],
            dir.path(),
            GitLimits {
                output_bytes: 1024,
                timeout: Duration::from_secs(3),
            },
            &AtomicBool::new(false),
        );
        match result {
            GitResult::Refused(GitRefusal::Failed { code, detail }) => {
                assert_eq!(code, 7, "real exit status must be reported");
                assert!(
                    detail.chars().count() <= 512,
                    "diagnostic must stay bounded: {} chars",
                    detail.chars().count()
                );
            }
            other => panic!("expected the real exit status, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn run_bounded_times_out_on_a_blocking_child() {
        let dir = tempfile::tempdir().unwrap();
        // Bounded watchdog: if the deadline regresses the test fails fast
        // instead of hanging the suite on the blocking `/dev/zero` reader.
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let root = dir.path().to_path_buf();
        let worker = std::thread::spawn(move || {
            let result = run_bounded(
                "/bin/sh",
                &["-c", "read _ < /dev/zero"],
                &root,
                GitLimits {
                    output_bytes: 1024,
                    timeout: Duration::from_millis(60),
                },
                &AtomicBool::new(false),
            );
            let _ = result_tx.send(result);
        });
        let result = result_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("run_bounded deadline regressed: no result within the watchdog");
        worker.join().unwrap();
        assert_eq!(result, GitResult::TimedOut);
    }

    #[cfg(unix)]
    #[test]
    fn run_bounded_cancels_and_reaps_a_blocking_child() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("started");
        let script = format!("touch {}; read _ < /dev/zero", sentinel.display());
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let root = dir.path().to_path_buf();
        let handle = std::thread::spawn(move || {
            run_bounded(
                "/bin/sh",
                &["-c", script.as_str()],
                &root,
                GitLimits {
                    output_bytes: 1024,
                    timeout: Duration::from_secs(30),
                },
                &flag,
            )
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !sentinel.exists() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(sentinel.exists(), "blocking child never started");
        cancelled.store(true, Ordering::SeqCst);
        assert_eq!(handle.join().unwrap(), GitResult::Cancelled);
    }
}
