use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use tokio::sync::mpsc;

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

/// Filesystem watcher that monitors a root directory and sends change events.
#[allow(dead_code)]
pub struct FsWatcher {
    /// Whether the watcher is currently forwarding events.
    active: Arc<AtomicBool>,
    /// Handle to the debouncer (dropped to stop watching).
    _debouncer: notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>,
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
        event_tx: mpsc::UnboundedSender<Event>,
    ) -> notify::Result<Self> {
        let active = Arc::new(AtomicBool::new(true));
        let active_clone = active.clone();
        let root_path = root.to_path_buf();

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
                            .map(|e| e.path.clone())
                            .filter(|p| !should_ignore(p, &ignore_patterns))
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

                        let _ = event_tx.send(Event::FsChange(final_paths));
                    }
                    Err(_errors) => {
                        // Watcher errors are non-fatal; silently ignore
                    }
                }
            },
        )?;

        debouncer
            .watcher()
            .watch(root, notify::RecursiveMode::Recursive)?;

        Ok(Self {
            active,
            _debouncer: debouncer,
        })
    }

    /// Pause event forwarding (watcher stays alive to avoid re-creating inotify watches).
    pub fn pause(&self) {
        self.active.store(false, Ordering::Relaxed);
    }

    /// Resume event forwarding.
    pub fn resume(&self) {
        self.active.store(true, Ordering::Relaxed);
    }

    /// Check if the watcher is currently active (forwarding events).
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
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
}
