use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};

use crate::event::Event;

/// Default patterns to ignore when watching the filesystem.
#[allow(dead_code)]
pub const DEFAULT_IGNORE_PATTERNS: &[&str] = &[
    ".git",
    "node_modules",
    "__pycache__",
    "venv",
    ".venv",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    "target",
];

/// Default debounce interval in milliseconds.
#[allow(dead_code)]
pub const DEFAULT_DEBOUNCE_MS: u64 = 300;

/// Default flood threshold (events per debounce window).
#[allow(dead_code)]
pub const DEFAULT_FLOOD_THRESHOLD: usize = 100;

/// Default entry budget for one recursive polling scan.
pub const DEFAULT_POLL_MAX_ENTRIES: usize = 20_000;

/// Maximum recursion depth for a polling scan (symlinks are not followed).
pub const POLL_MAX_DEPTH: usize = 32;

/// Filesystem watcher that monitors a root directory and sends change events.
#[allow(dead_code)]
pub struct FsWatcher {
    /// Whether the watcher is currently forwarding events.
    active: Arc<AtomicBool>,
    /// Handle to the debouncer (dropped to stop watching).
    _debouncer: notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>,
    output: crate::event::EventSender,
}

#[allow(dead_code)]
impl FsWatcher {
    /// Create a new FsWatcher that watches `root` recursively.
    ///
    /// Events are debounced by `debounce_duration` and sent via `event_tx`.
    /// Paths matching any of `ignore_patterns` are silently dropped.
    /// If more than `flood_threshold` events arrive in a single debounce window,
    /// they are collapsed into a single full-refresh event (root path only).
    pub fn new(
        root: &Path,
        debounce_duration: Duration,
        ignore_patterns: Vec<String>,
        flood_threshold: usize,
        event_tx: crate::event::EventSender,
    ) -> notify::Result<Self> {
        let active = Arc::new(AtomicBool::new(true));
        let active_clone = active.clone();
        let root_path = root.to_path_buf();
        let output = event_tx.producer();
        let event_tx = output.clone();

        let mut debouncer = new_debouncer(
            debounce_duration,
            move |result: Result<Vec<notify_debouncer_mini::DebouncedEvent>, notify::Error>| {
                // If paused, silently drop events
                if !active_clone.load(Ordering::Relaxed) {
                    return;
                }

                match result {
                    Ok(events) => {
                        let paths: Vec<PathBuf> = events
                            .iter()
                            .filter(|e| e.kind == DebouncedEventKind::Any)
                            .filter(|e| !should_ignore(&e.path, &ignore_patterns))
                            .take(flood_threshold.saturating_add(1))
                            .map(|e| e.path.clone())
                            .collect();

                        if paths.is_empty() {
                            return;
                        }

                        // Flood protection: if too many events, collapse to root refresh
                        let final_paths = if paths.len() > flood_threshold {
                            vec![root_path.clone()]
                        } else {
                            paths
                        };

                        let event = Event::FsChange(final_paths);
                        let event = if event.retained_bytes() > event_tx.max_event_bytes() {
                            Event::FsChange(vec![root_path.clone()])
                        } else {
                            event
                        };
                        let _ = event_tx.blocking_send(event);
                    }
                    Err(_errors) => {
                        // Watcher errors are non-fatal; silently ignore
                    }
                }
            },
        )?;

        if let Err(error) = debouncer
            .watcher()
            .watch(root, notify::RecursiveMode::Recursive)
        {
            output.stop();
            return Err(error);
        }

        Ok(Self {
            active,
            _debouncer: debouncer,
            output,
        })
    }

    /// Pause event forwarding (watcher stays alive to avoid re-creating inotify watches).
    ///
    /// **Deliberate retained API, not a test seam.** Phase 6 Task 4 stopped
    /// coupling the backend to the in-app tree-refresh toggle on purpose: the
    /// backend must keep forwarding so an open editor still notices external
    /// writes while the tree refresh is manual. These two methods are kept as
    /// the supported way to silence a live backend without tearing down its
    /// watches (for example a future explicit "pause watching" command), and
    /// `pause`/`resume` are exercised through the exposed
    /// [`Self::active_flag`]. Full disable (`--no-watcher` /
    /// `watcher.enabled = false`) spawns no backend at all.
    pub fn pause(&self) {
        self.active.store(false, Ordering::Relaxed);
    }

    /// Resume event forwarding after [`Self::pause`]; see its doc comment for
    /// why this retained API exists. Idempotent.
    pub fn resume(&self) {
        self.active.store(true, Ordering::Relaxed);
    }

    /// Get a clone of the internal active flag used by the watcher callback.
    pub fn active_flag(&self) -> Arc<AtomicBool> {
        self.active.clone()
    }

    /// Check if the watcher is currently active (forwarding events).
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }
}

impl Drop for FsWatcher {
    fn drop(&mut self) {
        // Wake a backpressured callback before the debouncer joins its thread.
        self.output.stop();
    }
}

/// One directory entry as observed by a polling scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PollEntry {
    pub name: OsString,
    pub is_dir: bool,
    pub len: u64,
    pub modified: Option<SystemTime>,
}

/// Bounded recursive listing of a workspace root.
///
/// Only names and cheap metadata are kept so two scans can be diffed without
/// reading file contents. `incomplete` is set when any listing failed or the
/// entry budget was hit; callers then treat absence of an entry as unknown
/// rather than as a deletion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PollSnapshot {
    pub dirs: BTreeMap<PathBuf, Vec<PollEntry>>,
    pub incomplete: bool,
}

/// Bounded recursive scan used by the polling backend.
///
/// `max_entries` caps the total number of entries retained across all
/// directories; `max_depth` bounds recursion (symlinked directories are not
/// followed). Ignored components are skipped entirely.
pub fn scan_poll_snapshot(
    root: &Path,
    ignore: &[String],
    max_entries: usize,
    max_depth: usize,
) -> PollSnapshot {
    let mut snapshot = PollSnapshot::default();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    let mut total = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        if snapshot.dirs.contains_key(&dir) {
            continue;
        }
        let read = match std::fs::read_dir(&dir) {
            Ok(read) => read,
            Err(_) => {
                snapshot.incomplete = true;
                snapshot.dirs.insert(dir, Vec::new());
                continue;
            }
        };
        let mut entries: Vec<PollEntry> = Vec::new();
        for entry in read {
            if total >= max_entries {
                snapshot.incomplete = true;
                break;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    snapshot.incomplete = true;
                    continue;
                }
            };
            let name = entry.file_name();
            let path = dir.join(&name);
            if should_ignore(&path, ignore) {
                continue;
            }
            let is_dir = entry.file_type().map(|k| k.is_dir()).unwrap_or(false);
            // DirEntry::metadata is lstat-like: a symlink is measured as a link,
            // never followed, so loops and retargets cannot recurse forever.
            let (len, modified) = match entry.metadata() {
                Ok(meta) => (meta.len(), meta.modified().ok()),
                Err(_) => (0, None),
            };
            total += 1;
            if is_dir && depth < max_depth {
                stack.push((path, depth + 1));
            }
            entries.push(PollEntry {
                name,
                is_dir,
                len,
                modified,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        snapshot.dirs.insert(dir, entries);
    }
    snapshot
}

/// Merge two name-sorted entry lists and record the changed entry paths.
fn diff_entries(
    dir: &Path,
    previous: &[PollEntry],
    current: &[PollEntry],
    changed: &mut std::collections::BTreeSet<PathBuf>,
) {
    let (mut i, mut j) = (0usize, 0usize);
    while i < previous.len() && j < current.len() {
        match previous[i].name.cmp(&current[j].name) {
            std::cmp::Ordering::Equal => {
                if previous[i] != current[j] {
                    changed.insert(dir.join(&current[j].name));
                }
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => {
                changed.insert(dir.join(&previous[i].name));
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                changed.insert(dir.join(&current[j].name));
                j += 1;
            }
        }
    }
    while i < previous.len() {
        changed.insert(dir.join(&previous[i].name));
        i += 1;
    }
    while j < current.len() {
        changed.insert(dir.join(&current[j].name));
        j += 1;
    }
}

/// Polling watcher backend.
///
/// Instead of relying on OS notifications, it re-scans the workspace every
/// interval and diffs the result. Because each scan is authoritative, events
/// that the native backend lost or coalesced are repaired on the next poll.
/// The scan is driven by an injected clock through [`PollingWatcher::pump`];
/// production uses [`PollingWatcher::run_blocking`].
pub struct PollingWatcher {
    root: PathBuf,
    ignore: Vec<String>,
    max_entries: usize,
    flood_threshold: usize,
    interval: Duration,
    known: PollSnapshot,
    next_due: Option<Duration>,
    active: Arc<AtomicBool>,
    output: crate::event::EventSender,
}

#[allow(dead_code)]
impl PollingWatcher {
    /// Production constructor. A missing root is not an error: the watcher
    /// starts empty and reports the root once it appears.
    pub fn new(
        root: &Path,
        interval: Duration,
        ignore_patterns: Vec<String>,
        flood_threshold: usize,
        max_entries: usize,
        event_tx: crate::event::EventSender,
    ) -> Self {
        let known = scan_poll_snapshot(root, &ignore_patterns, max_entries, POLL_MAX_DEPTH);
        Self::from_snapshot(
            root,
            interval,
            ignore_patterns,
            flood_threshold,
            max_entries,
            known,
            event_tx,
        )
    }

    /// Test seam: seed the known listing without touching the filesystem.
    pub(crate) fn from_snapshot(
        root: &Path,
        interval: Duration,
        ignore_patterns: Vec<String>,
        flood_threshold: usize,
        max_entries: usize,
        known: PollSnapshot,
        event_tx: crate::event::EventSender,
    ) -> Self {
        Self {
            root: root.to_path_buf(),
            ignore: ignore_patterns,
            max_entries,
            flood_threshold,
            interval,
            known,
            next_due: None,
            active: Arc::new(AtomicBool::new(true)),
            output: event_tx.producer(),
        }
    }

    /// Pause emission while keeping the backend alive.
    ///
    /// **Deliberate retained API, not a test seam.** Task 4 intentionally does
    /// not tie emission to the in-app tree-refresh toggle, so document change
    /// detection keeps working while the tree refresh is manual. This is the
    /// supported way to silence a live polling backend without stopping its
    /// thread; the coordinator can drive it through [`Self::active_flag`].
    pub fn pause(&self) {
        self.active.store(false, Ordering::Relaxed);
    }

    /// Resume emission after [`Self::pause`]; see its doc comment for why this
    /// retained API exists. Idempotent.
    pub fn resume(&self) {
        self.active.store(true, Ordering::Relaxed);
    }

    /// Shared flag so the coordinator can reflect the live auto-refresh policy.
    pub fn active_flag(&self) -> Arc<AtomicBool> {
        self.active.clone()
    }

    /// Whether the backend is currently emitting.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    /// The interval between scans.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Injected-clock poll. Runs the supplied backend exactly when the interval
    /// has elapsed and returns the bounded changed paths (or the root when the
    /// change set floods). Tests advance `now` and supply fake snapshots, so no
    /// sleeps or real I/O are needed.
    pub fn pump(
        &mut self,
        now: Duration,
        scan: impl FnOnce() -> PollSnapshot,
    ) -> Option<Vec<PathBuf>> {
        let due = self.next_due.is_none_or(|deadline| now >= deadline);
        if !due {
            return None;
        }
        self.next_due = Some(now.saturating_add(self.interval));
        if !self.active.load(Ordering::Relaxed) {
            return None;
        }
        Some(self.poll_once(scan()))
    }

    /// Diff `current` against the last known scan and update the baseline.
    ///
    /// Returns the specific entry paths that were added, removed, or changed
    /// (files and directories) so the coordinator can remap each to its parent
    /// row and match open documents exactly. Collapses to the root when the
    /// change set exceeds the flood threshold.
    pub fn poll_once(&mut self, current: PollSnapshot) -> Vec<PathBuf> {
        // Treat any incomplete scan as "everything might have changed": after a
        // cap or a transient error, absence is not evidence of deletion.
        if current.incomplete || self.known.incomplete {
            self.known = current;
            return vec![self.root.clone()];
        }
        let mut changed: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
        for (dir, entries) in &current.dirs {
            match self.known.dirs.get(dir) {
                Some(previous) => diff_entries(dir, previous, entries, &mut changed),
                None => {
                    // A directory seen for the first time: its creation is the
                    // change; its parent's listing diff reports the entry too.
                    changed.insert(dir.clone());
                }
            }
        }
        for dir in self.known.dirs.keys() {
            if !current.dirs.contains_key(dir) {
                changed.insert(dir.clone());
            }
        }
        self.known = current;
        if changed.len() > self.flood_threshold {
            vec![self.root.clone()]
        } else {
            changed.into_iter().collect()
        }
    }

    /// Fresh real scan of the workspace root.
    pub fn scan(&self) -> PollSnapshot {
        scan_poll_snapshot(&self.root, &self.ignore, self.max_entries, POLL_MAX_DEPTH)
    }

    fn emit(&self, paths: Vec<PathBuf>) {
        let event = Event::FsChange(paths);
        let event = if event.retained_bytes() > self.output.max_event_bytes() {
            Event::FsChange(vec![self.root.clone()])
        } else {
            event
        };
        let _ = self.output.blocking_send(event);
    }

    /// Production driver: scan every interval until the transport closes.
    ///
    /// A missing or erroring root simply produces an incomplete (or empty) scan
    /// and the loop sleeps again, so it can neither panic nor busy-loop.
    pub fn run_blocking(&mut self) {
        while !self.output.is_closed() {
            std::thread::sleep(self.interval);
            if self.output.is_closed() {
                break;
            }
            if !self.active.load(Ordering::Relaxed) {
                continue;
            }
            let snapshot = self.scan();
            let changed = self.poll_once(snapshot);
            if !changed.is_empty() {
                self.emit(changed);
            }
        }
    }
}

impl Drop for PollingWatcher {
    fn drop(&mut self) {
        // Wake a backpressured sender before the driver thread observes closure.
        self.output.stop();
    }
}

/// Check if a path should be ignored based on ignore patterns.
///
/// A path is ignored if any of its components match any ignore pattern exactly.
#[allow(dead_code)]
pub fn should_ignore(path: &Path, patterns: &[String]) -> bool {
    for component in path.components() {
        if let std::path::Component::Normal(name) = component {
            let name_str = name.to_string_lossy();
            for pattern in patterns {
                if name_str == *pattern {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn transport_watcher_drop_wakes_its_callback_without_closing_other_producers() {
        let root = tempfile::tempdir().unwrap();
        let (tx, mut rx) = crate::event::event_channel(crate::event::TransportLimits {
            slots: 1,
            ..Default::default()
        });
        tx.send(Event::Resize(80, 24)).await.unwrap();
        let watcher = FsWatcher::new(
            root.path(),
            Duration::from_millis(10),
            vec![],
            100,
            tx.clone(),
        )
        .unwrap();
        let callback = watcher.output.clone();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn(move || {
            entered.send(()).unwrap();
            callback.blocking_send(Event::FsChange(vec!["callback-path".into()]))
        });
        ready.await.unwrap();
        drop(watcher);
        assert_eq!(
            tokio::task::spawn_blocking(move || thread.join().unwrap())
                .await
                .unwrap(),
            Err(crate::event::SendError::Closed)
        );
        assert!(!tx.is_closed());
        assert!(matches!(rx.recv().await, Some(Event::Resize(80, 24))));
        tx.send(Event::Resize(40, 12)).await.unwrap();
        assert!(matches!(rx.recv().await, Some(Event::Resize(40, 12))));
    }

    #[tokio::test]
    async fn transport_native_watcher_delivers_bounded_paths_then_closes() {
        let root = tempfile::tempdir().unwrap();
        let (tx, mut rx) = crate::event::event_channel(Default::default());
        let watcher =
            FsWatcher::new(root.path(), Duration::from_millis(10), vec![], 100, tx).unwrap();
        let path = root.path().join("created");
        std::fs::write(&path, b"fixture").unwrap();
        let event = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event, Event::FsChange(paths) if paths.contains(&path)));
        rx.close();
        drop(watcher);
    }

    #[test]
    fn ignore_git_directory() {
        let patterns = vec![".git".to_string()];
        assert!(should_ignore(
            Path::new("/home/user/project/.git/HEAD"),
            &patterns
        ));
        assert!(should_ignore(
            Path::new("/home/user/project/.git/objects/abc"),
            &patterns
        ));
    }

    #[test]
    fn ignore_node_modules() {
        let patterns = vec!["node_modules".to_string()];
        assert!(should_ignore(
            Path::new("/project/node_modules/express/index.js"),
            &patterns
        ));
    }

    #[test]
    fn ignore_target_dir() {
        let patterns = vec!["target".to_string()];
        assert!(should_ignore(
            Path::new("/project/target/debug/binary"),
            &patterns
        ));
    }

    #[test]
    fn do_not_ignore_normal_paths() {
        let patterns = vec![".git".to_string(), "node_modules".to_string()];
        assert!(!should_ignore(
            Path::new("/home/user/project/src/main.rs"),
            &patterns
        ));
        assert!(!should_ignore(
            Path::new("/home/user/project/README.md"),
            &patterns
        ));
    }

    #[test]
    fn empty_patterns_ignore_nothing() {
        let patterns: Vec<String> = vec![];
        assert!(!should_ignore(Path::new("/project/.git/HEAD"), &patterns));
    }

    #[test]
    fn multiple_patterns() {
        let patterns = vec![
            ".git".to_string(),
            "node_modules".to_string(),
            "__pycache__".to_string(),
            "target".to_string(),
        ];
        assert!(should_ignore(Path::new("/p/.git/refs"), &patterns));
        assert!(should_ignore(Path::new("/p/node_modules/x"), &patterns));
        assert!(should_ignore(
            Path::new("/p/src/__pycache__/mod.pyc"),
            &patterns
        ));
        assert!(should_ignore(Path::new("/p/target/release/bin"), &patterns));
        assert!(!should_ignore(Path::new("/p/src/lib.rs"), &patterns));
    }

    #[test]
    fn partial_name_does_not_match() {
        let patterns = vec!["target".to_string()];
        // "target2" should NOT be ignored — exact component match required
        assert!(!should_ignore(
            Path::new("/project/target2/file.txt"),
            &patterns
        ));
    }

    #[test]
    fn flood_threshold_collapses_events() {
        // This tests the logic conceptually — the actual threshold is applied in the callback.
        let paths: Vec<PathBuf> = (0..200)
            .map(|i| PathBuf::from(format!("/tmp/file_{}", i)))
            .collect();
        let threshold = 100;
        let root = PathBuf::from("/tmp");

        let final_paths = if paths.len() > threshold {
            vec![root.clone()]
        } else {
            paths.clone()
        };

        assert_eq!(final_paths.len(), 1);
        assert_eq!(final_paths[0], root);
    }

    #[test]
    fn below_flood_threshold_keeps_individual_paths() {
        let paths: Vec<PathBuf> = (0..50)
            .map(|i| PathBuf::from(format!("/tmp/file_{}", i)))
            .collect();
        let threshold = 100;
        let root = PathBuf::from("/tmp");

        let final_paths = if paths.len() > threshold {
            vec![root]
        } else {
            paths.clone()
        };

        assert_eq!(final_paths.len(), 50);
    }

    // === Polling backend tests (injected clock/backend; no sleeps) ===

    fn entry(name: &str, is_dir: bool, len: u64) -> PollEntry {
        PollEntry {
            name: OsString::from(name),
            is_dir,
            len,
            modified: None,
        }
    }

    fn snapshot_with(dirs: &[(&Path, Vec<PollEntry>)]) -> PollSnapshot {
        PollSnapshot {
            dirs: dirs
                .iter()
                .map(|(path, entries)| ((*path).to_path_buf(), entries.clone()))
                .collect(),
            incomplete: false,
        }
    }

    fn polling(root: &Path, known: PollSnapshot) -> PollingWatcher {
        let (tx, _rx) = crate::event::event_channel(Default::default());
        PollingWatcher::from_snapshot(
            root,
            Duration::from_millis(1_000),
            vec![],
            100,
            1_000,
            known,
            tx,
        )
    }

    #[test]
    fn polling_repairs_lost_and_coalesced_events_without_sleeps() {
        let root = PathBuf::from("/ws");
        let initial = snapshot_with(&[(&root, vec![entry("a", false, 1)])]);
        let mut watcher = polling(&root, initial.clone());

        // First due poll establishes the schedule (no change yet).
        assert!(watcher
            .pump(Duration::from_millis(0), || initial.clone())
            .unwrap()
            .is_empty());
        // Before the next interval elapses the backend must not be consulted.
        assert!(watcher
            .pump(Duration::from_millis(500), || {
                panic!("backend consulted before the interval elapsed")
            })
            .is_none());

        // A lost event plus a coalesced event between polls are both repaired by
        // one authoritative diff.
        let next = snapshot_with(&[(&root, vec![entry("a", false, 2), entry("b", false, 1)])]);
        assert_eq!(
            watcher
                .pump(Duration::from_millis(1_000), || next.clone())
                .unwrap(),
            vec![root.join("a"), root.join("b")]
        );
        // The same snapshot on the next interval is quiet: no stale re-report.
        assert!(watcher
            .pump(Duration::from_millis(2_000), || next)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn polling_content_change_is_detected_by_metadata() {
        let root = PathBuf::from("/ws");
        let initial = snapshot_with(&[(&root, vec![entry("a", false, 1)])]);
        let mut watcher = polling(&root, initial);
        let changed = snapshot_with(&[(&root, vec![entry("a", false, 42)])]);
        assert_eq!(
            watcher.pump(Duration::ZERO, || changed.clone()).unwrap(),
            vec![root.join("a")]
        );
    }

    #[test]
    fn polling_deleted_and_recreated_root_never_panics() {
        let root = PathBuf::from("/ws");
        let initial = snapshot_with(&[(&root, vec![entry("a", false, 1)])]);
        let mut watcher = polling(&root, initial);

        // Root deleted: an empty, non-incomplete scan reports the root once.
        let gone = PollSnapshot {
            dirs: BTreeMap::new(),
            incomplete: false,
        };
        assert_eq!(
            watcher.pump(Duration::ZERO, || gone.clone()).unwrap(),
            vec![root.clone()]
        );
        // Still gone: quiet, no busy loop or repeated panic.
        assert!(watcher
            .pump(Duration::from_secs(1), || gone)
            .unwrap()
            .is_empty());

        // Root re-created with fresh content is reported exactly once.
        let back = snapshot_with(&[(&root, vec![entry("c", false, 1)])]);
        assert_eq!(
            watcher
                .pump(Duration::from_secs(2), || back.clone())
                .unwrap(),
            vec![root.clone()]
        );
        assert!(watcher
            .pump(Duration::from_secs(3), || back)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn polling_incomplete_scan_reports_root_instead_of_false_deletions() {
        let root = PathBuf::from("/ws");
        let initial = snapshot_with(&[
            (&root, vec![entry("a", false, 1)]),
            (Path::new("/ws/sub"), vec![entry("x", false, 1)]),
        ]);
        let mut watcher = polling(&root, initial);
        let capped = PollSnapshot {
            dirs: BTreeMap::new(),
            incomplete: true,
        };
        assert_eq!(
            watcher.pump(Duration::ZERO, || capped).unwrap(),
            vec![root.clone()]
        );
    }

    #[test]
    fn polling_flood_collapses_to_root() {
        let root = PathBuf::from("/ws");
        let mut watcher = polling(&root, PollSnapshot::default());
        let mut dirs: Vec<(PathBuf, Vec<PollEntry>)> = vec![(root.clone(), vec![])];
        for n in 0..200 {
            dirs.push((root.join(format!("d{n}")), vec![]));
        }
        let refs: Vec<(&Path, Vec<PollEntry>)> =
            dirs.iter().map(|(p, e)| (p.as_path(), e.clone())).collect();
        let flooded = snapshot_with(&refs);
        assert_eq!(
            watcher.pump(Duration::ZERO, || flooded).unwrap(),
            vec![root.clone()]
        );
    }

    #[test]
    fn polling_pause_suppresses_emission_until_resume() {
        let root = PathBuf::from("/ws");
        let initial = snapshot_with(&[(&root, vec![entry("a", false, 1)])]);
        let mut watcher = polling(&root, initial);
        watcher.pause();
        assert!(!watcher.is_active());
        let next = snapshot_with(&[(&root, vec![entry("b", false, 1)])]);
        assert!(watcher.pump(Duration::ZERO, || next.clone()).is_none());
        watcher.resume();
        assert!(watcher.is_active());
        assert_eq!(
            watcher.pump(Duration::from_secs(1), || next).unwrap(),
            vec![root.join("a"), root.join("b")]
        );
    }

    #[test]
    fn polling_scan_skips_ignored_dirs_and_finds_content() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target").join("secret"), b"x").unwrap();
        std::fs::write(dir.path().join("visible"), b"y").unwrap();
        let snapshot =
            scan_poll_snapshot(dir.path(), &["target".to_string()], 1_000, POLL_MAX_DEPTH);
        assert!(snapshot.dirs.contains_key(dir.path()));
        assert!(!snapshot.dirs.keys().any(|path| path.ends_with("target")));
        assert!(snapshot.dirs[dir.path()]
            .iter()
            .any(|e| e.name == "visible"));
    }

    #[tokio::test]
    async fn transport_polling_watcher_delivers_bounded_change_then_closes() {
        let root = tempfile::tempdir().unwrap();
        let (tx, mut rx) = crate::event::event_channel(Default::default());
        let mut watcher = PollingWatcher::new(
            root.path(),
            Duration::from_millis(20),
            vec![],
            100,
            1_000,
            tx.clone(),
        );
        let handle = std::thread::spawn(move || watcher.run_blocking());
        std::fs::write(root.path().join("created"), b"fixture").unwrap();
        let event = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event, Event::FsChange(paths) if !paths.is_empty()));
        rx.close();
        tokio::task::spawn_blocking(move || handle.join().unwrap())
            .await
            .unwrap();
    }
}
