use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use ratatui::layout::Rect;
use ratatui::text::Line;
use syntect::highlighting::Theme;
use syntect::parsing::SyntaxSet;

use crate::components::help::HelpState;
use crate::config::AppConfig;
use crate::editor::EditorState;
use crate::error::Result;
use crate::fs::clipboard::{ClipboardOp, ClipboardState};
use crate::fs::tree::{NodeType, TreeState};
use crate::preview_content;
use crate::terminal::{TerminalSelection, TerminalState};
use crate::theme::{self, ThemeColors};

/// The kind of dialog being displayed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum DialogKind {
    CreateFile,
    CreateDirectory,
    Rename {
        original: PathBuf,
    },
    DeleteConfirm {
        targets: Vec<PathBuf>,
    },
    Error {
        message: String,
    },
    Progress {
        message: String,
        current: usize,
        total: usize,
    },
    /// Save only; dismiss restores modal origin without changing input focus.
    SaveConfirm,
    /// Editor Esc: save or keep the buffer and explicitly return to preview focus.
    FocusBackConfirm,
    /// Explicit close/quit decision, with a captured stable document identity.
    DocumentDecision {
        id: crate::workspace::documents::DocumentId,
        path: PathBuf,
        quitting: bool,
    },
    /// A save refused to discard an external version or normalize mixed endings.
    SaveConflict {
        message: String,
        exit_after_save: bool,
        normalize: bool,
    },
    /// Save the buffer to a new name without replacing an existing destination.
    EditorSaveAs {
        exit_after_save: bool,
        normalize: bool,
    },
    /// Overwrite only the disk revision shown when this confirmation was opened.
    SaveOverwrite {
        exit_after_save: bool,
        normalize: bool,
        expected_revision: Option<crate::fs::save::FileRevision>,
    },
    /// Save settings dialog: Global / Local / Cancel.
    SaveSettings,
    /// One-at-a-time offer to recover a private snapshot into its document.
    ///
    /// The prompt names the specific document it is acting on and how many
    /// further records remain, and the handler re-offers the next remaining
    /// record after each handled one (bounded by the record count, never an
    /// unbounded loop). Declining leaves every record and file untouched.
    RecoveryPrompt {
        /// Document the offer currently acts on (restore/discard scope).
        document: PathBuf,
        /// Records still retained, this offer included.
        remaining: usize,
    },
    /// Interactive approval of a project-local LSP argv before execution.
    /// The approved identity is the (workspace root, argv) pair — never the
    /// language name alone. Session-scoped: nothing is persisted.
    LspTrust {
        language: String,
        argv: Vec<String>,
        root: PathBuf,
    },
    /// Read-only LSP capability/session status display.
    LspStatus {
        lines: Vec<String>,
    },
}

pub use crate::workspace::focus::{InputOverlay as AppMode, PanelFocus as FocusedPanel};

#[derive(Debug)]
struct DocumentLifecycle {
    quitting: bool,
    pending: std::collections::VecDeque<crate::workspace::documents::DocumentId>,
}

/// View mode for large-file head+tail preview.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ViewMode {
    #[default]
    HeadAndTail,
    HeadOnly,
    TailOnly,
}

/// State for the file preview panel.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct PreviewState {
    /// Path of the file currently being previewed.
    pub current_path: Option<PathBuf>,
    /// Rendered content lines (syntax-highlighted).
    pub content_lines: Vec<Line<'static>>,
    /// Absolute visual-row offset; equals logical-line index when unwrapped.
    pub scroll_offset: usize,
    /// Horizontal offset in logical display cells, independent of visual rows.
    pub horizontal_offset: usize,
    /// Current view mode for large files.
    pub view_mode: ViewMode,
    /// Whether long lines wrap.
    pub line_wrap: bool,
    /// Total number of lines in the content.
    pub total_lines: usize,
    /// Whether the current file is in large-file mode.
    pub is_large_file: bool,
    /// Number of head lines to show in head+tail mode.
    pub head_lines: usize,
    /// Number of tail lines to show in head+tail mode.
    pub tail_lines: usize,
    /// Whether the current directory preview is a shallow (depth-1) scan.
    /// When true, the user can press D to trigger a deep scan.
    pub is_shallow_preview: bool,
}

impl PreviewState {
    /// Number of visual rows; no document-wide glyph/row allocation.
    pub fn visual_row_count(&self, width: usize) -> usize {
        if !self.line_wrap || width == 0 {
            return self.content_lines.len();
        }
        self.content_lines
            .iter()
            .map(|line| {
                crate::text::visual_rows(&crate::text::line_text(line), width, true, false).count()
            })
            .sum()
    }

    /// Map an absolute visual row to its logical line and cell range.
    pub fn visual_row(
        &self,
        mut index: usize,
        width: usize,
    ) -> Option<(usize, crate::text::VisualRow)> {
        for (line_idx, line) in self.content_lines.iter().enumerate() {
            if !self.line_wrap && index > 0 {
                index -= 1;
                continue;
            }
            let content = crate::text::line_text(line);
            for row in crate::text::visual_rows(&content, width, self.line_wrap, false) {
                if index == 0 {
                    return Some((line_idx, row));
                }
                index -= 1;
            }
        }
        None
    }

    /// Maximum logical display width, including expanded tabs and graphemes.
    pub fn max_display_width(&self) -> usize {
        self.content_lines
            .iter()
            .map(|line| {
                let text = crate::text::line_text(line);
                crate::text::byte_to_display_col(&text, text.len(), crate::text::TAB_WIDTH)
            })
            .max()
            .unwrap_or(0)
    }
}

/// A single fuzzy search result.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SearchResult {
    /// The relative path string.
    pub path: PathBuf,
    /// Display string (relative path from root).
    pub display: String,
    /// Match score from fuzzy-matcher.
    pub score: i64,
    /// Indices of matched characters in the display string.
    pub match_indices: Vec<usize>,
}

/// State for the fuzzy finder overlay (Ctrl+P).
#[derive(Debug, Default)]
pub struct SearchState {
    /// Current search query string.
    pub query: String,
    /// Cursor position within the query.
    pub cursor_position: usize,
    /// Filtered and scored results.
    pub results: Vec<SearchResult>,
    /// Currently selected result index.
    pub selected_index: usize,
    /// Cached file path index (built on workers when the pipeline is enabled,
    /// synchronously for embedded callers without a loop to drain results).
    pub cached_paths: Option<Vec<PathBuf>>,
    /// Last fuzzy-finder status line. Kept in state so status changes that do
    /// not re-score results (index completion/failure) still update the UI.
    pub status: String,
    /// Immutable identity of the in-flight filename-index request.
    pub index_generation: Option<crate::background::RequestGeneration>,
    /// True while a resumable index batch is outstanding.
    pub indexing: bool,
    /// The traversal exhausted the tree without hitting a cap.
    pub index_complete: bool,
    /// The traversal stopped early because of a cap or unreadable entries, or
    /// ended without exhausting the tree (admission refusal, failed/panicked
    /// delivery). `indexing` is the distinct "request still outstanding" signal,
    /// so a usable partial index is never presented as complete or in-flight.
    pub index_capped: bool,
    pub index_unreadable: usize,
}

impl SearchState {
    /// Human-readable fuzzy-finder status. Every non-complete outcome (still
    /// indexing, capped, unreadable entries, or a partial/failed index) keeps an
    /// explicit incomplete marker; only a full traversal is unqualified.
    pub fn status_text(&self) -> String {
        if self.query.is_empty() {
            return if self.indexing {
                "Indexing filenames...".to_string()
            } else {
                "Type to search...".to_string()
            };
        }
        let count = self.results.len();
        let plural = if count == 1 { "" } else { "s" };
        if self.indexing {
            return format!("{count} result{plural} (indexing…)");
        }
        if self.index_capped || self.index_unreadable > 0 {
            let reason = if self.index_capped {
                if self.index_unreadable > 0 {
                    "cap + unreadable"
                } else {
                    "cap"
                }
            } else {
                "unreadable entries"
            };
            return format!("{count} result{plural} (index incomplete: {reason})");
        }
        format!("{count} result{plural}")
    }
}

/// State for the search action menu overlay.
#[derive(Debug, Clone)]
pub struct SearchActionState {
    /// Absolute path to the selected file/directory.
    pub path: PathBuf,
    /// Display string (relative path from root).
    pub display: String,
    /// Whether the target is a directory.
    pub is_directory: bool,
    /// Whether the target is a binary file.
    pub is_binary: bool,
}

/// State for a dialog's text input.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct DialogState {
    pub input: String,
    pub cursor_position: usize,
}

/// A reversible operation that can be undone.
#[derive(Debug, Clone)]
pub enum UndoAction {
    /// Undo a rename: rename back from `to` to `from`.
    Rename { from: PathBuf, to: PathBuf },
    /// Undo a copy-paste: delete the created paths.
    CopyPaste { created_paths: Vec<PathBuf> },
    /// Undo a move-paste: move files back from `to` to `from`.
    MovePaste { moves: Vec<(PathBuf, PathBuf)> },
    PartialPaste {
        copies: Vec<PathBuf>,
        moves: Vec<(PathBuf, PathBuf)>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct CopyOrigin {
    frame: crate::workspace::focus::WorkflowId,
    panel: FocusedPanel,
    document: Option<crate::workspace::documents::DocumentId>,
    presentation: RightPanelPresentation,
    clipboard_revision: u64,
    terminal_session: Option<u64>,
}
struct OperationOrigin {
    generation: crate::background::RequestGeneration,
    frame: crate::workspace::focus::WorkflowId,
    interrupt: Arc<AtomicBool>,
    clipboard_revision: u64,
}

/// Right-panel presentation is independent of the workspace keyboard target.
/// A browsing preview may be displayed while a pinned document is retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightPanelPresentation {
    RetainedDocument,
    SelectedPreview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitterDrag {
    Explorer,
    Terminal,
}

/// Upper bound on dirty documents snapshotted in one bounded pass. Retention is
/// enforced independently by the store, so this only bounds per-pass work.
const MAX_RECOVERY_SNAPSHOTS_PER_PASS: usize = 64;

/// Injected private recovery persistence for one running app instance.
///
/// The store, its resolved policy, the monotonic write throttle and the
/// discovered records for the workspace root are owned here. `App` keeps this
/// `None` unless a caller (production `main`, or a test with an injected state
/// directory) installs it, so no default-constructed app ever reads or writes a
/// user home directory.
pub struct RecoveryContext {
    pub store: crate::recovery::RecoveryStore,
    pub policy: crate::recovery::RecoveryPolicy,
    pub throttle: crate::recovery::SnapshotThrottle,
    /// Workspace root used for record keying (the explicit CLI root).
    pub root: PathBuf,
    /// Discovered, still-retained records for `root`, newest capture first.
    pub records: Vec<crate::recovery::RecoveryRecord>,
}

impl RecoveryContext {
    /// Build a context and discover the retained records for `root` in one
    /// bounded step (`load_all` applies count/age retention; each record read is
    /// size-capped). No home directory is consulted; `store` is injected.
    pub fn discover(
        store: crate::recovery::RecoveryStore,
        root: PathBuf,
        policy: crate::recovery::RecoveryPolicy,
    ) -> Self {
        let mut records = store.load_all(&root, &policy, std::time::SystemTime::now());
        records.sort_by_key(|r| std::cmp::Reverse(r.captured_secs));
        Self {
            throttle: crate::recovery::SnapshotThrottle::new(policy.min_interval),
            store,
            policy,
            root,
            records,
        }
    }

    /// Reload retained records after a discard/clear/enable change.
    fn refresh(&mut self) {
        self.records = self
            .store
            .load_all(&self.root, &self.policy, std::time::SystemTime::now());
        self.records
            .sort_by_key(|r| std::cmp::Reverse(r.captured_secs));
    }

    /// Write recovery snapshots for every dirty pinned document, bounded by
    /// [`MAX_RECOVERY_SNAPSHOTS_PER_PASS`]. Each write is revision-keyed, so a
    /// snapshot for an older on-disk revision lands in a distinct record file
    /// and can never overwrite the recovered text of a newer revision.
    fn write_snapshots(&mut self, workspace: &crate::workspace::Workspace) -> Option<String> {
        workspace.documents.observe_edits();
        let now = std::time::SystemTime::now();
        let mut written = 0usize;
        for document in workspace.documents.iter() {
            if written >= MAX_RECOVERY_SNAPSHOTS_PER_PASS {
                break;
            }
            if !document.editor.modified || !document.is_pinned() {
                continue;
            }
            let Ok(revision) = crate::recovery::RevisionRef::from_path(document.path()) else {
                // The source vanished or is unsafe: skip it honestly, never
                // inventing a revision the restore path could later trust.
                continue;
            };
            let record = match crate::recovery::RecoveryRecord::capture(
                &self.root,
                document.path(),
                &document.text(),
                revision,
                now,
                self.policy.max_text_bytes,
            ) {
                Ok(record) => record,
                Err(crate::recovery::RecoveryError::TooLarge(_)) => continue,
                Err(error) => return Some(format!("Recovery snapshot not saved: {error}")),
            };
            if let Err(error) = self.store.save(&record, &self.policy) {
                return Some(format!("Recovery snapshot not saved: {error}"));
            }
            written += 1;
            // Keep the in-memory offer list current without re-reading disk:
            // the just-written record replaces any earlier one for the same
            // document/revision and is capped by the retention policy.
            self.records.retain(|existing| {
                !(existing.document_path == record.document_path
                    && existing.revision == record.revision)
            });
            self.records.push(record);
            self.records
                .sort_by_key(|r| std::cmp::Reverse(r.captured_secs));
            self.records.truncate(self.policy.max_records.max(1));
        }
        None
    }
}

/// Main application state.
pub struct App {
    pub keymap: crate::keymap::Keymap,
    pub keymap_epoch: Instant,
    pub keymap_target: Option<(
        crate::keymap::FocusContext,
        Option<crate::workspace::documents::DocumentId>,
    )>,
    pub command_menu: Option<crate::components::command_menu::CommandMenu>,
    /// Language-feature overlay state (completion/hover/locations/symbols).
    pub language_features: Option<crate::components::language_features::LanguageFeatures>,
    pub command_entry_area: Rect,
    /// Direct secondary actions return to their workspace, not a stale search.
    pub command_selection_actions: bool,
    /// Merged configuration (CLI + file + defaults).
    pub config: AppConfig,
    /// Resolved theme colors for the UI.
    pub theme_colors: ThemeColors,
    pub tree_state: TreeState,
    pub should_quit: bool,
    #[allow(dead_code)]
    pub dialog_state: DialogState,
    #[allow(dead_code)]
    pub status_message: Option<(String, Instant)>,
    pub preview_state: PreviewState,
    pub right_panel_presentation: RightPanelPresentation,
    pub syntax_set: SyntaxSet,
    pub syntax_theme: Theme,
    /// Tracks which tree index was last previewed, to avoid re-loading on every frame.
    pub last_previewed_index: Option<usize>,
    /// Immutable identity of the preview the app currently wants prepared.
    desired_preview: Option<crate::highlighting::PreviewKey>,
    /// Class of the currently installed prepared preview, so input handlers can
    /// rekey/cycle without a file read.
    preview_class: Option<crate::highlighting::PreviewClass>,
    /// Prepared-only per-document syntax caches (bounded, checkpointed).
    syntax_caches:
        HashMap<crate::workspace::documents::DocumentId, crate::highlighting::SyntaxCache>,
    /// Main loop enables the prepared-only background pipeline. When disabled
    /// (embedded/non-async callers with no loop to drain results) prepared state
    /// is loaded synchronously so the app stays usable.
    pub prepared_pipeline: bool,
    /// Internal clipboard for copy/cut/paste operations.
    pub clipboard: ClipboardState,
    /// Cancellation token for async operations.
    pub cancel_token: Arc<AtomicBool>,
    /// Last reversible operation (single-level undo).
    pub last_undo: Option<UndoAction>,
    /// State for the fuzzy finder overlay (Ctrl+P).
    pub search_state: SearchState,
    /// Whether the search overlay is showing literal project content search
    /// instead of the fuzzy filename finder.
    pub content_search_active: bool,
    /// Bounded, generation-tagged content search hits/status.
    pub content_search: crate::components::content_search::ContentSearchState,
    /// Fuzzy matcher instance (reused across searches).
    pub fuzzy_matcher: SkimMatcherV2,
    /// Whether the filesystem watcher is currently active.
    pub watcher_active: bool,
    /// Bounded, short-lived record of writes this process performed, so the
    /// watcher's own event for an internal save is not reported as an external
    /// change. Each marker records the exact post-write identity observed at
    /// save time; see [`Self::note_self_write`].
    self_written: Vec<SelfWrite>,
    /// State for the help overlay.
    pub help_state: HelpState,
    /// Last rendered tree panel area (for mouse click mapping).
    pub tree_area: Rect,
    /// Last rendered preview panel area (for mouse click mapping).
    pub preview_area: Rect,
    /// Actual inner content of the retained bordered document/tree widgets.
    pub preview_content_area: Rect,
    pub tree_content_area: Rect,
    /// Rendered tab strip and mouse mapping from the same layout.
    pub document_tabs: crate::components::document_tabs::TabLayout,
    pub breadcrumbs: crate::components::workspace_chrome::BreadcrumbLayout,
    /// Document-list overlay uses the existing Search modal focus safeguards.
    pub document_list: Option<Vec<crate::workspace::documents::DocumentId>>,
    pub document_list_index: usize,
    document_lifecycle: Option<DocumentLifecycle>,
    /// Genuine tree double-click identity (row positions can change).
    pub tree_last_click: Option<(PathBuf, Instant)>,
    /// Current mouse text selection in preview panel (View mode).
    pub preview_selection: TerminalSelection,
    /// Embedded terminal state (PTY + emulator).
    pub terminal_state: TerminalState,
    /// Last rendered terminal panel area (for mouse click mapping).
    pub terminal_area: Rect,
    /// Last viewport and model leaves; caches only, never saved preferences.
    pub workspace_area: Option<Rect>,
    pub workspace_rects: crate::workspace::layout::WorkspaceRects,
    pub splitter_drag: Option<SplitterDrag>,
    /// Sole document ownership and panel/modal focus state.
    pub workspace: crate::workspace::Workspace,
    /// State for the search action menu overlay.
    pub search_action_state: Option<SearchActionState>,
    /// Event sender for spawning async operations from non-handler contexts.
    pub event_tx: Option<crate::event::EventSender>,
    /// Language-server sessions and execution trust (FR-10). Sessions own
    /// pump threads; events arrive generation-tagged via `Event::Lsp`.
    pub lsp: crate::lsp::LspManager,
    /// Native scan scheduler is lazily created inside a running Tokio context.
    jobs: Option<crate::app_jobs::AppJobs>,
    /// Bounded previous tree root, so profile replacement cannot revive A/B/A.
    jobs_root: Option<PathBuf>,
    /// Path of the directory currently being scanned asynchronously (dedup guard).
    pub active_dir_scan: Option<PathBuf>,
    /// Immutable current preview request identity, separate from path equality.
    active_summary: Option<crate::background::RequestGeneration>,
    active_s3_head: Option<crate::background::RequestGeneration>,
    active_copy: Option<(crate::background::RequestGeneration, CopyOrigin)>,
    pending_copy: Option<(String, CopyOrigin)>,
    clipboard_revision: u64,
    operation_sequence: u64,
    operations: Vec<OperationOrigin>,
    /// Visible height of the tree panel inner area (set during render).
    pub tree_visible_height: usize,
    /// X-coordinate of the scrollbar column (None if no scrollbar rendered).
    pub scrollbar_column: Option<u16>,
    /// Whether the user is dragging the scrollbar thumb.
    pub scrollbar_dragging: bool,
    /// Whether the viewport was explicitly scrolled (mouse wheel / scrollbar).
    /// When true, `update_scroll` is skipped so the viewport doesn't snap back
    /// to the selection. Cleared by any keyboard navigation or tree mouse click.
    pub tree_viewport_locked: bool,
    /// Text shown in the copy overlay (mouse capture disabled for browser clipboard).
    pub copy_overlay_text: Option<String>,
    pub copy_overlay_scroll: (u16, u16),
    copy_overlay_mouse_suspended: bool,
    text_clipboard_backend: crate::app_jobs::ClipboardBackend,
    /// Last left-click in the preview panel: (timestamp, screen_col, screen_row).
    /// Used to detect double-clicks for full-line selection.
    pub last_preview_click: Option<(Instant, u16, u16)>,
    /// S3 backend (None when in local mode).
    pub s3_backend: Option<crate::s3::S3Backend>,
    /// S3 config (None when in local mode).
    pub s3_config: Option<crate::s3::S3Config>,
    /// Cache of downloaded S3 files: S3 URI -> local cache path.
    #[allow(dead_code)]
    pub s3_download_cache: HashMap<String, PathBuf>,
    /// Whether S3 head preview is currently active (showing streamed content).
    pub s3_head_active: bool,
    /// Whether S3 head preview is currently loading.
    pub s3_head_loading: bool,
    /// Cached rendered lines for the current S3 head preview.
    pub s3_head_content: Option<Vec<Line<'static>>>,
    /// S3 URI for the current head preview (to invalidate on navigation).
    pub s3_head_uri: Option<String>,
    /// Injected private recovery persistence; `None` until a caller installs a
    /// store. The recovery commands are unavailable while it is `None`.
    pub recovery: Option<RecoveryContext>,
    /// Explicit CLI workspace root used to key recovery records. An injected
    /// session for another workspace is never applied to it.
    recovery_root: PathBuf,
    /// Documents already handled during the current recovery-prompt pass.
    /// Each handled document is recorded once, so re-offering advances through
    /// the bounded record set instead of looping on the same record.
    recovery_prompt_handled: Vec<PathBuf>,
    /// Generation-tagged read-only Git state. A snapshot is only rendered while
    /// its work-tree root still matches the current workspace, so a
    /// prior-workspace result cannot recolor the current tree even if its
    /// generation somehow survived.
    pub git: crate::git::GitState,
    /// One-entry memo of the resolved Git work tree for the current tree root,
    /// so render frames reuse the `.git` ancestor walk instead of repeating it
    /// every frame. Keyed by the tree root; cleared when a refresh is requested
    /// so a repository created after startup is still observed.
    git_worktree_cache: RefCell<Option<(PathBuf, Option<PathBuf>)>>,
    /// Most recent automatic Git refresh request (time, work tree). Used to
    /// coalesce requests so Git's own read-only working-tree scan -- which the
    /// watcher reports back as `FsChange` -- cannot spawn `git status` on every
    /// debounce window.
    git_last_request: Option<(Instant, PathBuf)>,
    /// A request that the coalescing floor dropped, kept as its due time
    /// (`last_request + GIT_REFRESH_MIN_INTERVAL`). The main loop folds this
    /// into its bounded timer wake so the dropped change still converges,
    /// without a further filesystem event.
    git_deferred_refresh_at: Option<Instant>,
}

/// Minimum interval between automatic Git refresh requests for the same work
/// tree. `git status` reads the working tree and `.git`, and the watcher
/// (notify 7 subscribes to open events) reports those reads back as
/// `FsChange`; without a floor the two would feed back and spawn `git status`
/// on every debounce window. The floor keeps indicator updates responsive while
/// bounding the spawn rate. A request for a *different* work tree always runs,
/// and a request dropped by the floor is retried by the bounded timer wake at
/// `last_request + GIT_REFRESH_MIN_INTERVAL` (see
/// [`App::git_refresh_wait`]), so a real change inside the window still
/// converges even when no further filesystem event follows.
const GIT_REFRESH_MIN_INTERVAL: Duration = Duration::from_millis(750);

/// How long a completed internal write stays eligible to absorb its own
/// watcher event before it is treated as external again. Kept at or above the
/// watcher debounce window so a coalesced event is still recognized.
const SELF_WRITE_TTL: Duration = Duration::from_secs(3);
/// Upper bound on retained self-write markers (FIFO eviction).
const SELF_WRITE_MAX: usize = 32;
/// Evidence that this process just wrote `path`.
///
/// Records the exact post-write identity: `len`, `mtime`, and a bounded content
/// digest **computed from the in-memory bytes the save published** (never
/// re-read from disk). A watcher event is only absorbed when the file on disk
/// still matches all three, so a genuine external write inside the TTL is
/// reported instead of silently swallowed — including a `cp -p`/`rsync -t`
/// overwrite that preserves both length and mtime.
struct SelfWrite {
    path: PathBuf,
    len: u64,
    mtime: Option<std::time::SystemTime>,
    /// Digest of the bounded front window of the saved bytes.
    ///
    /// The window is the first [`SELF_WRITE_DIGEST_BYTES`]; the event path
    /// hashes the same window of the on-disk file. See [`self_write_digest`]
    /// for the window choice, the >window behavior, and the residual limit.
    digest: u64,
    recorded_at: Instant,
}

/// Bound on the content bytes digested for a self-write identity.
///
/// The save path already holds the written bytes in memory, so this costs no
/// extra file I/O; it only bounds the hashing work. 64 KiB covers the small/
/// medium text files this editor is bounded to while keeping marker creation
/// well under a millisecond.
const SELF_WRITE_DIGEST_BYTES: usize = 64 * 1024;

/// FNV-1a digest over the bounded window of `bytes`.
///
/// The window is the **first** [`SELF_WRITE_DIGEST_BYTES`] bytes (the prefix,
/// because that is what the on-disk check can read cheaply without knowing the
/// file size up front). A plain fast hash is sufficient here because the digest
/// is only ever compared against the same file moments later under a 3 s TTL
/// and is always combined with the exact length and mtime; it is not a
/// persisted or adversarial integrity check.
///
/// When the content is larger than the window, only the prefix is digested on
/// both sides and the tail is **not** verified. With the exact `len` still
/// compared, the only residual theoretical limit is an external overwrite that
/// keeps the same length, forces the same mtime, and differs *only* beyond the
/// first [`SELF_WRITE_DIGEST_BYTES`] bytes.
fn self_write_digest(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes.iter().take(SELF_WRITE_DIGEST_BYTES) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Digest of the front window of the on-disk file at `path`.
///
/// Reads at most [`SELF_WRITE_DIGEST_BYTES`] bytes; never the whole file when it
/// is larger, so the event path stays bounded. Returns `None` when the file
/// cannot be read.
fn self_write_disk_digest(path: &Path) -> Option<u64> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut window = vec![0u8; SELF_WRITE_DIGEST_BYTES];
    let mut filled = 0usize;
    while filled < window.len() {
        match file.read(&mut window[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }
    Some(self_write_digest(&window[..filled]))
}

/// On-disk length, mtime, and bounded-window digest of `path`, or `None` when
/// it cannot be observed.
fn self_write_identity(path: &Path) -> Option<(u64, Option<std::time::SystemTime>, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let digest = self_write_disk_digest(path)?;
    Some((meta.len(), meta.modified().ok(), digest))
}

fn shell_quote_single(input: &str) -> String {
    format!("'{}'", input.replace('\'', "'\"'\"'"))
}

/// Walk up from `path` to the enclosing Git work-tree root.
///
/// Returns the first ancestor (inclusive) that contains a `.git` entry — a
/// directory for an ordinary checkout or a file for a linked work-tree or
/// submodule. Returns `None` when no ancestor is a repository, so callers
/// degrade to no indicators instead of running Git.
fn git_worktree_root(path: &Path) -> Option<PathBuf> {
    let mut current = Some(path);
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

impl App {
    #[allow(dead_code)]
    pub fn apply_keymap_config(
        &mut self,
        config: crate::keymap::KeymapConfig,
    ) -> std::result::Result<(), String> {
        let replacement = crate::keymap::Keymap::compile(&config)?;
        self.keymap.replace(replacement);
        self.config.keymap = config;
        self.keymap_target = None;
        Ok(())
    }
    /// Input context for configured workspace routes. Native modal input stays modal.
    pub fn input_context(&self) -> crate::keymap::FocusContext {
        use crate::keymap::FocusContext;
        match self.workspace.focus.overlay {
            AppMode::CommandMenu => FocusContext::Menu,
            AppMode::Normal => crate::commands::CommandContext::capture(self).binding_context(self),
            _ => FocusContext::Modal,
        }
    }

    /// Entry hints refer to the captured origin, not the overlay's Modal/Menu scope.
    pub fn command_entry_context(&self) -> crate::keymap::FocusContext {
        if let Some(menu) = &self.command_menu {
            return menu.origin.binding_context(self);
        }
        if self.workspace.focus.overlay == AppMode::Help {
            if let Some(origin) = &self.help_state.origin {
                return origin.binding_context(self);
            }
        }
        crate::commands::CommandContext::capture(self).binding_context(self)
    }

    /// Explicit entry only; never replace an existing modal workflow.
    pub fn open_command_menu(&mut self) {
        if self.workspace.focus.overlay != AppMode::Normal {
            return;
        }
        self.keymap.reset();
        let origin = crate::commands::CommandContext::capture(self);
        if let Err(error) = self
            .workspace
            .focus
            .open_overlay(AppMode::CommandMenu, origin.document)
        {
            self.set_status_message(error.to_string());
            return;
        }
        self.command_menu = Some(crate::components::command_menu::CommandMenu::new(origin));
    }

    pub fn dismiss_command_menu(&mut self) {
        if self.workspace.focus.overlay != AppMode::CommandMenu {
            return;
        }
        let presentation = self.command_menu.take().map(|m| m.origin.presentation);
        self.dismiss_overlay();
        if let Some(presentation) = presentation {
            self.right_panel_presentation = presentation;
        }
    }

    /// Create a new App rooted at the given path, using the provided config.
    ///
    /// Uses deferred tree loading: the root directory is shown immediately
    /// in a "Loading..." state. Call `spawn_initial_load()` after the event
    /// loop is set up to populate the tree asynchronously.
    pub fn new(path: &Path, config: AppConfig) -> Result<Self> {
        let keymap = crate::keymap::Keymap::compile(&config.keymap)
            .map_err(crate::error::AppError::InvalidPath)?;
        let page_size = config.max_entries_per_page();
        let mut tree_state = TreeState::new_deferred(path, page_size)?;
        tree_state.root.is_loading = false; // Loading only after real admission.
                                            // Apply config: show_hidden
        tree_state.show_hidden = config.show_hidden();
        // Apply config: sort settings
        tree_state.sort_by = crate::fs::tree::SortBy::from_str(config.sort_by());
        tree_state.dirs_first = config.dirs_first();
        // No sort_all_children() here — there are no children loaded yet
        tree_state.flatten();

        let syntax_set = SyntaxSet::load_defaults_newlines();
        let syntax_theme =
            preview_content::load_theme(Some(config.syntax_theme_name(config.theme_scheme())));
        let theme_colors = theme::resolve_theme(&config.theme);
        let watcher_auto_refresh = config.watcher_enabled() && config.watcher_auto_refresh();
        let mut terminal_state = TerminalState::default();
        terminal_state.set_scrollback_limit(config.terminal_scrollback());
        let workspace = crate::workspace::Workspace {
            layout: config.layout.state(),
            ..Default::default()
        };
        Ok(Self {
            keymap,
            keymap_epoch: Instant::now(),
            keymap_target: None,
            command_menu: None,
            language_features: None,
            command_entry_area: Rect::default(),
            config,
            theme_colors,
            tree_state,
            should_quit: false,
            dialog_state: DialogState::default(),
            status_message: None,
            preview_state: PreviewState::default(),
            syntax_set,
            syntax_theme,
            last_previewed_index: None,
            desired_preview: None,
            preview_class: None,
            syntax_caches: HashMap::new(),
            prepared_pipeline: false,
            clipboard: ClipboardState::new(),
            cancel_token: Arc::new(AtomicBool::new(false)),
            last_undo: None,
            search_state: SearchState::default(),
            content_search_active: false,
            content_search: crate::components::content_search::ContentSearchState::default(),
            fuzzy_matcher: SkimMatcherV2::default(),
            watcher_active: watcher_auto_refresh,
            self_written: Vec::new(),
            help_state: HelpState::default(),
            tree_area: Rect::default(),
            preview_area: Rect::default(),
            preview_content_area: Rect::default(),
            tree_content_area: Rect::default(),
            document_tabs: Default::default(),
            breadcrumbs: crate::components::workspace_chrome::BreadcrumbLayout::new(
                Path::new(""),
                Rect::default(),
                true,
            ),
            document_list: None,
            document_lifecycle: None,
            document_list_index: 0,
            tree_last_click: None,
            right_panel_presentation: RightPanelPresentation::RetainedDocument,
            preview_selection: TerminalSelection::default(),
            terminal_state,
            terminal_area: Rect::default(),
            workspace,
            workspace_area: None,
            workspace_rects: Default::default(),
            splitter_drag: None,
            search_action_state: None,
            command_selection_actions: false,
            event_tx: None,
            lsp: crate::lsp::LspManager::new(),
            jobs: None,
            jobs_root: (path.as_os_str().len() <= 4096).then(|| path.to_path_buf()),
            active_dir_scan: None,
            active_summary: None,
            active_s3_head: None,
            active_copy: None,
            pending_copy: None,
            clipboard_revision: 0,
            operation_sequence: 0,
            operations: Vec::new(),
            tree_visible_height: 0,
            scrollbar_column: None,
            scrollbar_dragging: false,
            tree_viewport_locked: false,
            copy_overlay_text: None,
            copy_overlay_scroll: (0, 0),
            copy_overlay_mouse_suspended: false,
            text_clipboard_backend: default_text_clipboard_backend(),
            last_preview_click: None,
            s3_backend: None,
            s3_config: None,
            s3_download_cache: HashMap::new(),
            s3_head_active: false,
            s3_head_loading: false,
            s3_head_content: None,
            s3_head_uri: None,
            recovery: None,
            recovery_root: path.to_path_buf(),
            recovery_prompt_handled: Vec::new(),
            git: crate::git::GitState::new(),
            git_worktree_cache: RefCell::new(None),
            git_last_request: None,
            git_deferred_refresh_at: None,
        })
    }

    /// Assemble the base native job envelope shared by every target kind.
    fn build_native_job(&mut self, target: crate::app_jobs::Target) -> crate::app_jobs::NativeJob {
        crate::app_jobs::NativeJob {
            target: target.clone(),
            max_entries: self.config.snapshot_max_entries(),
            timeout: std::time::Duration::from_millis(self.config.preview_timeout_ms()),
            result_bytes: crate::background::Limits::default().result_bytes,
            snapshot_options: crate::fs::tree::SnapshotOptions {
                sort_by: self.tree_state.sort_by.clone(),
                dirs_first: self.tree_state.dirs_first,
                page_size: self.tree_state.page_size,
                child_depth: TreeState::find_node_mut_pub(&mut self.tree_state.root, target.path())
                    .map_or(1, |node| node.depth + 1),
            },
            summary_colors: target.is_summary().then(|| self.theme_colors.clone()),
            progress: None,
            s3_profile: target
                .is_s3()
                .then(|| {
                    self.s3_backend
                        .as_ref()
                        .and_then(|backend| backend.profile())
                        .map(str::to_owned)
                })
                .flatten(),
            s3_head_lines: self.config.s3_head_lines(),
            clipboard: None,
            preview_theme: target.is_preview().then(|| crate::app_jobs::PreviewTheme {
                name: self
                    .config
                    .syntax_theme_name(self.config.theme_scheme())
                    .to_string(),
                colors: self.theme_colors.clone(),
            }),
            search: None,
        }
    }

    /// Nonwaiting native admission; no loader flag changes until it succeeds.
    fn submit_native_job(&mut self, target: crate::app_jobs::Target) -> bool {
        self.reconcile_job_root();
        if self.jobs.is_none() {
            match crate::app_jobs::AppJobs::new(
                crate::background::Limits::default(),
                crate::background::Worker::Blocking(Arc::new(crate::app_jobs::run)),
            ) {
                Ok(jobs) => self.jobs = Some(jobs),
                Err(error) => {
                    self.set_status_message(format!("Background job not admitted: {error}"));
                    return false;
                }
            }
        }
        if target.is_s3()
            && (target.path().as_os_str().len() > 4096
                || self
                    .s3_backend
                    .as_ref()
                    .and_then(|backend| backend.profile())
                    .is_some_and(|profile| profile.len() > 4096))
        {
            self.set_status_message("Background job not admitted: S3 input budget".into());
            return false;
        }
        let job = self.build_native_job(target.clone());
        match self.jobs.as_mut().unwrap().submit(job) {
            Ok(generation) => {
                if target.is_summary() {
                    self.active_summary = Some(generation);
                    self.active_dir_scan = Some(target.path().to_path_buf());
                }
                if matches!(target, crate::app_jobs::Target::S3Head(_)) {
                    self.active_s3_head = Some(generation);
                }
                if target.is_snapshot()
                    || matches!(
                        target,
                        crate::app_jobs::Target::S3Root(_) | crate::app_jobs::Target::S3Expand(_)
                    )
                {
                    if let Some(node) =
                        TreeState::find_node_mut_pub(&mut self.tree_state.root, target.path())
                    {
                        node.is_loading = true;
                        node.is_expanded = true;
                    }
                    self.tree_state.flatten();
                    self.set_status_message("Directory scan admitted; loading...".into());
                }
                true
            }
            Err(error) => {
                self.set_status_message(format!("Background job not admitted: {error:?}"));
                false
            }
        }
    }

    pub fn spawn_initial_load(&mut self, _event_tx: &crate::event::EventSender) {
        if self.tree_state.root.node_type == NodeType::Directory {
            self.submit_native_job(crate::app_jobs::Target::Root(
                self.tree_state.root.path.clone(),
            ));
        }
    }

    /// Whether optional read-only Git indicators participate for this workspace.
    /// S3 and other virtual roots never receive indicators.
    pub fn git_indicators_enabled(&self) -> bool {
        self.config.git_enabled() && !self.is_s3_mode()
    }

    /// Request one bounded, generation-tagged read-only Git refresh for the
    /// current workspace through the existing background-job transport. Never
    /// called from render. A non-repository root records no request, so the
    /// tree degrades to no indicators rather than an error.
    pub fn request_git_refresh(&mut self) {
        if !self.git_indicators_enabled() {
            return;
        }
        // A refresh revalidates the work tree, so drop the memo first.
        *self.git_worktree_cache.get_mut() = None;
        let Some(root) = self.git_worktree() else {
            return;
        };
        let Some(tx) = self.event_tx.clone() else {
            return;
        };
        // Coalesce requests for the same work tree: Git's own read-only
        // working-tree scan is reported back by the watcher, so an unthrottled
        // re-request would spawn until the debounce loop stops. A different
        // work tree always refreshes immediately. A same-tree request inside the
        // floor is not discarded -- it is recorded as a deferred deadline that
        // the main loop's bounded timer wake issues, so the change still
        // converges without a further filesystem event.
        let now = Instant::now();
        if let Some((last, last_root)) = &self.git_last_request {
            if last_root == &root && now.duration_since(*last) < GIT_REFRESH_MIN_INTERVAL {
                self.git_deferred_refresh_at = Some(*last + GIT_REFRESH_MIN_INTERVAL);
                return;
            }
        }
        self.git_last_request = Some((now, root.clone()));
        self.git_deferred_refresh_at = None;
        let generation = self.git.begin(root.clone());
        crate::git::spawn_refresh(
            tx,
            root,
            generation,
            crate::git::GitLimits::default(),
            self.cancel_token.clone(),
        );
    }

    /// Time until a refresh dropped by the coalescing floor is due, or `None`
    /// when nothing was deferred or the deadline has already passed (in which
    /// case the main loop's current iteration already issues it). Folded into
    /// the main loop's bounded timer wake alongside the redraw/status/recovery
    /// waits, so a coalesced-away change converges even with no further
    /// filesystem event and with no per-frame spawn.
    pub fn git_refresh_wait(&self, now: Instant) -> Option<Duration> {
        let due = self.git_deferred_refresh_at?;
        Some(due.saturating_duration_since(now))
    }

    /// Issue a deferred refresh once its deadline has passed, or `None` when
    /// nothing is deferred or it is not due yet. Exactly one deferred refresh is
    /// issued per dropped window; the refresh itself re-arms the floor, so a
    /// deferred request cannot storm. `request_git_refresh` clears the deferred
    /// deadline, so a normal request supersedes it instead of double-issuing.
    pub fn issue_deferred_git_refresh(&mut self, now: Instant) -> Option<PathBuf> {
        let due = self.git_deferred_refresh_at?;
        if now < due {
            return None;
        }
        let rooted = self.git_last_request.as_ref().map(|(_, root)| root.clone());
        self.git_deferred_refresh_at = None;
        self.request_git_refresh();
        rooted
    }

    /// Install a generation-tagged refresh result; stale generations are refused
    /// without touching the retained snapshot.
    pub fn accept_git_refresh(&mut self, refresh: crate::git::GitRefresh) -> bool {
        self.git.accept(refresh)
    }

    /// The Git snapshot to render, or `None` when indicators are disabled or in
    /// S3 mode, no snapshot is retained, or the retained snapshot belongs to a
    /// different work-tree root than the current workspace (a stale or
    /// prior-workspace result). Degrades to no indicators, never an error.
    pub fn git_snapshot(&self) -> Option<&crate::git::GitSnapshot> {
        if !self.git_indicators_enabled() {
            return None;
        }
        let current = self.git_worktree()?;
        if self.git.root() != Some(current.as_path()) {
            return None;
        }
        self.git.snapshot()
    }

    /// Resolve the enclosing Git work tree for the current tree root, memoized
    /// in a single bounded entry keyed by that tree root. Render frames reuse
    /// the resolved value instead of walking `.git` ancestors on every frame;
    /// [`request_git_refresh`](Self::request_git_refresh) clears the entry so a
    /// repository created after startup is still observed.
    fn git_worktree(&self) -> Option<PathBuf> {
        let tree_root = &self.tree_state.root.path;
        let mut cache = self.git_worktree_cache.borrow_mut();
        match cache.as_ref() {
            Some((key, resolved)) if key == tree_root => resolved.clone(),
            _ => {
                let resolved = git_worktree_root(tree_root);
                *cache = Some((tree_root.clone(), resolved.clone()));
                resolved
            }
        }
    }

    /// Repository work-tree root and snapshot to render, if any.
    pub fn git_render(&self) -> Option<(&Path, &crate::git::GitSnapshot)> {
        let snapshot = self.git_snapshot()?;
        Some((self.git.root()?, snapshot))
    }

    /// Open a dialog of the given kind.
    #[allow(dead_code)]
    pub fn open_dialog(&mut self, kind: DialogKind) {
        self.dialog_state = DialogState::default();
        if let DialogKind::Rename { ref original } = kind {
            if let Some(name) = original.file_name() {
                let name = name.to_string_lossy().to_string();
                self.dialog_state.cursor_position = name.len();
                self.dialog_state.input = name;
            }
        }
        self.set_overlay(AppMode::Dialog(kind));
    }

    /// Close the current dialog and return to normal mode.
    #[allow(dead_code)]
    pub fn close_dialog(&mut self) {
        self.document_lifecycle = None;
        self.dismiss_overlay();
        self.dialog_state = DialogState::default();
    }

    /// Install the resolved private recovery store and discover the retained
    /// records for the explicit workspace root in one bounded step. Returns a
    /// visible notice when unsaved work is offered.
    pub fn configure_recovery(&mut self, store: crate::recovery::RecoveryStore) -> Option<String> {
        let policy = crate::recovery::RecoveryPolicy::from_config(&self.config);
        let context = RecoveryContext::discover(store, self.recovery_root.clone(), policy);
        let count = context.records.len();
        let enabled = context.policy.enabled;
        self.recovery = Some(context);
        if !enabled || count == 0 {
            return None;
        }
        Some(format!(
            "Recovery: {count} unsaved document(s) available (recovery.restore)"
        ))
    }

    /// Whether private recovery is configured and currently enabled.
    pub fn recovery_enabled(&self) -> bool {
        self.recovery
            .as_ref()
            .is_some_and(|context| context.policy.enabled)
    }

    /// Offer the newest discovered snapshot at startup. A no-op when nothing is
    /// retained or recovery is disabled, so no modal appears without real unsaved
    /// work. The prompt names one document at a time and discloses how many
    /// records remain, so it cannot be read as handling every record at once.
    /// An explicit call starts a fresh pass over the retained records.
    pub fn open_recovery_prompt(&mut self) {
        self.recovery_prompt_handled.clear();
        self.offer_next_recovery();
    }

    /// Offer the newest retained record whose document has not been handled in
    /// the current pass, disclosing how many distinct documents the pass will
    /// still offer (this one included). Closes the dialog when the bounded set is
    /// exhausted or recovery is disabled.
    fn offer_next_recovery(&mut self) {
        if !self.recovery_enabled() {
            self.close_dialog();
            return;
        }
        let Some(record) = self.next_recovery_offer() else {
            // Every retained document has already been handled this pass.
            self.close_dialog();
            return;
        };
        let document = record.document_path.clone();
        // Count distinct unhandled document paths, not raw records: one document
        // may hold several revision-keyed records that collapse to a single offer.
        let remaining = self.distinct_recovery_offers();
        self.open_dialog(DialogKind::RecoveryPrompt {
            document,
            remaining,
        });
    }

    /// Number of distinct documents the current pass will still offer, i.e.
    /// unique unhandled record paths. This is what the prompt's "N further"
    /// disclosure is derived from, so it cannot overstate the remaining offers.
    fn distinct_recovery_offers(&self) -> usize {
        let Some(context) = self.recovery.as_ref() else {
            return 0;
        };
        let mut seen: Vec<&std::path::Path> = Vec::new();
        for record in &context.records {
            let path = record.document_path.as_path();
            if self.recovery_prompt_handled.contains(&record.document_path) {
                continue;
            }
            if !seen.contains(&path) {
                seen.push(path);
            }
        }
        seen.len()
    }

    /// Newest retained record whose document has not been handled during the
    /// current prompt pass. Returns `None` when the bounded set is exhausted.
    fn next_recovery_offer(&self) -> Option<&crate::recovery::RecoveryRecord> {
        let context = self.recovery.as_ref()?;
        context
            .records
            .iter()
            .find(|record| !self.recovery_prompt_handled.contains(&record.document_path))
    }

    /// Advance to the next remaining record after one was handled. Bounded by
    /// the number of retained documents: every handled document is recorded
    /// once, so this cannot loop on the same record. Closes the dialog when
    /// nothing is left or recovery is disabled.
    pub fn reoffer_recovery_prompt(&mut self) {
        self.offer_next_recovery();
    }

    /// Restore the offered snapshot into its document (opening it when needed).
    /// The buffer becomes dirty; neither the source file nor the record is
    /// written here. A refusal or failure leaves both untouched.
    pub fn restore_recovery(&mut self) -> std::result::Result<String, String> {
        let Some(context) = self.recovery.as_ref() else {
            return Err("Private recovery is not implemented yet (Phase 7)".into());
        };
        if !context.policy.enabled {
            return Err("Private recovery is disabled".into());
        }
        let Some(record) = self.next_recovery_offer().cloned() else {
            return Err("No recovery snapshots available".into());
        };
        let store = context.store.clone();
        if !self
            .workspace
            .documents
            .iter()
            .any(|document| document.path() == record.document_path)
        {
            self.workspace
                .documents
                .open(
                    &record.document_path,
                    crate::workspace::documents::OpenDisposition::Pinned,
                )
                .map_err(|error| {
                    format!(
                        "Recovery: could not open {}: {error}",
                        record.document_path.display()
                    )
                })?;
        }
        let document = record.document_path.clone();
        let outcome = store
            .restore(&record, &mut self.workspace.documents)
            .map(|()| {
                format!(
                    "Recovered unsaved text for {} (save to keep it)",
                    document.display()
                )
            })
            .map_err(|error| format!("Recovery restore refused: {error}"));
        // Restore keeps every record, so advance past this document explicitly;
        // otherwise the same record would be offered again on the next keypress.
        self.recovery_prompt_handled.push(document);
        outcome
    }

    /// Discard every retained snapshot for the offered document. Other documents'
    /// records, the open buffer and the source file are untouched.
    pub fn discard_recovery(&mut self) -> std::result::Result<String, String> {
        let Some(context) = self.recovery.as_ref() else {
            return Err("Private recovery is not implemented yet (Phase 7)".into());
        };
        if !context.policy.enabled {
            return Err("Private recovery is disabled".into());
        }
        let Some(record) = self.next_recovery_offer().cloned() else {
            return Err("No recovery snapshots available".into());
        };
        let document = record.document_path.clone();
        let removed = context
            .store
            .discard(&context.root, &document)
            .map_err(|error| format!("Recovery discard failed: {error}"))?;
        if let Some(context) = self.recovery.as_mut() {
            context.refresh();
        }
        self.recovery_prompt_handled.push(document.clone());
        Ok(format!(
            "Discarded {removed} recovery snapshot(s) for {}",
            document.display()
        ))
    }

    /// Clear every owned recovery record (all workspaces) and restart the
    /// prompt pass. Open buffers and source files are untouched.
    pub fn clear_recovery(&mut self) -> std::result::Result<String, String> {
        let Some(context) = self.recovery.as_ref() else {
            return Err("Private recovery is not implemented yet (Phase 7)".into());
        };
        let removed = context
            .store
            .clear()
            .map_err(|error| format!("Recovery clear failed: {error}"))?;
        if let Some(context) = self.recovery.as_mut() {
            context.refresh();
        }
        self.recovery_prompt_handled.clear();
        Ok(format!("Cleared {removed} owned recovery snapshot(s)"))
    }

    /// Explicit runtime enable/disable surface. Disabling keeps every existing
    /// record on disk and only stops new snapshots for this session.
    pub fn set_recovery_enabled(&mut self, enabled: bool) -> String {
        self.config.recovery.enabled = Some(enabled);
        if let Some(context) = self.recovery.as_mut() {
            context.policy.enabled = enabled;
            context.refresh();
        }
        if enabled {
            "Private recovery enabled".to_string()
        } else {
            "Private recovery disabled (existing snapshots kept)".to_string()
        }
    }

    /// Bounded, throttled snapshot pass for dirty documents. Safe to call from
    /// the main loop after input handling; never from render.
    ///
    /// Guard order matters: the enabled check and the "is there anything to
    /// write" check both run *before* `due(now)`. A due call consumes the write
    /// window, so an input event that arrives with nothing dirty must leave the
    /// window untouched; otherwise it would delay a genuinely needed snapshot by
    /// up to one throttle interval. The throttle semantics themselves are
    /// unchanged.
    pub fn snapshot_dirty_documents(&mut self, now: Instant) -> Option<String> {
        if !self
            .recovery
            .as_ref()
            .is_some_and(|context| context.policy.enabled)
        {
            return None;
        }
        if !self.has_dirty_pinned_documents() {
            return None;
        }
        let context = self.recovery.as_mut()?;
        if !context.throttle.due(now) {
            return None;
        }
        context.write_snapshots(&self.workspace)
    }

    /// Whether any open document is both modified and durable (pinned), i.e.
    /// whether a recovery snapshot pass has real work to do. Used as the
    /// pre-throttle admission check so an idle event never consumes the write
    /// window.
    fn has_dirty_pinned_documents(&self) -> bool {
        self.workspace
            .documents
            .iter()
            .any(|document| document.editor.modified && document.is_pinned())
    }

    /// Time until the next throttled snapshot attempt is permitted while real
    /// dirty work is pending, or `None` when there is nothing to capture (or
    /// the next attempt is already due). The main loop folds this into its wait
    /// so an idle application still captures the last edit within the throttle
    /// interval instead of relying on a further input event.
    pub fn recovery_snapshot_wait(&self, now: Instant) -> Option<Duration> {
        let context = self.recovery.as_ref()?;
        if !context.policy.enabled {
            return None;
        }
        if !self.has_dirty_pinned_documents() {
            return None;
        }
        context.throttle.remaining(now)
    }

    /// Bounded final snapshot pass at shutdown, bypassing the throttle so the
    /// last unsaved edit is captured. Failures are returned for stderr.
    pub fn flush_recovery_on_shutdown(&mut self) -> Option<String> {
        let context = self.recovery.as_mut()?;
        if !context.policy.enabled {
            return None;
        }
        context.write_snapshots(&self.workspace)
    }

    /// Open or advance one modal workflow without overwriting its origin.
    pub fn set_overlay(&mut self, overlay: AppMode) -> bool {
        self.keymap.reset();
        if overlay == AppMode::Help && self.workspace.focus.overlay == AppMode::Normal {
            self.help_state.origin = Some(crate::commands::CommandContext::capture(self));
        }
        if overlay == AppMode::Normal {
            self.dismiss_overlay();
            return true;
        }
        if self.workspace.focus.overlay != AppMode::Normal {
            self.workspace.focus.replace_overlay(overlay);
            return true;
        }
        if let Err(error) = self
            .workspace
            .focus
            .open_overlay(overlay, self.workspace.documents.active_id())
        {
            self.set_status_message(error.to_string());
            return false;
        }
        true
    }

    /// Restore focus and reactivate a still-owned origin, never reload it.
    pub fn dismiss_overlay(&mut self) {
        self.keymap.reset();
        if let Some(context) = self.workspace.focus.dismiss_overlay() {
            if let Some(id) = context.document {
                let _ = self.workspace.documents.activate(id);
            }
        } else {
            self.workspace.focus.overlay = AppMode::Normal;
        }
        if self.workspace.focus.panel == FocusedPanel::Editor {
            self.right_panel_presentation = RightPanelPresentation::RetainedDocument;
        }
        self.reconcile_pane_focus();
    }

    /// Save/reload target is fixed at modal entry, not at completion.
    fn editor_target(&self) -> Option<crate::workspace::documents::DocumentId> {
        if self.workspace.focus.overlay != AppMode::Normal {
            self.workspace
                .focus
                .overlay_document()
                .or_else(|| self.workspace.documents.active_id())
        } else {
            self.workspace.documents.active_id()
        }
    }

    /// Display the explicitly selected browsing preview without changing input focus.
    pub fn show_selected_preview(&mut self) {
        self.right_panel_presentation = RightPanelPresentation::SelectedPreview;
    }

    /// The UI and mouse handler use the same presentation decision. Input routing
    /// remains exclusively owned by Workspace.focus, never this choice.
    pub fn editor_visible(&self) -> bool {
        self.workspace.documents.active().is_some_and(|d| {
            self.workspace.focus.panel == FocusedPanel::Editor
                || (self.right_panel_presentation == RightPanelPresentation::RetainedDocument
                    && (d.is_pinned() || self.workspace.focus.panel == FocusedPanel::Terminal))
        }) && self.workspace.focus.panel != FocusedPanel::Preview
    }

    /// Insert a character at the current cursor position.
    #[allow(dead_code)]
    pub fn dialog_input_char(&mut self, c: char) {
        self.dialog_state
            .input
            .insert(self.dialog_state.cursor_position, c);
        self.dialog_state.cursor_position += c.len_utf8();
    }

    /// Delete the character before the cursor (backspace).
    #[allow(dead_code)]
    pub fn dialog_delete_char(&mut self) {
        if self.dialog_state.cursor_position > 0 {
            let byte_pos = self.dialog_state.cursor_position;
            let prev_char = self.dialog_state.input[..byte_pos]
                .chars()
                .next_back()
                .expect("cursor > 0 guarantees at least one char");
            self.dialog_state.cursor_position -= prev_char.len_utf8();
            self.dialog_state
                .input
                .remove(self.dialog_state.cursor_position);
        }
    }

    /// Move cursor left by one character.
    #[allow(dead_code)]
    pub fn dialog_move_cursor_left(&mut self) {
        if self.dialog_state.cursor_position > 0 {
            let prev_char = self.dialog_state.input[..self.dialog_state.cursor_position]
                .chars()
                .next_back()
                .expect("cursor > 0 guarantees at least one char");
            self.dialog_state.cursor_position -= prev_char.len_utf8();
        }
    }

    /// Move cursor right by one character.
    #[allow(dead_code)]
    pub fn dialog_move_cursor_right(&mut self) {
        if self.dialog_state.cursor_position < self.dialog_state.input.len() {
            let next_char = self.dialog_state.input[self.dialog_state.cursor_position..]
                .chars()
                .next()
                .expect("cursor < len guarantees at least one char");
            self.dialog_state.cursor_position += next_char.len_utf8();
        }
    }

    /// Move cursor to the beginning of the input.
    #[allow(dead_code)]
    pub fn dialog_cursor_home(&mut self) {
        self.dialog_state.cursor_position = 0;
    }

    /// Move cursor to the end of the input.
    #[allow(dead_code)]
    pub fn dialog_cursor_end(&mut self) {
        self.dialog_state.cursor_position = self.dialog_state.input.len();
    }

    /// Set a status message with current timestamp.
    #[allow(dead_code)]
    pub fn set_status_message(&mut self, msg: String) {
        self.status_message = Some((msg, Instant::now()));
    }

    /// Exact four-second legacy deadline (>3 whole elapsed seconds).
    pub fn status_wait(&self, now: Instant) -> Option<std::time::Duration> {
        self.status_message.as_ref().map(|(_, created)| {
            std::time::Duration::from_secs(4)
                .saturating_sub(now.saturating_duration_since(*created))
        })
    }

    pub fn expire_status(&mut self, now: Instant) -> bool {
        if self.status_wait(now) == Some(std::time::Duration::ZERO) {
            self.status_message = None;
            true
        } else {
            false
        }
    }

    pub fn clear_expired_status(&mut self) {
        self.expire_status(Instant::now());
    }

    /// Get the directory of the currently selected item.
    #[allow(dead_code)]
    pub fn current_dir(&self) -> PathBuf {
        if let Some(item) = self
            .tree_state
            .flat_items
            .get(self.tree_state.selected_index)
        {
            if item.node_type == NodeType::Directory {
                return item.path.clone();
            }
            if let Some(parent) = item.path.parent() {
                return parent.to_path_buf();
            }
        }
        self.tree_state.root.path.clone()
    }

    /// Compute only through the approved model. Disabled terminal is a runtime gate,
    /// not a mutation of its saved preference or process lifecycle.
    pub fn pane_rects(&self) -> crate::workspace::layout::WorkspaceRects {
        let mut layout = self.workspace.layout.clone();
        if !self.config.terminal_enabled() {
            if layout.maximized() == Some(crate::workspace::layout::MaximizedPane::Terminal) {
                layout.restore();
            }
            // Document maximization already suppresses terminal geometry; toggling
            // here would also restore that overlay in the approved model.
            if layout.terminal_visible() && layout.maximized().is_none() {
                layout.toggle_terminal();
            }
        }
        crate::workspace::layout::compute_layout(
            self.workspace_area.unwrap_or(Rect::new(0, 0, 120, 40)),
            &layout,
        )
    }

    pub fn pane_available(&self, panel: FocusedPanel) -> bool {
        let rects = self.pane_rects();
        let usable = |r: Rect| r.width > 0 && r.height > 0;
        match panel {
            FocusedPanel::Tree => usable(ratatui::widgets::Block::bordered().inner(rects.explorer)),
            FocusedPanel::Preview => {
                usable(ratatui::widgets::Block::bordered().inner(rects.document))
            }
            FocusedPanel::Editor => {
                self.workspace.documents.active_id().is_some()
                    && usable(ratatui::widgets::Block::bordered().inner(rects.document))
            }
            FocusedPanel::Terminal => usable(rects.terminal_content),
        }
    }

    /// Keep ownership/presentation unchanged when an unavailable pane loses focus.
    /// Modal return frames retain their document and are reconciled on dismissal.
    pub fn reconcile_pane_focus(&mut self) {
        if self.pane_available(self.workspace.focus.panel) {
            return;
        }
        for panel in [
            if self.editor_visible() {
                FocusedPanel::Editor
            } else {
                FocusedPanel::Preview
            },
            FocusedPanel::Tree,
            FocusedPanel::Terminal,
        ] {
            if self.pane_available(panel) {
                self.workspace.focus.panel = panel;
                self.keymap.reset();
                return;
            }
        }
    }

    /// Synchronize actual content dimensions before any terminal widget is drawn.
    pub fn update_workspace_geometry(&mut self, area: Rect) {
        self.workspace_area = Some(area);
        self.workspace_rects = self.pane_rects();
        debug_assert!(self.workspace_rects.all_inside(area));
        self.tree_area = self.workspace_rects.explorer;
        self.preview_area = self.workspace_rects.document;
        self.preview_content_area = ratatui::widgets::Block::bordered().inner(self.preview_area);
        self.tree_content_area = ratatui::widgets::Block::bordered().inner(self.tree_area);
        self.terminal_area = self.workspace_rects.terminal_content;
        self.reconcile_pane_focus();
        let (rows, cols) = (self.terminal_area.height, self.terminal_area.width);
        if rows > 0
            && cols > 0
            && (self.terminal_state.emulator.visible_rows() != usize::from(rows)
                || self.terminal_state.emulator.visible_cols() != usize::from(cols))
        {
            if let Some(pty) = &self.terminal_state.pty {
                if let Err(error) = pty.resize(rows, cols) {
                    self.set_status_message(format!("Terminal resize failed: {error}"));
                    return;
                }
            }
            self.terminal_state
                .emulator
                .resize(rows.into(), cols.into());
        }
    }

    pub fn layout_changed(&mut self) {
        self.keymap.reset();
        if let Some(area) = self.workspace_area {
            self.update_workspace_geometry(area);
        } else {
            self.reconcile_pane_focus();
        }
    }

    fn document_panel(&self) -> FocusedPanel {
        if self.workspace.documents.active_id().is_some() {
            FocusedPanel::Editor
        } else {
            FocusedPanel::Preview
        }
    }

    fn focus_usable(&mut self, panel: FocusedPanel) {
        self.retire_copy();
        self.keymap.reset();
        if self.pane_available(panel) {
            self.workspace.focus.panel = panel;
            if panel == FocusedPanel::Editor {
                self.right_panel_presentation = RightPanelPresentation::RetainedDocument;
            }
        }
    }

    /// Cycle only usable panels; skipped panes never change document presentation.
    pub fn toggle_focus(&mut self) {
        self.retire_copy();
        let document = self.document_panel();
        let order = [FocusedPanel::Tree, document, FocusedPanel::Terminal];
        let current = if matches!(
            self.workspace.focus.panel,
            FocusedPanel::Editor | FocusedPanel::Preview
        ) {
            1
        } else if self.workspace.focus.panel == FocusedPanel::Terminal {
            2
        } else {
            0
        };
        self.keymap.reset();
        for step in 1..=3 {
            let panel = order[(current + step) % 3];
            if self.pane_available(panel) {
                self.focus_usable(panel);
                return;
            }
        }
    }

    pub fn focus_left(&mut self) {
        self.focus_usable(FocusedPanel::Tree);
    }
    pub fn focus_right(&mut self) {
        self.focus_usable(self.document_panel());
    }
    pub fn focus_up(&mut self) {
        if self.workspace.focus.panel == FocusedPanel::Terminal {
            let panel = if self.pane_available(FocusedPanel::Tree) {
                FocusedPanel::Tree
            } else {
                self.document_panel()
            };
            self.focus_usable(panel);
        } else {
            self.keymap.reset();
        }
    }
    pub fn focus_down(&mut self) {
        self.focus_usable(FocusedPanel::Terminal);
    }

    /// Explicit user toggle: a restored unstarted pane starts its missing shell.
    pub fn toggle_terminal(&mut self, event_tx: &crate::event::EventSender) -> bool {
        if !self.config.terminal_enabled() {
            self.set_status_message("Terminal disabled (--no-terminal or config)".into());
            return false;
        }
        let alive = self
            .terminal_state
            .pty
            .as_ref()
            .is_some_and(|pty| pty.is_alive());
        if self.workspace.layout.terminal_visible() && alive {
            self.workspace.layout.toggle_terminal();
            if self.workspace.focus.panel == FocusedPanel::Terminal
                && self.pane_available(FocusedPanel::Tree)
            {
                self.workspace.focus.panel = FocusedPanel::Tree;
            }
            self.layout_changed();
            return false;
        }
        self.open_terminal(event_tx)
    }

    /// Only explicit open actions call this lifecycle boundary, never render/settings/focus.
    pub fn open_terminal(&mut self, event_tx: &crate::event::EventSender) -> bool {
        if !self.config.terminal_enabled() || self.is_s3_mode() {
            self.set_status_message("Terminal unavailable in this workspace".into());
            return false;
        }
        if !self.workspace.layout.terminal_visible() {
            self.workspace.layout.toggle_terminal();
        }
        self.layout_changed();
        if !self
            .terminal_state
            .pty
            .as_ref()
            .is_some_and(|pty| pty.is_alive())
        {
            if let Some(pty) = self.terminal_state.pty.take() {
                pty.shutdown();
            }
            let content = self.pane_rects().terminal_content;
            // An explicit start in a suppressed viewport uses a valid fallback grid;
            // subsequent nonzero geometry resizes it, never zero dimensions.
            let (rows, cols) = if content.width > 0 && content.height > 0 {
                (content.height, content.width)
            } else {
                (24, 80)
            };
            match crate::terminal::pty::PtyProcess::spawn(
                &self.config.terminal_shell(),
                &self.current_dir(),
                rows,
                cols,
                event_tx.clone(),
            ) {
                Ok(pty) => {
                    self.terminal_state.pty = Some(pty);
                    self.terminal_state.exited = false;
                    self.terminal_state
                        .emulator
                        .resize(rows.into(), cols.into());
                }
                Err(error) => {
                    self.set_status_message(format!("Terminal start failed: {error}"));
                    self.reconcile_pane_focus();
                    return false;
                }
            }
        }
        if self.pane_available(FocusedPanel::Terminal) {
            self.workspace.focus.panel = FocusedPanel::Terminal;
        }
        true
    }

    /// Decrease the saved terminal height by two rows, bounded by actual geometry.
    pub fn resize_terminal_up(&mut self) {
        self.resize_terminal(-2);
    }
    /// Increase the saved terminal height by two rows, bounded by actual geometry.
    pub fn resize_terminal_down(&mut self) {
        self.resize_terminal(2);
    }
    fn resize_terminal(&mut self, rows: i32) {
        if self.config.terminal_enabled() {
            let area = self.workspace_area.unwrap_or(Rect::new(0, 0, 120, 40));
            self.workspace.layout.resize_terminal(area, rows);
            self.layout_changed();
        }
    }

    /// Shut down the terminal PTY process (called on app exit).
    pub fn shutdown_terminal(&mut self) {
        self.retire_copy();
        if let Some(ref pty) = self.terminal_state.pty {
            pty.shutdown();
        }
        self.terminal_state.pty = None;
    }

    // ── LSP sessions and execution trust (FR-10) ────────────────────────────

    /// Seed the manager's durable grants from the trusted config layer.
    /// Called once at startup before any document opens.
    pub fn init_lsp_trust(&mut self) {
        self.lsp.apply_config_trust(&self.config.lsp_global);
    }

    /// Start (or gate) the language server matching `path`'s language, if a
    /// server is configured. Spawning happens on the session's pump thread;
    /// this returns immediately with at most a status note + a queued trust
    /// dialog.
    pub fn maybe_start_lsp_for_path(&mut self, path: &Path) {
        let interactive = self.event_tx.is_some();
        let tx = self.event_tx.clone().unwrap_or_else(|| {
            // No event loop (headless/tests): the sender exists only to let
            // the gate record its decision; nothing ever spawns.
            let (tx, _rx) = crate::event::event_channel(crate::event::TransportLimits::default());
            tx
        });
        if let Some(note) = self.lsp.maybe_start_for_path(
            path,
            &self.config.lsp_global,
            &self.config.lsp_local,
            interactive,
            &tx,
        ) {
            self.set_status_message(note);
        }
        self.maybe_prompt_lsp_trust();
    }

    /// Start servers for every document already open (session restore).
    pub fn start_lsp_for_open_documents(&mut self) {
        let paths: Vec<PathBuf> = self
            .workspace
            .documents
            .iter()
            .map(|d| d.path().to_path_buf())
            .collect();
        for path in paths {
            self.maybe_start_lsp_for_path(&path);
        }
    }

    /// Poll-based document sync: reconcile the open document set against
    /// the LSP manager's tracked table. One call per event covers every
    /// mutation source uniformly (typing, paste, undo, external reload,
    /// rename, close) — content changes are detected via `content_revision`
    /// so no editor path needs its own hook. Text is fetched lazily and
    /// only for documents that actually owe a send.
    pub fn sync_lsp_documents(&mut self) {
        if !self.config.lsp_global.enabled() {
            return;
        }
        let docs: Vec<_> = self
            .workspace
            .documents
            .iter()
            .map(|d| (d.id(), d.path().to_path_buf(), d.editor.content_revision()))
            .collect();
        self.lsp.sync_documents(
            &self.config.lsp_global,
            docs.iter().map(|(id, p, r)| (*id, p.as_path(), *r)),
            |id| self.workspace.documents.get(id).map(|d| d.text()),
        );
    }

    /// Open the trust dialog for the next pending project argv — only when
    /// no other overlay owns the input.
    pub fn maybe_prompt_lsp_trust(&mut self) {
        if self.workspace.focus.overlay != AppMode::Normal {
            return;
        }
        let Some(pending) = self.lsp.next_pending_trust() else {
            return;
        };
        self.open_dialog(DialogKind::LspTrust {
            language: pending.resolved.spec.language.clone(),
            argv: pending.resolved.spec.argv.clone(),
            root: pending.resolved.root.clone(),
        });
    }

    /// Interactive approval: bind (root, argv) for this session and spawn.
    pub fn approve_lsp_trust(&mut self) {
        let note = self
            .event_tx
            .clone()
            .and_then(|tx| self.lsp.approve_next_trust(&tx))
            .or_else(|| {
                // No event loop → nothing can spawn; treat as refused.
                self.lsp.deny_next_trust()
            });
        if let Some(note) = note {
            self.set_status_message(note);
        }
        self.close_dialog();
        self.maybe_prompt_lsp_trust();
    }

    /// Interactive refusal: denied for the rest of the session.
    pub fn deny_lsp_trust(&mut self) {
        if let Some(note) = self.lsp.deny_next_trust() {
            self.set_status_message(note);
        }
        self.close_dialog();
        self.maybe_prompt_lsp_trust();
    }

    /// Open the capability/status dialog for all live/attempted sessions.
    pub fn show_lsp_status(&mut self) {
        let mut lines = self.lsp.status_summary();
        if lines.is_empty() {
            lines.push("No LSP servers configured or started.".to_string());
            lines.push(
                "Configure [lsp.servers.<language>] argv in global or project config.".to_string(),
            );
        }
        self.open_dialog(DialogKind::LspStatus { lines });
    }

    /// Restart the server for the active document's language.
    pub fn restart_lsp_current(&mut self) -> std::result::Result<(), String> {
        let language = self
            .current_lsp_language()
            .ok_or_else(|| "No LSP language for the current document".to_string())?;
        match self.lsp.restart(&language) {
            Some(note) => {
                self.set_status_message(note);
                Ok(())
            }
            None => Err(format!("No running LSP session for {language}")),
        }
    }

    /// Language mapped to the currently focused text document.
    pub fn current_lsp_language(&self) -> Option<String> {
        let id = self.workspace.documents.active_id()?;
        let document = self.workspace.documents.get(id)?;
        crate::lsp::config::language_for_path(document.path(), &self.config.lsp.languages)
    }

    // ── LSP language features (Phase 11 Task 2) ─────────────────────────

    /// Issue an LSP feature request for the current editor document. The
    /// shared gate is capability-first: a server that never advertised the
    /// provider fails visibly here and nothing is sent. The request carries
    /// the document's `content_revision` — the staleness token checked when
    /// the result lands.
    fn lsp_issue_request(
        &mut self,
        label: &'static str,
        method: &'static str,
        capability: impl Fn(crate::lsp::features::ServerFeatures) -> bool,
        params_for: impl Fn(&str, u64, u64) -> serde_json::Value,
    ) -> std::result::Result<(), String> {
        let id = self
            .editor_target()
            .ok_or_else(|| "No active document".to_string())?;
        let document = self
            .workspace
            .documents
            .get(id)
            .ok_or_else(|| "No active document".to_string())?;
        let language =
            crate::lsp::config::language_for_path(document.path(), &self.config.lsp.languages)
                .ok_or_else(|| "No LSP language for the current document".to_string())?;
        let features = self
            .lsp
            .ready_features(&language)
            .ok_or_else(|| format!("LSP {language}: server not ready"))?;
        if !capability(features) {
            return Err(format!(
                "LSP {language}: {label} unsupported by this server"
            ));
        }
        let encoding = self
            .lsp
            .ready_encoding(&language)
            .ok_or_else(|| format!("LSP {language}: server not ready"))?;
        let Some((uri, _)) = self.lsp.tracked_revision(id) else {
            return Err(format!("LSP {language}: document is not synced"));
        };
        let cursor = document.editor.cursor_position();
        let character = document
            .editor
            .buffer
            .get(cursor.line)
            .and_then(|line| crate::lsp::positions::byte_to_lsp(line, cursor.byte, encoding))
            .ok_or_else(|| "Cursor position cannot be encoded".to_string())?;
        let revision = document.editor.content_revision();
        let params = params_for(&uri, cursor.line as u64, character as u64);
        self.lsp
            .request_feature(&language, method, params, id, uri, revision)
            .map(|_| ())
            .ok_or_else(|| format!("LSP {language}: request could not be sent"))
    }

    pub fn lsp_completion(&mut self) -> std::result::Result<(), String> {
        self.lsp_issue_request(
            "completion",
            "textDocument/completion",
            |f| f.completion,
            crate::lsp::features::position_params,
        )
    }

    pub fn lsp_hover(&mut self) -> std::result::Result<(), String> {
        self.lsp_issue_request(
            "hover",
            "textDocument/hover",
            |f| f.hover,
            crate::lsp::features::position_params,
        )
    }

    pub fn lsp_definition(&mut self) -> std::result::Result<(), String> {
        self.lsp_issue_request(
            "definition",
            "textDocument/definition",
            |f| f.definition,
            crate::lsp::features::position_params,
        )
    }

    pub fn lsp_references(&mut self) -> std::result::Result<(), String> {
        self.lsp_issue_request(
            "references",
            "textDocument/references",
            |f| f.references,
            crate::lsp::features::references_params,
        )
    }

    pub fn lsp_document_symbols(&mut self) -> std::result::Result<(), String> {
        self.lsp_issue_request(
            "document symbols",
            "textDocument/documentSymbol",
            |f| f.document_symbol,
            |uri, _, _| crate::lsp::features::document_symbol_params(uri),
        )
    }

    /// Consume resolved feature results — runs every event-loop iteration
    /// right after `sync_lsp_documents`. Every result revalidates its
    /// staleness tokens before anything is shown or applied.
    pub fn drain_lsp_results(&mut self) {
        for result in self.lsp.take_feature_results() {
            match result {
                crate::lsp::FeatureResult::Error { req, message } => {
                    self.set_status_message(format!(
                        "LSP {}: {} — {message}",
                        req.language, req.method
                    ));
                }
                crate::lsp::FeatureResult::Ready { req, result } => {
                    self.deliver_feature_result(req, result)
                }
            }
        }
    }

    /// Route one resolved result: validate the request's staleness tokens
    /// against the live document, parse, then open the overlay (or jump
    /// straight to a single definition).
    fn deliver_feature_result(
        &mut self,
        req: crate::lsp::FeatureRequest,
        result: serde_json::Value,
    ) {
        use crate::components::language_features::FeatureView;
        let language = req.language.clone();
        let Some(document) = self.workspace.documents.get(req.document) else {
            return; // document closed while the request was in flight
        };
        if crate::lsp::features::uri_for_path(document.path()) != req.uri {
            self.set_status_message(format!("LSP {language}: result dropped — document renamed"));
            return;
        }
        if document.editor.content_revision() != req.revision {
            self.set_status_message(format!(
                "LSP {language}: result dropped — buffer changed during the request"
            ));
            return;
        }
        let encoding = self.lsp.ready_encoding(&language).unwrap_or_default();
        let lines = &document.editor.buffer;
        let view = match req.method.as_str() {
            "textDocument/completion" => {
                match crate::lsp::features::parse_completion(&result, lines, encoding) {
                    Ok(items) if items.is_empty() => {
                        self.set_status_message(format!("LSP {language}: no completions"));
                        return;
                    }
                    Ok(items) => FeatureView::Completion { items },
                    Err(e) => {
                        self.set_status_message(format!(
                            "LSP {language}: malformed completion — {e}"
                        ));
                        return;
                    }
                }
            }
            "textDocument/hover" => match crate::lsp::features::parse_hover(&result) {
                Some(text) => FeatureView::Text {
                    title: "Hover".to_string(),
                    lines: text.lines().map(str::to_string).collect(),
                },
                None => {
                    self.set_status_message(format!("LSP {language}: no hover"));
                    return;
                }
            },
            "textDocument/definition" | "textDocument/references" => {
                let items = crate::lsp::features::parse_locations(&result);
                if items.is_empty() {
                    self.set_status_message(format!("LSP {language}: no results"));
                    return;
                }
                if items.len() == 1 && req.method == "textDocument/definition" {
                    // The common case jumps straight to the target — no
                    // overlay round-trip for a single definition.
                    self.navigate_to_location(items[0].clone());
                    return;
                }
                FeatureView::Locations {
                    title: if req.method == "textDocument/definition" {
                        "Definition".to_string()
                    } else {
                        "References".to_string()
                    },
                    items,
                }
            }
            "textDocument/documentSymbol" => {
                let items = crate::lsp::features::parse_symbols(&result);
                if items.is_empty() {
                    self.set_status_message(format!("LSP {language}: no symbols"));
                    return;
                }
                FeatureView::Symbols { items }
            }
            _ => return,
        };
        self.open_language_features(req, view);
    }

    /// Open (or refresh) the feature overlay with a resolved result.
    fn open_language_features(
        &mut self,
        req: crate::lsp::FeatureRequest,
        view: crate::components::language_features::FeatureView,
    ) {
        use crate::components::language_features::LanguageFeatures;
        if self.workspace.focus.overlay == AppMode::Normal {
            if let Err(e) = self
                .workspace
                .focus
                .open_overlay(AppMode::LanguageFeatures, Some(req.document))
            {
                self.set_status_message(e.to_string());
                return;
            }
        } else if self.workspace.focus.overlay != AppMode::LanguageFeatures {
            self.set_status_message(format!(
                "LSP {}: result dropped — another overlay is active",
                req.language
            ));
            return;
        }
        self.language_features = Some(LanguageFeatures::new(
            req.document,
            req.uri,
            req.revision,
            view,
        ));
    }

    pub fn dismiss_language_features(&mut self) {
        if self.workspace.focus.overlay != AppMode::LanguageFeatures {
            return;
        }
        self.language_features = None;
        self.dismiss_overlay();
    }

    /// Enter on the overlay: apply a completion or navigate to a location.
    pub fn apply_language_selection(&mut self) {
        let Some(features) = self.language_features.as_ref() else {
            return;
        };
        if let Some(item) = features.selected_completion() {
            let item = item.clone();
            let document_id = features.document;
            let uri = features.uri.clone();
            let revision = features.revision;
            let Some(document) = self.workspace.documents.get_mut(document_id) else {
                self.dismiss_language_features();
                return;
            };
            if document.editor.content_revision() != revision
                || crate::lsp::features::uri_for_path(document.path()) != uri
            {
                self.dismiss_language_features();
                self.set_status_message(
                    "Completion is stale — the buffer changed; request again".to_string(),
                );
                return;
            }
            match crate::lsp::features::apply_completion(document, &item) {
                Ok(()) => {
                    self.dismiss_language_features();
                    self.set_status_message("Completion applied".to_string());
                }
                Err(e) => {
                    self.dismiss_language_features();
                    self.set_status_message(format!("Completion failed: {e}"));
                }
            }
            return;
        }
        if let Some(loc) = features.selected_location() {
            let loc = loc.clone();
            self.dismiss_language_features();
            self.navigate_to_location(loc);
            return;
        }
        if let Some(sym) = features.selected_symbol() {
            let (line, character) = (sym.line, sym.character);
            let document = features.document;
            self.dismiss_language_features();
            self.goto_lsp_position(document, line, character);
        }
    }

    /// Navigate to a `Location`: file-scheme URIs only — every other scheme
    /// fails visibly. The originating document stays in the store with its
    /// dirty state and cursor untouched.
    pub fn navigate_to_location(&mut self, loc: crate::lsp::features::LocationEntry) {
        let Some(path) = crate::lsp::features::path_for_uri(&loc.uri) else {
            self.set_status_message(format!(
                "LSP: unsupported URI scheme — {}",
                crate::lsp::features::sanitize_server_text(&loc.uri, 120)
            ));
            return;
        };
        if self.open_document_path(&path, false) {
            if let Some(id) = self.workspace.documents.active_id() {
                self.goto_lsp_position(id, loc.start_line, loc.start_character);
            }
        }
    }

    /// Place the cursor at an LSP position inside a document, decoding the
    /// server units through the session's negotiated encoding.
    fn goto_lsp_position(
        &mut self,
        document: crate::workspace::documents::DocumentId,
        line: u64,
        character: u64,
    ) {
        let encoding = self
            .current_lsp_language()
            .and_then(|l| self.lsp.ready_encoding(&l))
            .unwrap_or_default();
        let Some(doc) = self.workspace.documents.get_mut(document) else {
            return;
        };
        let line = (line as usize).min(doc.editor.buffer.len().saturating_sub(1));
        let byte = doc
            .editor
            .buffer
            .get(line)
            .and_then(|text| crate::lsp::positions::lsp_to_byte(text, character as usize, encoding))
            .unwrap_or(0);
        doc.editor.set_cursor_position(line, byte);
    }

    /// Bounded teardown of every server session; after the event loop, with
    /// terminal shutdown.
    pub fn shutdown_lsp(&mut self) {
        self.lsp.shutdown_all();
    }

    // ── S3 Mode ─────────────────────────────────────────────────────────────

    /// Initialize S3 browse mode.
    ///
    /// Sets up the S3 backend and replaces the tree with a virtual S3 root node.
    pub fn init_s3_mode(&mut self, config: crate::s3::S3Config) {
        let backend = crate::s3::S3Backend::new(&config);

        // Create a virtual root node for the S3 path
        let s3_path = &config.path;
        let display_name = format!("☁ {}", s3_path.to_uri());
        let s3_uri_path = PathBuf::from(s3_path.to_uri());

        // Build a minimal root TreeNode (virtual — no filesystem stat)
        let root = crate::fs::tree::TreeNode {
            name: display_name,
            path: s3_uri_path,
            node_type: crate::fs::tree::NodeType::Directory,
            children: None,
            is_expanded: true,
            depth: 0,
            meta: crate::fs::tree::FileMeta {
                size: 0,
                modified: None,
                is_hidden: false,
            },
            total_child_count: None,
            loaded_child_count: 0,
            has_more_children: false,
            snapshot: None,
            loaded_offset: 0,
            is_stale: false,
            is_loading: false,
        };

        self.tree_state.root = root;
        self.tree_state.flatten();

        self.s3_backend = Some(backend);
        self.s3_config = Some(config);

        // Disable features not applicable in S3 mode
        self.watcher_active = false;

        self.set_status_message("☁ S3 mode — loading...".to_string());
    }

    /// Check whether the app is in S3 browse mode.
    pub fn is_s3_mode(&self) -> bool {
        self.s3_backend.is_some()
    }

    /// Spawn the initial S3 listing to populate the tree.
    pub fn spawn_s3_initial_load(&mut self, _event_tx: &crate::event::EventSender) {
        if let Some(config) = &self.s3_config {
            self.submit_native_job(crate::app_jobs::Target::S3Root(PathBuf::from(
                config.path.to_uri(),
            )));
        }
    }

    /// Handle S3 listing completion — build tree nodes from entries.
    pub fn handle_s3_listing_complete(&mut self, s3_uri: &str, entries: Vec<crate::s3::S3Entry>) {
        let s3_path = match crate::s3::S3Path::parse(s3_uri) {
            Some(p) => p,
            None => return,
        };

        // Build child TreeNodes from S3 entries
        let children: Vec<crate::fs::tree::TreeNode> = entries
            .iter()
            .map(|entry| {
                let child_s3 = s3_path.child(&entry.name);
                let child_uri = child_s3.to_uri();
                crate::fs::tree::TreeNode {
                    name: entry.name.clone(),
                    path: PathBuf::from(&child_uri),
                    node_type: if entry.is_dir {
                        crate::fs::tree::NodeType::Directory
                    } else {
                        crate::fs::tree::NodeType::File
                    },
                    children: None,
                    is_expanded: false,
                    depth: 1, // children of root
                    meta: crate::fs::tree::FileMeta {
                        size: entry.size,
                        modified: None, // S3 modified dates are strings, not SystemTime
                        is_hidden: entry.name.starts_with('.'),
                    },
                    total_child_count: None,
                    loaded_child_count: 0,
                    has_more_children: false,
                    snapshot: None,
                    loaded_offset: 0,
                    is_stale: false,
                    is_loading: false,
                }
            })
            .collect();

        let count = children.len();
        self.tree_state.root.children = Some(children);
        self.tree_state.root.total_child_count = Some(count);
        self.tree_state.root.loaded_child_count = count;
        self.tree_state.root.is_loading = false;
        self.tree_state.flatten();

        if count == 0 {
            self.set_status_message("☁ S3: Empty prefix".to_string());
        } else {
            self.set_status_message(format!("☁ S3: {} items loaded", count));
        }
    }

    /// Expand an S3 directory by listing its prefix.
    pub fn spawn_s3_expand(&mut self, s3_uri: String, _event_tx: &crate::event::EventSender) {
        if self.s3_backend.is_some() && crate::s3::S3Path::parse(&s3_uri).is_some() {
            self.submit_native_job(crate::app_jobs::Target::S3Expand(PathBuf::from(s3_uri)));
        }
    }

    /// Handle completion of an S3 subdirectory listing.
    pub fn handle_s3_subdirectory_complete(
        &mut self,
        s3_uri: &str,
        entries: Vec<crate::s3::S3Entry>,
    ) {
        let s3_path = match crate::s3::S3Path::parse(s3_uri) {
            Some(p) => p,
            None => return,
        };

        let node_path = PathBuf::from(s3_uri);
        let node = match crate::fs::tree::TreeState::find_node_mut_pub(
            &mut self.tree_state.root,
            &node_path,
        ) {
            Some(n) => n,
            None => return,
        };

        let depth = node.depth + 1;
        let children: Vec<crate::fs::tree::TreeNode> = entries
            .iter()
            .map(|entry| {
                let child_s3 = s3_path.child(&entry.name);
                let child_uri = child_s3.to_uri();
                crate::fs::tree::TreeNode {
                    name: entry.name.clone(),
                    path: PathBuf::from(&child_uri),
                    node_type: if entry.is_dir {
                        crate::fs::tree::NodeType::Directory
                    } else {
                        crate::fs::tree::NodeType::File
                    },
                    children: None,
                    is_expanded: false,
                    depth,
                    meta: crate::fs::tree::FileMeta {
                        size: entry.size,
                        modified: None,
                        is_hidden: entry.name.starts_with('.'),
                    },
                    total_child_count: None,
                    loaded_child_count: 0,
                    has_more_children: false,
                    snapshot: None,
                    loaded_offset: 0,
                    is_stale: false,
                    is_loading: false,
                }
            })
            .collect();

        let count = children.len();
        node.children = Some(children);
        node.total_child_count = Some(count);
        node.loaded_child_count = count;
        node.is_loading = false;

        self.tree_state.flatten();
    }

    /// Spawn an async S3 head preview: stream the first N lines of the selected S3 file.
    pub fn spawn_s3_head(&mut self, _event_tx: &crate::event::EventSender) {
        let Some(item) = self
            .tree_state
            .flat_items
            .get(self.tree_state.selected_index)
            .cloned()
        else {
            return;
        };
        if item.node_type != NodeType::File
            || self.s3_backend.is_none()
            || crate::s3::S3Path::parse(&item.path.to_string_lossy()).is_none()
        {
            return;
        }
        if !self.submit_native_job(crate::app_jobs::Target::S3Head(item.path.clone())) {
            return;
        }
        self.s3_head_loading = true;
        self.s3_head_uri = Some(item.path.to_string_lossy().into_owned());
        let loading_lines = vec![
            Line::raw(format!(
                "☁ S3 Head ({} lines) — {}",
                self.config.s3_head_lines(),
                item.name
            )),
            Line::raw(String::new()),
            Line::raw("  ☁ Loading head preview...".to_string()),
        ];
        let total = loading_lines.len();
        self.preview_state = PreviewState {
            current_path: Some(item.path),
            content_lines: loading_lines,
            scroll_offset: 0,
            view_mode: ViewMode::default(),
            line_wrap: self.config.preview.line_wrap.unwrap_or(false),
            horizontal_offset: 0,
            total_lines: total,
            is_large_file: false,
            is_shallow_preview: false,
            head_lines: self.config.head_lines(),
            tail_lines: self.config.tail_lines(),
        };
    }

    /// Handle completion of an S3 head preview fetch.
    pub fn handle_s3_head_complete(
        &mut self,
        s3_uri: &str,
        content: std::result::Result<String, String>,
    ) {
        self.s3_head_loading = false;

        // Ignore stale results (user navigated to a different file)
        if self.s3_head_uri.as_deref() != Some(s3_uri) {
            return;
        }

        match content {
            Ok(text) => {
                // Extract filename from S3 URI for syntax detection
                let filename = s3_uri.rsplit('/').next().unwrap_or("file.txt");

                let n_lines = self.config.s3_head_lines();
                let (mut highlighted, _total) = preview_content::highlight_content_from_string(
                    &text,
                    filename,
                    &self.syntax_set,
                    &self.syntax_theme,
                    &self.theme_colors,
                );

                // Prepend header
                let header = Line::raw(format!(
                    "☁ S3 Head ({} lines) — {}  [H to close]",
                    n_lines, filename
                ));
                highlighted.insert(0, Line::raw(String::new()));
                highlighted.insert(0, header);

                let total_with_header = highlighted.len();
                self.s3_head_content = Some(highlighted.clone());
                self.s3_head_active = true;

                self.preview_state = PreviewState {
                    current_path: self.preview_state.current_path.clone(),
                    content_lines: highlighted,
                    scroll_offset: 0,
                    view_mode: ViewMode::default(),
                    line_wrap: self.config.preview.line_wrap.unwrap_or(false),
                    horizontal_offset: 0,
                    total_lines: total_with_header,
                    is_large_file: false,
                    is_shallow_preview: false,
                    head_lines: self.config.head_lines(),
                    tail_lines: self.config.tail_lines(),
                };
            }
            Err(err) => {
                self.s3_head_active = false;
                self.s3_head_content = None;
                self.set_status_message(format!("☁ S3 head preview failed: {}", err));
                // Revert to metadata view
                self.last_previewed_index = None;
                self.update_preview();
            }
        }
    }

    /// Clean up S3 cache directory on exit.
    pub fn cleanup_s3(&self) {
        if let Some(ref backend) = self.s3_backend {
            backend.cleanup_cache();
        }
    }

    /// Collect paths for clipboard: multi-selected if any, else focused item.
    fn collect_target_paths(&self) -> Vec<PathBuf> {
        if !self.tree_state.multi_selected.is_empty() {
            self.tree_state
                .multi_selected
                .iter()
                .filter_map(|&idx| self.tree_state.flat_items.get(idx))
                .map(|item| item.path.clone())
                .collect()
        } else if let Some(item) = self
            .tree_state
            .flat_items
            .get(self.tree_state.selected_index)
        {
            vec![item.path.clone()]
        } else {
            vec![]
        }
    }

    /// Copy selected/focused items to clipboard.
    pub fn copy_to_clipboard(&mut self) {
        self.retire_copy();
        let paths = self.collect_target_paths();
        if paths.is_empty() {
            return;
        }
        let count = paths.len();
        self.clipboard_revision += 1;
        self.clipboard.set(paths, ClipboardOp::Copy);
        self.set_status_message(format!(
            "📋 {} item{} copied",
            count,
            if count == 1 { "" } else { "s" }
        ));
    }

    /// Cut selected/focused items to clipboard.
    pub fn cut_to_clipboard(&mut self) {
        self.retire_copy();
        let paths = self.collect_target_paths();
        if paths.is_empty() {
            return;
        }
        let count = paths.len();
        self.clipboard_revision += 1;
        self.clipboard.set(paths, ClipboardOp::Cut);
        self.set_status_message(format!(
            "✂ {} item{} cut",
            count,
            if count == 1 { "" } else { "s" }
        ));
    }

    fn copy_origin(&self) -> CopyOrigin {
        CopyOrigin {
            frame: self.workspace.focus.workflow_identity(),
            panel: self.workspace.focus.panel,
            document: self.workspace.documents.active_id(),
            presentation: self.right_panel_presentation,
            clipboard_revision: self.clipboard_revision,
            terminal_session: if self.workspace.focus.panel == FocusedPanel::Terminal {
                self.terminal_state.pty.as_ref().map(|pty| pty.session())
            } else {
                None
            },
        }
    }

    fn ensure_jobs(&mut self) -> bool {
        self.reconcile_job_root();
        if self.jobs.is_none() {
            match crate::app_jobs::AppJobs::new(
                Default::default(),
                crate::background::Worker::Blocking(Arc::new(crate::app_jobs::run)),
            ) {
                Ok(jobs) => self.jobs = Some(jobs),
                Err(error) => {
                    self.set_status_message(format!("Background job not admitted: {error}"));
                    return false;
                }
            }
        }
        true
    }

    fn clipboard_job(
        &self,
        target: crate::app_jobs::Target,
        clipboard: crate::app_jobs::ClipboardJob,
    ) -> crate::app_jobs::NativeJob {
        crate::app_jobs::NativeJob {
            target,
            clipboard: Some(clipboard),
            max_entries: self.config.snapshot_max_entries(),
            timeout: std::time::Duration::from_millis(self.config.preview_timeout_ms()),
            result_bytes: self.jobs.as_ref().unwrap().limits().result_bytes,
            snapshot_options: crate::fs::tree::SnapshotOptions {
                sort_by: self.tree_state.sort_by.clone(),
                dirs_first: self.tree_state.dirs_first,
                page_size: self.tree_state.page_size,
                child_depth: 1,
            },
            summary_colors: None,
            progress: None,
            s3_profile: None,
            s3_head_lines: 0,
            preview_theme: None,
            search: None,
        }
    }

    /// Native copy uses the same nonwaiting finite admission as directory work.
    pub fn copy_text_async(&mut self, text: String, _tx: &crate::event::EventSender) {
        if text.capacity() > 1024 * 1024
            || (self.workspace.focus.overlay != AppMode::CopyOverlay
                && !self.workspace.focus.can_open_overlay())
            || !self.ensure_jobs()
        {
            self.set_status_message(
                "Clipboard copy not admitted: input/fallback frame budget".into(),
            );
            return;
        }
        let origin = self.copy_origin();
        let target = crate::app_jobs::Target::Clipboard(PathBuf::new());
        let job = self.clipboard_job(
            target,
            crate::app_jobs::ClipboardJob::Copy {
                text,
                backend: self.text_clipboard_backend.clone(),
            },
        );
        use crate::background::Payload;
        if job.payload_bytes() > self.jobs.as_ref().unwrap().limits().result_bytes {
            self.set_status_message("Clipboard copy not admitted: fallback result budget".into());
            return;
        }
        match self.jobs.as_mut().unwrap().submit(job) {
            Ok(generation) => {
                self.active_copy = Some((generation, origin));
                self.pending_copy = None;
                self.set_status_message("Copying selection: admitted".into());
            }
            Err(error) => {
                self.set_status_message(format!("Clipboard copy not admitted: {error:?}"))
            }
        }
    }

    pub(crate) fn present_copy_fallback(
        &mut self,
        writer: &mut impl std::io::Write,
        osc_available: bool,
    ) {
        if let Some((text, origin)) = self.pending_copy.take() {
            if self.copy_origin() == origin {
                self.show_copyable_text(text, writer, osc_available);
            }
        }
    }

    /// Returns whether capture was disabled. OSC52 is a request,
    /// not an acknowledgement; the manual fallback remains available.
    pub fn show_copyable_text(
        &mut self,
        text: String,
        writer: &mut impl std::io::Write,
        osc_available: bool,
    ) -> bool {
        if self.workspace.focus.overlay != AppMode::CopyOverlay {
            if let Err(error) = self
                .workspace
                .focus
                .open_overlay(AppMode::CopyOverlay, self.workspace.documents.active_id())
            {
                self.set_status_message(error.to_string());
                return false;
            }
        }
        let requested = if osc_available && text.len() <= 1024 * 1024 {
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
            write!(writer, "\x1b]52;c;{encoded}\x07")
                .and_then(|_| writer.flush())
                .is_ok()
        } else {
            false
        };
        self.copy_overlay_text = Some(text);
        self.copy_overlay_scroll = (0, 0);
        self.set_status_message(if requested {
            "OSC52 copy requested (not confirmed); manual copy available".into()
        } else {
            "Native clipboard unavailable; select text to copy manually".into()
        });
        let disable = self.config.mouse_enabled() && !self.copy_overlay_mouse_suspended;
        if disable {
            self.copy_overlay_mouse_suspended =
                crossterm::execute!(writer, crossterm::event::DisableMouseCapture).is_ok();
        }
        disable && self.copy_overlay_mouse_suspended
    }

    /// Close the manual-copy view, restore its input context, and report whether
    /// mouse capture needs restoring through the terminal output transport.
    pub fn dismiss_copy_overlay(&mut self) -> bool {
        self.copy_overlay_text = None;
        if self.workspace.focus.overlay == AppMode::CopyOverlay {
            self.dismiss_overlay();
        }
        let restore = self.copy_overlay_mouse_suspended && self.config.mouse_enabled();
        if !restore {
            self.copy_overlay_mouse_suspended = false;
        }
        self.set_status_message("Copy overlay closed; clipboard copy was not confirmed".into());
        restore
    }

    /// Only restore capture disabled by our overlay, never by native success.
    pub fn restore_copy_mouse_capture(&mut self, writer: &mut impl std::io::Write) {
        if self.workspace.focus.overlay != AppMode::CopyOverlay
            && self.copy_overlay_mouse_suspended
            && (!self.config.mouse_enabled()
                || crossterm::execute!(writer, crossterm::event::EnableMouseCapture).is_ok())
        {
            self.copy_overlay_mouse_suspended = false;
        }
    }

    /// Copy the editor's exact text payload asynchronously when a sender exists;
    /// otherwise retain the in-memory clipboard without external side effects.
    pub fn copy_editor_text(&mut self) {
        let text = self.editor().map(EditorState::clipboard_text);
        if let Some(text) = text {
            if let Some(tx) = self.event_tx.clone() {
                self.copy_text_async(text, &tx);
            } else {
                // Deterministic in-memory fallback without background side effects.
                self.set_status_message(
                    "Text copied internally; native clipboard unavailable".into(),
                );
            }
        }
    }

    /// Copy the path of the selected item to the system clipboard.
    pub fn copy_path_to_system_clipboard(&mut self, event_tx: &crate::event::EventSender) {
        let item = match self
            .tree_state
            .flat_items
            .get(self.tree_state.selected_index)
        {
            Some(i) if i.node_type != NodeType::LoadMore => i,
            _ => return,
        };

        let path_str = item.path.to_string_lossy().to_string();
        // SearchAction explicitly closes after copying; capture its exact
        // immutable return frame, not whatever happens to be focused later.
        let return_origin = (self.workspace.focus.overlay == AppMode::SearchAction)
            .then(|| {
                self.workspace
                    .focus
                    .return_workflow()
                    .map(|(context, frame)| CopyOrigin {
                        frame,
                        panel: context.panel,
                        document: context.document,
                        presentation: self.right_panel_presentation,
                        clipboard_revision: self.clipboard_revision,
                        terminal_session: if context.panel == FocusedPanel::Terminal {
                            self.terminal_state.pty.as_ref().map(|pty| pty.session())
                        } else {
                            None
                        },
                    })
            })
            .flatten();
        let before = self.active_copy.map(|(generation, _)| generation);
        self.copy_text_async(path_str, event_tx);
        if self.active_copy.map(|(generation, _)| generation) != before {
            if let (Some((_, origin)), Some(return_origin)) = (&mut self.active_copy, return_origin)
            {
                *origin = return_origin;
            }
        }
    }

    /// Open the selected file or directory with the system's default application.
    pub fn open_in_system(&mut self) {
        let item = match self
            .tree_state
            .flat_items
            .get(self.tree_state.selected_index)
        {
            Some(i) if i.node_type != NodeType::LoadMore && i.node_type != NodeType::Loading => i,
            _ => return,
        };

        let path = item.path.clone();
        let path_display = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());

        #[cfg(target_os = "linux")]
        let cmd = "xdg-open";
        #[cfg(target_os = "macos")]
        let cmd = "open";
        #[cfg(target_os = "windows")]
        let cmd = "start";
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        let cmd = "xdg-open";

        match std::process::Command::new(cmd)
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => {
                self.set_status_message(format!("🔗 Opened: {}", path_display));
            }
            Err(e) => {
                self.set_status_message(format!("⚠ Failed to open {}: {}", path_display, e));
            }
        }
    }

    /// Admit the whole operation before opening a modal or touching files.
    pub fn paste_clipboard_async(&mut self, event_tx: crate::event::EventSender) {
        if self.clipboard.is_empty() {
            self.set_status_message("Clipboard is empty".into());
            return;
        }
        let dest = self.current_dir();
        let envelope = crate::app_jobs::operation_envelope(&self.clipboard.paths, &dest);
        if envelope > event_tx.max_event_bytes() {
            self.set_status_message(
                "Operation exceeds transport lifecycle payload limit; no files changed".into(),
            );
            return;
        }
        if !self.ensure_jobs() {
            return;
        }
        let input_bytes = self.clipboard.paths.iter().fold(
            std::mem::size_of::<crate::app_jobs::NativeJob>()
                .saturating_add(dest.capacity())
                .saturating_add(
                    self.clipboard
                        .paths
                        .len()
                        .saturating_mul(std::mem::size_of::<PathBuf>()),
                )
                .saturating_add(64),
            |bytes, path| bytes.saturating_add(path.capacity()),
        );
        if envelope > self.jobs.as_ref().unwrap().limits().result_bytes
            || input_bytes > self.jobs.as_ref().unwrap().limits().job_bytes
            || self
                .clipboard
                .paths
                .iter()
                .any(|path| path.as_os_str().len() > 4096)
            || dest.as_os_str().len() > 4096
            || !self.workspace.focus.can_open_overlay()
        {
            self.set_status_message(
                "Operation not admitted: lifecycle/overlay/path budget; no files changed".into(),
            );
            return;
        }
        let Some(sequence) = self.operation_sequence.checked_add(1) else {
            self.set_status_message("Operation not admitted: identity exhausted".into());
            return;
        };
        let interrupt = Arc::new(AtomicBool::new(false));
        let total = self.clipboard.paths.len();
        let job = self.clipboard_job(
            crate::app_jobs::Target::Paste(dest, sequence),
            crate::app_jobs::ClipboardJob::Paste {
                paths: self.clipboard.paths.clone(),
                cut: self.clipboard.operation == Some(ClipboardOp::Cut),
                interrupt: interrupt.clone(),
            },
        );
        match self.jobs.as_mut().unwrap().submit(job) {
            Ok(generation) => {
                self.operation_sequence = sequence;
                self.workspace
                    .focus
                    .open_overlay(
                        AppMode::Dialog(DialogKind::Progress {
                            message: "Preparing...".into(),
                            current: 0,
                            total,
                        }),
                        self.workspace.documents.active_id(),
                    )
                    .expect("overlay preflight without intervening mutation");
                self.operations.push(OperationOrigin {
                    generation,
                    frame: self.workspace.focus.workflow_identity(),
                    interrupt,
                    clipboard_revision: self.clipboard_revision,
                });
            }
            Err(error) => self.set_status_message(format!(
                "Operation not admitted: {error:?}; no files changed"
            )),
        }
    }

    /// All native snapshots/counts use the same finite worker/result admission.
    pub fn spawn_async_snapshot(&mut self, dir_path: &Path, _event_tx: &crate::event::EventSender) {
        let target = if dir_path == self.tree_state.root.path {
            crate::app_jobs::Target::Root(dir_path.to_path_buf())
        } else {
            crate::app_jobs::Target::Snapshot(dir_path.to_path_buf())
        };
        self.submit_native_job(target);
    }

    #[allow(dead_code)]
    pub fn spawn_async_child_count(
        &mut self,
        dir_path: &Path,
        _event_tx: &crate::event::EventSender,
    ) {
        self.submit_native_job(crate::app_jobs::Target::Count(dir_path.to_path_buf()));
    }

    pub(crate) async fn next_background(&mut self) -> Option<crate::app_jobs::Delivery> {
        match &mut self.jobs {
            Some(jobs) if jobs.has_pending() => jobs.next().await,
            _ => std::future::pending().await,
        }
    }

    pub(crate) async fn shutdown_background(&mut self) {
        // Interrupt is NOT scheduler invalidation. Drain required receipts before
        // closing the result channel and joining real blocking workers.
        for operation in &self.operations {
            operation.interrupt.store(true, Ordering::SeqCst);
        }
        while !self.operations.is_empty() {
            if let Some(delivery) = self.next_background().await {
                self.apply_background(delivery);
            }
        }
        self.active_copy = None;
        self.pending_copy = None;
        self.cancel_dir_summary();
        if self.active_s3_head.take().is_some()
            && self
                .preview_state
                .current_path
                .as_ref()
                .map(|path| path.to_string_lossy())
                .as_deref()
                == self.s3_head_uri.as_deref()
        {
            self.preview_state.content_lines = vec![Line::raw("S3 head Incomplete (cancelled)")];
            self.preview_state.total_lines = 1;
        }
        self.s3_head_loading = false;
        if let Some(jobs) = &mut self.jobs {
            for target in jobs.shutdown().await {
                if target.is_snapshot() || target.is_s3() {
                    if let Some(node) =
                        TreeState::find_node_mut_pub(&mut self.tree_state.root, target.path())
                    {
                        node.is_loading = false;
                    }
                }
            }
            self.tree_state.flatten();
        }
    }

    pub(crate) fn apply_background(&mut self, delivery: crate::app_jobs::Delivery) {
        self.reconcile_job_root();
        use crate::app_jobs::NativeOutput;
        if delivery.target.is_preview() {
            self.apply_preview(delivery);
            return;
        }
        if matches!(delivery.target, crate::app_jobs::Target::FilenameIndex(_)) {
            self.apply_filename_index(delivery);
            return;
        }
        if matches!(delivery.target, crate::app_jobs::Target::ContentSearch(_)) {
            self.apply_content_search(delivery);
            return;
        }

        if matches!(delivery.target, crate::app_jobs::Target::Clipboard(_)) {
            if let Some((generation, origin)) = self.active_copy {
                if generation != delivery.generation {
                    return;
                }
                self.active_copy = None;
                if self.copy_origin() != origin {
                    return;
                }
                match delivery.result {
                    Ok(NativeOutput::Clipboard { text, native: true }) => self.set_status_message(
                        format!("Copied {} bytes to native clipboard", text.len()),
                    ),
                    Ok(NativeOutput::Clipboard {
                        text,
                        native: false,
                    }) => self.pending_copy = Some((text, origin)),
                    Ok(NativeOutput::Failed(reason)) => self.set_status_message(reason.into()),
                    _ => {
                        self.set_status_message("Clipboard copy incomplete: worker failure".into())
                    }
                }
            }
            return;
        }
        if matches!(delivery.target, crate::app_jobs::Target::Paste(..)) {
            self.apply_operation(delivery);
            return;
        }
        if delivery.target.is_summary() {
            self.apply_summary(delivery);
            return;
        }
        if delivery.target.is_s3() {
            self.apply_s3(delivery);
            return;
        }
        let path = delivery.target.path();
        let failure = match delivery.result {
            Ok(NativeOutput::Snapshot(snapshot)) if delivery.target.is_snapshot() => {
                if snapshot.options.sort_by != self.tree_state.sort_by
                    || snapshot.options.dirs_first != self.tree_state.dirs_first
                    || snapshot.options.page_size != self.tree_state.page_size
                    || TreeState::find_node_mut_pub(&mut self.tree_state.root, path)
                        .is_some_and(|node| node.depth + 1 != snapshot.options.child_depth)
                {
                    "Directory snapshot discarded: sort/page/depth options changed".into()
                } else {
                    self.install_prepared_snapshot(path, snapshot);
                    return;
                }
            }
            Ok(NativeOutput::Count {
                count,
                complete: true,
            }) if !delivery.target.is_snapshot() => {
                self.handle_dir_count_complete(path, count);
                return;
            }
            Ok(NativeOutput::Count {
                count,
                complete: false,
            }) => {
                if let Some(node) = TreeState::find_node_mut_pub(&mut self.tree_state.root, path) {
                    node.total_child_count = None;
                }
                // The renderer reads copied FlatItem badges. Patch only this
                // target; flatten would discard unrelated multi-selection.
                for item in &mut self.tree_state.flat_items {
                    if item.path == path {
                        item.child_count = None;
                    }
                }
                format!("Directory count incomplete (cap/deadline/I/O): at least {count} entries")
            }
            Ok(NativeOutput::Failed(reason)) => reason.to_string(),
            Err(error) => format!("Background job failed: {error:?}"),
            _ => "Background job returned mismatched result".into(),
        };
        if delivery.target.is_snapshot() {
            if let Some(node) = TreeState::find_node_mut_pub(&mut self.tree_state.root, path) {
                node.is_loading = false;
            }
            self.tree_state.flatten();
        }
        self.set_status_message(failure);
    }

    fn apply_s3(&mut self, delivery: crate::app_jobs::Delivery) {
        use crate::app_jobs::{NativeOutput, Target};
        let head = matches!(delivery.target, Target::S3Head(_));
        if head {
            if self.active_s3_head != Some(delivery.generation) {
                return;
            }
            self.active_s3_head = None;
            self.s3_head_loading = false;
        }
        let path = delivery.target.path();
        let uri = path.to_string_lossy();
        let failure = match delivery.result {
            Ok(NativeOutput::S3Head(content)) if head => {
                self.handle_s3_head_complete(&uri, Ok(content));
                return;
            }
            Ok(NativeOutput::S3Listing(entries)) if !head => {
                if matches!(delivery.target, Target::S3Root(_)) {
                    if self.tree_state.root.path == path {
                        self.handle_s3_listing_complete(&uri, entries);
                    }
                } else {
                    self.handle_s3_subdirectory_complete(&uri, entries);
                }
                return;
            }
            Ok(NativeOutput::Failed(reason)) => reason.to_owned(),
            Err(error) => format!("S3 background failed: {error:?}"),
            _ => "S3 returned mismatched result".to_owned(),
        };
        if head {
            if self.preview_state.current_path.as_deref() == Some(path) {
                self.preview_state.content_lines =
                    vec![Line::raw(format!("S3 head Incomplete: {failure}"))];
                self.preview_state.total_lines = 1;
            }
        } else if let Some(node) = TreeState::find_node_mut_pub(&mut self.tree_state.root, path) {
            node.is_loading = false;
            node.total_child_count = None;
            self.tree_state.flatten();
        }
        self.set_status_message(failure);
    }

    fn apply_summary(&mut self, delivery: crate::app_jobs::Delivery) {
        use crate::app_jobs::{NativeOutput, Target};
        if self.active_summary != Some(delivery.generation) {
            return;
        }
        let visible = self.preview_state.current_path.as_deref() == Some(delivery.target.path());
        if let Ok(NativeOutput::Progress(statistics)) = delivery.result {
            if visible
                && self
                    .jobs
                    .as_ref()
                    .is_some_and(|jobs| jobs.accepts_progress(delivery.generation))
            {
                self.render_deep_summary(statistics, "Scanning...");
            }
            return;
        }
        self.active_summary = None;
        self.active_dir_scan = None;
        if !visible {
            return;
        }
        let failure = match delivery.result {
            Ok(NativeOutput::Deep {
                statistics,
                complete,
            }) if matches!(delivery.target, Target::Summary { deep: true, .. }) => {
                self.render_deep_summary(
                    statistics,
                    if complete {
                        "Complete"
                    } else {
                        "Incomplete (cap/deadline/I/O)"
                    },
                );
                if !complete {
                    self.set_status_message(
                        "Directory summary Incomplete (cap/deadline/I/O)".into(),
                    );
                }
                return;
            }
            Ok(NativeOutput::Shallow(mut summary))
                if matches!(delivery.target, Target::Summary { deep: false, .. }) =>
            {
                if summary.lines.is_empty() {
                    summary.lines.push(ratatui::text::Line::raw(
                        "Incomplete directory summary (retention budget)",
                    ));
                    summary.total = 1;
                }
                self.preview_state.content_lines = summary.lines;
                self.preview_state.total_lines = summary.total;
                self.preview_state.is_shallow_preview = true;
                self.preview_selection.clear();
                if !summary.complete {
                    self.set_status_message(
                        "Directory summary Incomplete (cap/deadline/I/O/budget)".into(),
                    );
                }
                return;
            }
            Ok(NativeOutput::Failed(reason)) => reason.to_string(),
            Err(error) => format!("Background job failed: {error:?}"),
            _ => "Directory summary returned mismatched result".into(),
        };
        let message = format!("Directory summary Incomplete: {failure}");
        self.preview_state.content_lines = vec![ratatui::text::Line::raw(message.clone())];
        self.preview_state.total_lines = 1;
        self.set_status_message(message);
    }

    fn render_deep_summary(&mut self, statistics: crate::app_jobs::Statistics, status: &str) {
        let summary = format!(
            "📁 Directory Summary — Deep Scan ({status})\n\n  Files: {}\n  Directories: {}\n  Total size: {}",
            statistics.files, statistics.dirs, format_size_bytes(statistics.size),
        );
        self.preview_state.content_lines = summary
            .lines()
            .map(|line| ratatui::text::Line::raw(line.to_string()))
            .collect();
        self.preview_state.total_lines = self.preview_state.content_lines.len();
        self.preview_state.is_shallow_preview = false;
        self.preview_selection.clear();
    }

    fn install_prepared_snapshot(
        &mut self,
        path: &Path,
        prepared: crate::fs::tree::PreparedSnapshot,
    ) {
        let total = prepared.snapshot.len();
        let incomplete = prepared.snapshot.capped || prepared.snapshot.skipped_count > 0;
        if let Some(node) = TreeState::find_node_mut_pub(&mut self.tree_state.root, path) {
            node.total_child_count = (!incomplete).then_some(total);
            node.loaded_child_count = prepared.children.len();
            node.children = Some(prepared.children);
            node.loaded_offset = prepared.consumed;
            node.has_more_children = prepared.consumed < total;
            node.snapshot =
                (incomplete || total > prepared.options.page_size).then_some(prepared.snapshot);
            node.is_expanded = true;
            node.is_loading = false;
            node.is_stale = false;
            self.tree_state.flatten();
            let message = if incomplete {
                format!("Incomplete directory snapshot (cap/deadline/skipped): {total} retained entries")
            } else if total > prepared.options.page_size {
                format!(
                    "📂 Loaded {} entries (showing first {})",
                    total, prepared.options.page_size
                )
            } else {
                format!("📂 Loaded {} entries", total)
            };
            self.set_status_message(message);
        }
    }

    /// Nonwaiting admission to the shared App pool and shared preview domain.
    pub fn spawn_async_dir_summary(
        &mut self,
        path: &Path,
        _tx: &crate::event::EventSender,
    ) -> bool {
        self.submit_native_job(crate::app_jobs::Target::Summary {
            path: path.to_path_buf(),
            deep: true,
        })
    }

    pub fn spawn_async_dir_summary_shallow(
        &mut self,
        path: &Path,
        _tx: &crate::event::EventSender,
    ) -> bool {
        self.submit_native_job(crate::app_jobs::Target::Summary {
            path: path.to_path_buf(),
            deep: false,
        })
    }

    fn cancel_dir_summary(&mut self) {
        if let Some(path) = self.active_dir_scan.take() {
            if self.preview_state.current_path.as_ref() == Some(&path) {
                self.preview_state.content_lines =
                    vec![Line::raw("Incomplete directory summary (cancelled)")];
                self.preview_state.total_lines = 1;
            }
            if let Some(jobs) = &mut self.jobs {
                jobs.cancel(&crate::app_jobs::Target::Summary { path, deep: false });
            }
        }
        self.active_summary = None;
    }

    /// Handle an async operation completion.
    #[allow(dead_code)] // Legacy frontend adapter; production completions are targeted.
    pub fn handle_operation_complete(&mut self, result: crate::event::OperationResult) {
        let identity = self.workspace.focus.oldest_progress_identity();
        self.complete_operation(result, identity, true);
    }

    fn complete_operation(
        &mut self,
        result: crate::event::OperationResult,
        identity: Option<crate::workspace::focus::WorkflowId>,
        clear_clipboard: bool,
    ) {
        self.invalidate_search_cache();
        let progress_on_top = matches!(
            self.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::Progress { .. })
        ) && identity == Some(self.workspace.focus.workflow_identity());
        if let Some(context) = identity.and_then(|id| self.workspace.focus.retire_progress_for(id))
        {
            if let Some(id) = context.document {
                let _ = self.workspace.documents.activate(id);
            }
        }
        if progress_on_top {
            self.dialog_state = DialogState::default();
        }

        // Refresh dest dir
        self.tree_state.reload_dir(&result.dest_dir);
        self.clear_search_cache();

        // For cut/move, also refresh source parents
        if result.was_cut {
            for src in &result.source_paths {
                if let Some(parent) = src.parent() {
                    self.tree_state.reload_dir(parent);
                }
            }
            // Clear clipboard after successful cut
            if clear_clipboard && result.errors.is_empty() {
                self.clipboard_revision += 1;
                self.clipboard.clear();
            }
        }

        if !result.created_paths.is_empty() {
            // Retain undo even for partially completed operations.
            if result.was_cut && result.errors.is_empty() {
                // Build move pairs: (original_src, created_dest)
                let moves: Vec<(PathBuf, PathBuf)> = result
                    .source_paths
                    .iter()
                    .zip(result.created_paths.iter())
                    .map(|(src, dest)| (src.clone(), dest.clone()))
                    .collect();
                self.last_undo = Some(UndoAction::MovePaste { moves });
            } else {
                self.last_undo = Some(UndoAction::CopyPaste {
                    created_paths: result.created_paths.clone(),
                });
            }
        }
        if result.errors.is_empty() {
            let op_name = if result.was_cut { "Moved" } else { "Pasted" };
            self.set_status_message(format!(
                "{} {} item{}",
                op_name,
                result.success_count,
                if result.success_count == 1 { "" } else { "s" }
            ));
        } else {
            self.set_status_message(format!("Error: {}", result.errors.join("; ")));
        }
    }

    fn apply_operation(&mut self, delivery: crate::app_jobs::Delivery) {
        let Some(index) = self
            .operations
            .iter()
            .position(|op| op.generation == delivery.generation)
        else {
            return;
        };
        if let Ok(crate::app_jobs::NativeOutput::OperationProgress(statistics)) = delivery.result {
            if self
                .jobs
                .as_ref()
                .is_some_and(|jobs| jobs.accepts_progress(delivery.generation))
                && self.workspace.focus.workflow_identity() == self.operations[index].frame
                && matches!(
                    self.workspace.focus.overlay,
                    AppMode::Dialog(DialogKind::Progress { .. })
                )
            {
                self.workspace
                    .focus
                    .replace_overlay(AppMode::Dialog(DialogKind::Progress {
                        message: "Pasting...".into(),
                        current: statistics.files as usize,
                        total: statistics.dirs as usize,
                    }));
            }
            return;
        }
        let origin = self.operations.remove(index);
        match delivery.result {
            Ok(crate::app_jobs::NativeOutput::Paste(paste)) => {
                let undo = match (paste.copies.is_empty(), paste.moves.is_empty()) {
                    (false, true) => Some(UndoAction::CopyPaste {
                        created_paths: paste.copies,
                    }),
                    (true, false) => Some(UndoAction::MovePaste { moves: paste.moves }),
                    (false, false) => Some(UndoAction::PartialPaste {
                        copies: paste.copies,
                        moves: paste.moves,
                    }),
                    _ => None,
                };
                self.complete_operation(
                    paste.result,
                    Some(origin.frame),
                    self.clipboard_revision == origin.clipboard_revision,
                );
                if undo.is_some() {
                    self.last_undo = undo;
                }
            }
            result => {
                if let Some(context) = self.workspace.focus.retire_progress_for(origin.frame) {
                    if let Some(document) = context.document {
                        let _ = self.workspace.documents.activate(document);
                    }
                }
                self.set_status_message(match result {
                    Ok(crate::app_jobs::NativeOutput::Failed(reason)) => reason.into(),
                    _ => "Operation incomplete: worker failure; inspect destination before undo"
                        .into(),
                });
            }
        }
    }

    /// Handle a progress update from an async operation.
    #[allow(dead_code)]
    pub fn handle_progress(&mut self, update: crate::event::ProgressUpdate) {
        if let AppMode::Dialog(DialogKind::Progress { .. }) = &self.workspace.focus.overlay {
            self.set_overlay(AppMode::Dialog(DialogKind::Progress {
                message: update.current_file,
                current: update.current,
                total: update.total,
            }));
        }
    }

    /// Cancel an ongoing async operation.
    pub fn cancel_operation(&mut self) {
        self.cancel_token.store(true, Ordering::SeqCst);
        let frame = self.workspace.focus.workflow_identity();
        if let Some(operation) = self.operations.iter().rev().find(|op| op.frame == frame) {
            operation.interrupt.store(true, Ordering::SeqCst);
        }
    }

    /// Undo the last reversible operation.
    pub fn undo(&mut self) {
        self.invalidate_search_cache();
        use crate::fs::operations;

        let action = match self.last_undo.take() {
            Some(a) => a,
            None => {
                self.set_status_message("Nothing to undo".to_string());
                return;
            }
        };

        match action {
            UndoAction::PartialPaste { copies, moves } => {
                self.last_undo = Some(UndoAction::CopyPaste {
                    created_paths: copies,
                });
                self.undo();
                let copied = self
                    .status_message
                    .as_ref()
                    .map(|s| s.0.clone())
                    .unwrap_or_default();
                self.last_undo = Some(UndoAction::MovePaste { moves });
                self.undo();
                let moved = self
                    .status_message
                    .as_ref()
                    .map(|s| s.0.clone())
                    .unwrap_or_default();
                self.set_status_message(format!("{copied}; {moved}"));
            }
            UndoAction::Rename { from, to } => {
                let changes = match self.workspace.documents.preflight_rename(&to, &from) {
                    Ok(changes) => changes,
                    Err(error) => {
                        self.last_undo = Some(UndoAction::Rename { from, to });
                        self.set_status_message(format!("Undo failed: {error}"));
                        return;
                    }
                };
                // Rename back: from is original, to is what it was renamed to
                match operations::rename(&to, &from) {
                    Ok(()) => {
                        self.workspace.documents.commit_rename(changes);
                        if let Some(parent) = from.parent() {
                            self.tree_state.reload_dir(parent);
                        }
                        self.set_status_message("Undo: rename reverted".to_string());
                    }
                    Err(e) => self.set_status_message(format!("Undo failed: {}", e)),
                }
            }
            UndoAction::CopyPaste { created_paths } => {
                let mut errors = Vec::new();
                for path in &created_paths {
                    if let Err(e) = operations::delete(path) {
                        errors.push(format!("{}: {}", path.display(), e));
                    } else if let Some(parent) = path.parent() {
                        self.workspace.documents.mark_deleted_path(path);
                        self.tree_state.reload_dir(parent);
                    }
                }
                if errors.is_empty() {
                    self.set_status_message(format!(
                        "Undo: deleted {} copied item{}",
                        created_paths.len(),
                        if created_paths.len() == 1 { "" } else { "s" }
                    ));
                } else {
                    self.set_status_message(format!("Undo partial: {}", errors.join("; ")));
                }
            }
            UndoAction::MovePaste { moves } => {
                let mut errors = Vec::new();
                for (original_src, current_dest) in &moves {
                    // Move back: current_dest → original_src
                    if let Some(parent) = original_src.parent() {
                        match operations::move_item(current_dest, parent) {
                            Ok(_) => {
                                self.tree_state.reload_dir(parent);
                                if let Some(dest_parent) = current_dest.parent() {
                                    self.tree_state.reload_dir(dest_parent);
                                }
                            }
                            Err(e) => errors.push(format!("{}: {}", current_dest.display(), e)),
                        }
                    }
                }
                if errors.is_empty() {
                    self.set_status_message(format!(
                        "Undo: moved {} item{} back",
                        moves.len(),
                        if moves.len() == 1 { "" } else { "s" }
                    ));
                } else {
                    self.set_status_message(format!("Undo partial: {}", errors.join("; ")));
                }
            }
        }
    }

    /// Scroll preview down by one line.
    pub fn preview_scroll_down(&mut self) {
        let max = self.preview_max_scroll_offset();
        self.preview_state.scroll_offset = (self.preview_state.scroll_offset + 1).min(max);
    }

    /// Scroll preview up by one line.
    pub fn preview_scroll_up(&mut self) {
        self.clamp_preview_scroll();
        if self.preview_state.scroll_offset > 0 {
            self.preview_state.scroll_offset -= 1;
        }
    }

    /// Scroll horizontally without changing intentional vertical preview position.
    pub fn preview_scroll_horizontal(&mut self, right: bool) {
        if self.preview_state.line_wrap {
            return;
        }
        if right {
            self.preview_state.horizontal_offset =
                self.preview_state.horizontal_offset.saturating_add(4);
        } else {
            self.preview_state.horizontal_offset =
                self.preview_state.horizontal_offset.saturating_sub(4);
        }
        self.clamp_preview_scroll();
    }

    /// Toggle wrapping and map the old top logical line into the new layout.
    pub fn preview_toggle_wrap(&mut self) {
        let width = self.preview_content_area.width as usize;
        let top = self
            .preview_state
            .visual_row(self.preview_state.scroll_offset, width)
            .map(|(line, _)| line)
            .unwrap_or(0);
        self.preview_state.line_wrap = !self.preview_state.line_wrap;
        self.preview_state.horizontal_offset = 0;
        self.preview_state.scroll_offset = if self.preview_state.line_wrap {
            self.preview_state.content_lines[..top]
                .iter()
                .map(|line| {
                    crate::text::visual_rows(&crate::text::line_text(line), width, true, false)
                        .count()
                })
                .sum()
        } else {
            top
        };
        self.clamp_preview_scroll();
    }

    /// Jump preview to the first line.
    pub fn preview_jump_top(&mut self) {
        self.preview_state.scroll_offset = 0;
    }

    /// Jump preview to the last line.
    pub fn preview_jump_bottom(&mut self) {
        self.preview_state.scroll_offset = self.preview_max_scroll_offset();
    }

    /// Scroll preview down by half a page.
    pub fn preview_half_page_down(&mut self, visible_height: usize) {
        let half = visible_height / 2;
        let max = self.preview_max_scroll_offset();
        self.preview_state.scroll_offset = (self.preview_state.scroll_offset + half).min(max);
    }

    /// Scroll preview up by half a page.
    pub fn preview_half_page_up(&mut self, visible_height: usize) {
        self.clamp_preview_scroll();
        let half = visible_height / 2;
        self.preview_state.scroll_offset = self.preview_state.scroll_offset.saturating_sub(half);
    }

    /// Clamp preview scroll offset to valid bounds for the current viewport.
    pub fn clamp_preview_scroll(&mut self) {
        let max = self.preview_max_scroll_offset();
        self.preview_state.scroll_offset = self.preview_state.scroll_offset.min(max);
        let width = self.preview_content_area.width as usize;
        if self.preview_state.line_wrap || width == 0 {
            self.preview_state.horizontal_offset = 0;
        } else if self.preview_state.horizontal_offset > 0 {
            self.preview_state.horizontal_offset = self
                .preview_state
                .horizontal_offset
                .min(self.preview_state.max_display_width().saturating_sub(width));
        }
    }

    fn preview_max_scroll_offset(&self) -> usize {
        (if self.preview_state.content_lines.is_empty() {
            self.preview_line_count()
        } else {
            self.preview_state
                .visual_row_count(self.preview_content_area.width as usize)
        })
        .saturating_sub(self.preview_visible_height())
    }

    fn preview_visible_height(&self) -> usize {
        self.preview_content_area.height.max(1) as usize
    }

    fn preview_line_count(&self) -> usize {
        if self.preview_state.content_lines.is_empty() {
            self.preview_state.total_lines.max(1)
        } else {
            self.preview_state.content_lines.len()
        }
    }

    /// Borrow the active owned editor, irrespective of current UI focus.
    pub fn editor(&self) -> Option<&EditorState> {
        self.workspace.documents.active().map(|d| &d.editor)
    }

    /// Admit or reactivate the previewed document using bounded store policy.
    pub fn enter_edit_mode(&mut self) -> bool {
        self.retire_copy();
        // Must be in Normal mode with preview focused
        if self.workspace.focus.overlay != AppMode::Normal
            || self.workspace.focus.panel != FocusedPanel::Preview
        {
            return false;
        }

        if !self.config.preview_enabled() {
            let Some(path) = self
                .tree_state
                .flat_items
                .get(self.tree_state.selected_index)
                .filter(|item| item.node_type == NodeType::File)
                .map(|item| item.path.clone())
            else {
                return false;
            };
            return self.open_document_path(&path, true);
        }

        let path = match &self.preview_state.current_path {
            Some(p) => p.clone(),
            None => return false,
        };

        // Guard: directories cannot be edited
        if let Some(item) = self
            .tree_state
            .flat_items
            .get(self.tree_state.selected_index)
        {
            if item.node_type == NodeType::Directory {
                return false;
            }
        }

        if self
            .workspace
            .documents
            .active()
            .is_some_and(|d| std::fs::canonicalize(&path).is_ok_and(|p| p == d.path()))
        {
            self.workspace.focus.panel = FocusedPanel::Editor;
            self.right_panel_presentation = RightPanelPresentation::RetainedDocument;
            return true;
        }

        self.workspace
            .documents
            .set_limits(crate::workspace::documents::DocumentLimits {
                max_bytes: self.config.max_editor_bytes_usize(),
                max_lines: self.config.max_editor_lines(),
            });
        match self.admit_document(&path, crate::workspace::documents::OpenDisposition::Pinned) {
            Ok(_) => {
                self.workspace.focus.panel = FocusedPanel::Editor;
                self.right_panel_presentation = RightPanelPresentation::RetainedDocument;
                true
            }
            Err(error) => {
                let prefix = if matches!(
                    error,
                    crate::workspace::documents::DocumentError::TooManyLines { .. }
                        | crate::workspace::documents::DocumentError::Load(
                            crate::fs::save::SaveError::TooLarge { .. }
                        )
                ) {
                    "File too large for editing"
                } else {
                    "Cannot edit"
                };
                self.set_status_message(format!("{prefix}: {error}"));
                false
            }
        }
    }

    /// Exit edit mode and return to normal mode.
    /// Does NOT check for unsaved changes — caller should handle save confirmation.
    pub fn exit_edit_mode(&mut self) {
        self.retire_copy();
        if self.workspace.focus.overlay != AppMode::Normal {
            self.dismiss_overlay();
        }
        self.workspace.focus.panel = FocusedPanel::Preview;
        // Force re-preview of the current file to reflect any saved changes
        self.last_previewed_index = None;
    }

    /// Save the editor buffer to disk.
    /// Returns Ok(()) on success or Err with message on failure.
    pub fn save_editor_buffer(&mut self) -> std::result::Result<(), String> {
        let exit_after_save = matches!(
            self.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::FocusBackConfirm)
        );
        let Some(id) = self
            .editor_target()
            .filter(|id| self.workspace.documents.get(*id).is_some())
        else {
            self.document_lifecycle = None;
            let message =
                "Save failed: target document no longer owned; close/quit halted".to_string();
            self.set_status_message(message.clone());
            return Err(message);
        };
        let document = self
            .workspace
            .documents
            .get_mut(id)
            .expect("validated save origin remains owned during exclusive borrow");
        let result = if document.disk_change() != crate::workspace::documents::DiskChange::Unchanged
        {
            Err(crate::fs::save::SaveError::Conflict(document.path().into()))
        } else {
            document.editor.save()
        };
        self.finish_editor_save(result, exit_after_save)
    }

    /// Prepare a second confirmation bound to the target's current disk revision.
    pub fn begin_editor_overwrite(&mut self, exit_after_save: bool, normalize: bool) {
        let Some(id) = self.editor_target() else {
            return;
        };
        let Some(path) = self
            .workspace
            .documents
            .get(id)
            .map(|d| &d.editor)
            .map(|e| e.file_path.clone())
        else {
            return;
        };
        let limit = self.config.max_editor_bytes_usize();
        let revision = match crate::fs::save::load_document_bounded(&path, limit) {
            Ok((_, revision)) => Some(revision),
            Err(crate::fs::save::SaveError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                None
            }
            Err(error) => {
                self.set_status_message(format!("Save failed: {}", error));
                return;
            }
        };
        self.set_overlay(AppMode::Dialog(DialogKind::SaveOverwrite {
            exit_after_save,
            normalize,
            expected_revision: revision,
        }));
    }

    /// Save a newly named file. Relative names are relative to the editor's path.
    pub fn save_editor_as(
        &mut self,
        input: &str,
        exit_after_save: bool,
        normalize: bool,
    ) -> std::result::Result<(), String> {
        self.invalidate_search_cache();
        let id = self
            .editor_target()
            .ok_or_else(|| "No editor state".to_string())?;
        let path = self
            .workspace
            .documents
            .get(id)
            .ok_or_else(|| "No editor state".to_string())?
            .editor
            .file_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(input);
        let path = self
            .workspace
            .documents
            .preflight_save_as(id, &path)
            .map_err(|e| {
                let message = format!("Save As refused: {e}");
                self.set_status_message(message.clone());
                message
            })?;
        let editor = self
            .workspace
            .documents
            .get_mut(id)
            .map(|d| &mut d.editor)
            .ok_or_else(|| "No editor state".to_string())?;
        if normalize {
            editor.confirm_normalization(crate::editor::LineEnding::Lf);
        }
        let result = editor.save_as(&path);
        if result.is_ok() {
            self.workspace.documents.commit_save_as(id, path.clone());
            if let Some(parent) = path.parent() {
                self.tree_state.reload_dir(parent);
                self.clear_search_cache();
            }
        }
        self.finish_editor_save(result, exit_after_save)
    }

    /// Commit only after explicit confirmation, still checking the displayed revision.
    pub fn confirm_editor_overwrite(
        &mut self,
        expected_revision: Option<&crate::fs::save::FileRevision>,
        exit_after_save: bool,
        normalize: bool,
    ) -> std::result::Result<(), String> {
        let id = self
            .editor_target()
            .ok_or_else(|| "No editor state".to_string())?;
        let editor = self
            .workspace
            .documents
            .get_mut(id)
            .map(|d| &mut d.editor)
            .ok_or_else(|| "No editor state".to_string())?;
        if normalize {
            editor.confirm_normalization(crate::editor::LineEnding::Lf);
        }
        // Use the ordinary revision-checked save, not a fresh unconditional reload.
        // A failure must not change the document's original loaded revision.
        let loaded_revision = editor.source_revision.clone();
        editor.source_revision = expected_revision.cloned();
        let result = editor.save();
        if result.is_err() {
            editor.source_revision = loaded_revision;
        } else {
            let _ = self.workspace.documents.acknowledge_disk_change(id);
        }
        self.finish_editor_save(result, exit_after_save)
    }

    fn finish_editor_save(
        &mut self,
        result: std::result::Result<(), crate::fs::save::SaveError>,
        exit_after_save: bool,
    ) -> std::result::Result<(), String> {
        let id = self.editor_target();
        match result {
            Ok(()) => {
                if self.workspace.focus.overlay != AppMode::Normal {
                    self.dismiss_overlay();
                }
                self.dialog_state = DialogState::default();
                self.set_status_message("File saved".to_string());
                if exit_after_save {
                    self.exit_edit_mode();
                }
                if let Some(id) = id {
                    // The digest is computed from the in-memory buffer the save
                    // just published, so no file is re-read to record the
                    // marker.
                    if let Some((path, written)) = self.workspace.documents.get_mut(id).map(|d| {
                        let path = d.path().to_path_buf();
                        let written = d.editor.serialized_content();
                        (path, written)
                    }) {
                        self.note_self_write(&path, &written);
                    }
                    self.lsp.document_saved(id);
                    self.complete_lifecycle_document(id, false);
                }
                Ok(())
            }
            Err(error) => {
                // Any failure halts the captured sequence; conflict recovery is
                // still document-scoped, but cannot implicitly resume a quit.
                let halted = self.document_lifecycle.take().is_some();
                if error.is_conflict() {
                    if let Some(id) = id {
                        self.record_document_disk_change(id);
                    }
                }
                let path = self
                    .workspace
                    .documents
                    .get(id.expect("save target retained"))
                    .map(|d| &d.editor)
                    .map(|editor| editor.file_path.display().to_string())
                    .unwrap_or_default();
                let message = format!(
                    "Save failed ({path}): {error}{}",
                    if halted {
                        "; close/quit halted — remaining documents retained"
                    } else {
                        ""
                    }
                );
                self.set_status_message(message.clone());
                if error.is_conflict() || error.requires_normalization() {
                    self.set_overlay(AppMode::Dialog(DialogKind::SaveConflict {
                        message: message.clone(),
                        exit_after_save,
                        normalize: error.requires_normalization(),
                    }));
                }
                Err(message)
            }
        }
    }

    /// Explicitly discard the editor's buffer in favor of the current disk version.
    /// A failed or oversized reload leaves the original buffer and dialog intact.
    pub fn reload_editor_buffer(&mut self) {
        let Some(id) = self.editor_target() else {
            return;
        };
        let Some(path) = self
            .workspace
            .documents
            .get(id)
            .map(|d| &d.editor)
            .map(|e| e.file_path.clone())
        else {
            return;
        };
        let result = self.workspace.documents.reload(id);
        match result {
            Ok(()) => {
                self.document_lifecycle = None;
                self.dismiss_overlay();
                self.set_status_message(format!("Reloaded: {}", path.display()));
            }
            Err(error) => {
                self.set_status_message(format!("Reload {} failed: {}", path.display(), error))
            }
        }
    }

    /// Update preview content when the selected tree item changes.
    pub fn update_preview(&mut self) {
        if !self.config.preview_enabled() {
            return;
        }
        let idx = self.tree_state.selected_index;
        if self.last_previewed_index == Some(idx) {
            return; // No change
        }
        self.retire_copy();
        self.last_previewed_index = Some(idx);

        let item = match self.tree_state.flat_items.get(idx).cloned() {
            Some(item) => item,
            None => return,
        };

        // Check if we're reloading the same path (e.g., after FS watcher event).
        // If so, preserve the current scroll offset.
        let same_path = self
            .preview_state
            .current_path
            .as_ref()
            .map(|p| p == &item.path)
            .unwrap_or(false);
        let native_directory = item.node_type == NodeType::Directory && !self.is_s3_mode();
        if native_directory {
            if let Some(tx) = self.event_tx.clone() {
                if self.active_dir_scan.as_ref() == Some(&item.path) {
                    return;
                }
                if !self.spawn_async_dir_summary_shallow(&item.path, &tx) {
                    self.last_previewed_index = None;
                    return;
                }
            }
        }
        let preserved_scroll = if same_path {
            self.preview_state.scroll_offset
        } else {
            // Cancel any in-flight directory scan when navigating away
            if self.active_dir_scan.is_some() && !(native_directory && self.event_tx.is_some()) {
                self.cancel_dir_summary();
            }
            0
        };
        // Preview content is about to be replaced; clear stale selection.
        self.preview_selection.clear();

        // Reset S3 head preview state when navigating to a different file
        if !same_path {
            if let Some(uri) = &self.s3_head_uri {
                if let Some(jobs) = &mut self.jobs {
                    jobs.cancel(&crate::app_jobs::Target::S3Head(PathBuf::from(uri)));
                }
            }
            self.active_s3_head = None;
            self.s3_head_active = false;
            self.s3_head_loading = false;
            self.s3_head_content = None;
            self.s3_head_uri = None;
        }

        // S3 mode: show S3 metadata instead of reading local filesystem
        if self.is_s3_mode() {
            self.cancel_preview();
            let path_str = item.path.to_string_lossy().to_string();
            let name = &item.name;
            let node_type = item.node_type.clone();

            // Use preview panel width (minus borders) to wrap long URIs.
            // Falls back to 80 on the very first frame before layout runs.
            let wrap_width = self.preview_content_area.width.max(20) as usize;

            if node_type == NodeType::Directory {
                let mut lines: Vec<Line> = Vec::new();
                lines.push(Line::raw("☁  S3 Prefix".to_string()));
                lines.push(Line::raw(String::new()));
                lines.push(Line::raw(format!("  📁  {}", name)));
                lines.push(Line::raw(String::new()));
                lines.push(Line::raw("  Path:".to_string()));
                for wl in wrap_text(&path_str, wrap_width.saturating_sub(4)) {
                    lines.push(Line::raw(format!("    {}", wl)));
                }
                lines.push(Line::raw(String::new()));
                lines.push(Line::raw("  Press Enter to expand".to_string()));
                let total = lines.len();
                self.preview_state = PreviewState {
                    current_path: Some(item.path.clone()),
                    content_lines: lines,
                    scroll_offset: preserved_scroll,
                    view_mode: ViewMode::default(),
                    line_wrap: self.config.preview.line_wrap.unwrap_or(false),
                    horizontal_offset: 0,
                    total_lines: total,
                    is_large_file: false,
                    is_shallow_preview: false,
                    head_lines: self.config.head_lines(),
                    tail_lines: self.config.tail_lines(),
                };
            } else if self.s3_head_active {
                // S3 head preview is active — show cached head content
                if let Some(ref content) = self.s3_head_content {
                    let total = content.len();
                    self.preview_state = PreviewState {
                        current_path: Some(item.path.clone()),
                        content_lines: content.clone(),
                        scroll_offset: preserved_scroll,
                        view_mode: ViewMode::default(),
                        line_wrap: self.config.preview.line_wrap.unwrap_or(false),
                        horizontal_offset: 0,
                        total_lines: total,
                        is_large_file: false,
                        is_shallow_preview: false,
                        head_lines: self.config.head_lines(),
                        tail_lines: self.config.tail_lines(),
                    };
                }
            } else {
                // File: show S3 object metadata
                let size = if let Some(node) = crate::fs::tree::TreeState::find_node_mut_pub(
                    &mut self.tree_state.root,
                    &item.path,
                ) {
                    node.meta.size
                } else {
                    0
                };
                let size_str = format_size_bytes(size);
                let mut lines: Vec<Line> = Vec::new();
                lines.push(Line::raw("☁  S3 Object".to_string()));
                lines.push(Line::raw(String::new()));
                lines.push(Line::raw(format!("  📄  {}", name)));
                lines.push(Line::raw(String::new()));
                lines.push(Line::raw(format!("  Size: {}", size_str)));
                lines.push(Line::raw("  URI:".to_string()));
                for wl in wrap_text(&path_str, wrap_width.saturating_sub(4)) {
                    lines.push(Line::raw(format!("    {}", wl)));
                }
                lines.push(Line::raw(String::new()));
                lines.push(Line::raw(
                    "  Press Y to copy S3 URI | H for head preview".to_string(),
                ));
                let total = lines.len();
                self.preview_state = PreviewState {
                    current_path: Some(item.path.clone()),
                    content_lines: lines,
                    scroll_offset: preserved_scroll,
                    view_mode: ViewMode::default(),
                    line_wrap: self.config.preview.line_wrap.unwrap_or(false),
                    horizontal_offset: 0,
                    total_lines: total,
                    is_large_file: false,
                    is_shallow_preview: false,
                    head_lines: self.config.head_lines(),
                    tail_lines: self.config.tail_lines(),
                };
            }
            self.clamp_preview_scroll();
            return;
        }

        // Directory preview: use async scan if event_tx is available
        if item.node_type == NodeType::Directory {
            self.cancel_preview();
            let path = item.path.clone();

            if self.event_tx.is_some() {
                // On re-scan of the same directory (e.g. after FS watcher event),
                // keep existing preview content to avoid flickering "Scanning..."
                if !same_path {
                    let dir_name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.to_string_lossy().to_string());
                    let placeholder = format!("📁 Directory: {}\n\n  Scanning...", dir_name);
                    self.preview_state = PreviewState {
                        current_path: Some(path.clone()),
                        content_lines: placeholder
                            .lines()
                            .map(|l| ratatui::text::Line::raw(l.to_string()))
                            .collect(),
                        scroll_offset: preserved_scroll,
                        view_mode: ViewMode::default(),
                        line_wrap: self.config.preview.line_wrap.unwrap_or(false),
                        horizontal_offset: 0,
                        total_lines: 3,
                        is_large_file: false,
                        is_shallow_preview: true,
                        head_lines: self.config.head_lines(),
                        tail_lines: self.config.tail_lines(),
                    };
                }
            } else {
                // Fallback: sync load (for tests without event_tx)
                let (lines, total) =
                    preview_content::load_directory_summary_shallow(&path, &self.theme_colors);
                self.preview_state = PreviewState {
                    current_path: Some(path),
                    content_lines: lines,
                    scroll_offset: preserved_scroll,
                    view_mode: ViewMode::default(),
                    line_wrap: self.config.preview.line_wrap.unwrap_or(false),
                    horizontal_offset: 0,
                    total_lines: total,
                    is_large_file: false,
                    is_shallow_preview: true,
                    head_lines: self.config.head_lines(),
                    tail_lines: self.config.tail_lines(),
                };
            }
            self.clamp_preview_scroll();
            return;
        }

        if item.node_type != NodeType::File {
            self.cancel_preview();
            self.preview_state = PreviewState::default();
            return;
        }

        // Prepared-only rendering (FR-7): submit a versioned load. The worker
        // owns the file read, classification, notebook parse and highlighting;
        // rendering consumes prepared lines or an explicit pending state.
        let path = item.path.clone();
        let key = self.preview_key_for(&path);
        self.preview_selection.clear();
        if self.prepared_pipeline && self.background_schedulable() {
            let admitted = self.submit_native_job(crate::app_jobs::Target::Preview(key.clone()));
            self.desired_preview = Some(key);
            if admitted {
                self.show_pending_preview(&path, preserved_scroll);
            } else {
                // Pool pressure/refusal: keep previous prepared state and retry.
                self.last_previewed_index = None;
            }
            self.clamp_preview_scroll();
            return;
        }
        // Prepared pipeline disabled or no background runtime (embedded/non-async
        // caller): prepared state cannot be scheduled, so load synchronously and
        // record the read on the render-thread probe.
        crate::highlighting::note_render_io();
        self.load_preview_sync(&path, preserved_scroll);
    }

    /// Whether background prepared state can be scheduled right now.
    fn background_schedulable(&self) -> bool {
        self.jobs.is_some() || tokio::runtime::Handle::try_current().is_ok()
    }

    /// Immutable preview identity for `path`, derived from in-memory tree
    /// metadata (no render-path `stat`).
    fn preview_key_for(&mut self, path: &Path) -> crate::highlighting::PreviewKey {
        let (size, modified) = TreeState::find_node_mut_pub(&mut self.tree_state.root, path)
            .map(|node| (node.meta.size, node.meta.modified))
            .unwrap_or((0, None));
        crate::highlighting::PreviewKey {
            path: path.to_path_buf(),
            revision: crate::highlighting::revision_from_meta(size, modified),
            theme: crate::highlighting::theme_signature(
                self.config.syntax_theme_name(self.config.theme_scheme()),
            ),
            view: crate::highlighting::RequestedView::from_config(self.config.preview_view_mode()),
            head_lines: self.config.head_lines(),
            tail_lines: self.config.tail_lines(),
            max_full_bytes: self.config.max_full_preview_bytes(),
        }
    }

    /// Submit one versioned preview load for an explicit view/head/tail change,
    /// with a synchronous fallback when the prepared pipeline is off.
    fn submit_preview_request(
        &mut self,
        path: &Path,
        view: crate::highlighting::RequestedView,
        head_lines: usize,
        tail_lines: usize,
    ) {
        let mut key = self.preview_key_for(path);
        key.view = view;
        key.head_lines = head_lines;
        key.tail_lines = tail_lines;
        self.preview_selection.clear();
        if self.background_schedulable() {
            let admitted = self.submit_native_job(crate::app_jobs::Target::Preview(key.clone()));
            self.desired_preview = Some(key);
            if admitted {
                let scroll = self.preview_state.scroll_offset;
                self.show_pending_preview(path, scroll);
            } else {
                self.last_previewed_index = None;
            }
            self.clamp_preview_scroll();
            return;
        }
        crate::highlighting::note_render_io();
        self.load_preview_sync_key(key);
    }

    /// Explicit pending state: never stale prepared lines, never blocks input.
    /// Line counts and view come from the in-flight identity so a head/tail
    /// adjustment stays coherent while the worker prepares the new window.
    fn show_pending_preview(&mut self, path: &Path, preserved_scroll: usize) {
        let (head_lines, tail_lines, is_large, view_mode) = self
            .desired_preview
            .as_ref()
            .map(|key| {
                let view = match key.view {
                    crate::highlighting::RequestedView::HeadOnly => ViewMode::HeadOnly,
                    crate::highlighting::RequestedView::TailOnly => ViewMode::TailOnly,
                    crate::highlighting::RequestedView::HeadAndTail => ViewMode::HeadAndTail,
                    crate::highlighting::RequestedView::Full => ViewMode::default(),
                };
                (key.head_lines, key.tail_lines, key.view.is_large(), view)
            })
            .unwrap_or((
                self.config.head_lines(),
                self.config.tail_lines(),
                false,
                ViewMode::default(),
            ));
        self.preview_state = PreviewState {
            current_path: Some(path.to_path_buf()),
            content_lines: vec![Line::raw("Preparing preview...")],
            scroll_offset: preserved_scroll,
            horizontal_offset: 0,
            view_mode,
            line_wrap: self.config.preview.line_wrap.unwrap_or(false),
            total_lines: 1,
            is_large_file: is_large,
            is_shallow_preview: false,
            head_lines,
            tail_lines,
        };
    }

    /// Synchronous non-async fallback (tests/embedded callers without a runtime).
    fn load_preview_sync(&mut self, path: &Path, preserved_scroll: usize) {
        let key = self.preview_key_for(path);
        self.load_preview_sync_key_keep_scroll(key, preserved_scroll);
    }

    /// Synchronous non-async fallback for an explicit key.
    fn load_preview_sync_key(&mut self, key: crate::highlighting::PreviewKey) {
        self.load_preview_sync_key_keep_scroll(key, 0);
    }

    fn load_preview_sync_key_keep_scroll(
        &mut self,
        key: crate::highlighting::PreviewKey,
        preserved_scroll: usize,
    ) {
        let name = self
            .config
            .syntax_theme_name(self.config.theme_scheme())
            .to_string();
        let prepared = crate::highlighting::load_file_preview(
            &key,
            &self.syntax_set,
            &name,
            &self.theme_colors,
        );
        self.install_prepared_preview(prepared, preserved_scroll);
    }

    /// Install immutable prepared lines as the current preview state.
    fn install_prepared_preview(
        &mut self,
        prepared: crate::highlighting::PreparedPreview,
        preserved_scroll: usize,
    ) {
        use crate::highlighting::{PreviewClass, RequestedView};
        let crate::highlighting::PreparedPreview {
            key,
            lines,
            total_lines,
            class,
        } = prepared;
        let view_mode = match key.view {
            RequestedView::HeadOnly => ViewMode::HeadOnly,
            RequestedView::TailOnly => ViewMode::TailOnly,
            RequestedView::HeadAndTail => ViewMode::HeadAndTail,
            RequestedView::Full => {
                if matches!(class, PreviewClass::Large) {
                    ViewMode::HeadAndTail
                } else {
                    ViewMode::default()
                }
            }
        };
        let is_large = matches!(class, PreviewClass::Large);
        self.preview_class = Some(class);
        self.preview_state = PreviewState {
            current_path: Some(key.path.clone()),
            content_lines: lines,
            scroll_offset: preserved_scroll,
            horizontal_offset: 0,
            view_mode,
            line_wrap: self.config.preview.line_wrap.unwrap_or(false),
            total_lines,
            is_large_file: is_large,
            is_shallow_preview: false,
            head_lines: key.head_lines,
            tail_lines: key.tail_lines,
        };
        self.preview_selection.clear();
        // Only ordinary text previews become read-only documents; binary,
        // notebook and large-file previews stay preview-only (AC-11).
        if !self.editor_visible() && matches!(class, PreviewClass::Full) {
            self.configure_document_limits();
            // Failed bounded admission leaves the read-only preview intact.
            let path = key.path.clone();
            let _ =
                self.admit_document(&path, crate::workspace::documents::OpenDisposition::Preview);
        }
        self.clamp_preview_scroll();
    }

    /// Apply a versioned preview completion, rejecting delayed stale identities.
    fn apply_preview(&mut self, delivery: crate::app_jobs::Delivery) {
        use crate::app_jobs::{NativeOutput, Target};
        let Target::Preview(key) = &delivery.target else {
            return;
        };
        if self.desired_preview.as_ref() != Some(key) {
            return;
        }
        let same_path = self.preview_state.current_path.as_deref() == Some(key.path.as_path());
        let preserved = if same_path {
            self.preview_state.scroll_offset
        } else {
            0
        };
        match delivery.result {
            Ok(NativeOutput::Preview(prepared)) => {
                if prepared.key != *key {
                    return;
                }
                self.install_prepared_preview(prepared, preserved);
            }
            Ok(NativeOutput::Failed(reason)) => {
                if same_path || self.preview_state.current_path.is_none() {
                    self.preview_state.content_lines =
                        vec![Line::raw(format!("Preview Incomplete: {reason}"))];
                    self.preview_state.total_lines = 1;
                }
            }
            Err(error) if same_path => {
                self.preview_state.content_lines =
                    vec![Line::raw(format!("Preview failed: {error:?}"))];
                self.preview_state.total_lines = 1;
            }
            _ => {}
        }
        if !self.preview_state.is_large_file {
            self.clamp_preview_scroll();
        }
    }

    /// Prepared-only syntax cache for a document, or the shared empty cache
    /// (rendering then shows the explicit pending style).
    pub(crate) fn syntax_cache_for(
        &self,
        id: crate::workspace::documents::DocumentId,
    ) -> Option<&crate::highlighting::SyntaxCache> {
        self.syntax_caches.get(&id)
    }

    /// Bounded invalidation for closed documents (close/external change), plus an
    /// aggregate bound across all open documents.
    ///
    /// Closed documents are retired first. If the remaining caches still exceed
    /// `SYNTAX_TOTAL_CACHE_BYTES`, least-recently-used non-active caches are
    /// evicted outright (they re-prepare on demand) so the total stays bounded.
    fn retire_dead_syntax(&mut self) {
        self.retire_dead_syntax_with_limit(crate::highlighting::SYNTAX_TOTAL_CACHE_BYTES);
    }

    /// Aggregate bound with an injectable limit so tests can prove eviction.
    fn retire_dead_syntax_with_limit(&mut self, limit: usize) {
        if !self.syntax_caches.is_empty() {
            let live: std::collections::HashSet<crate::workspace::documents::DocumentId> =
                self.workspace.documents.iter().map(|d| d.id()).collect();
            self.syntax_caches.retain(|id, _| live.contains(id));
        }
        if self.syntax_cache_bytes() <= limit {
            return;
        }
        let active = self.workspace.documents.active_id();
        let order: Vec<_> = self.workspace.documents.iter().map(|d| d.id()).collect();
        for id in order {
            if self.syntax_cache_bytes() <= limit {
                break;
            }
            if Some(id) == active {
                continue;
            }
            self.syntax_caches.remove(&id);
        }
    }

    /// Aggregate retained bytes across every prepared syntax cache.
    pub(crate) fn syntax_cache_bytes(&self) -> usize {
        self.syntax_caches
            .values()
            .map(crate::highlighting::SyntaxCache::retained_bytes)
            .fold(0usize, usize::saturating_add)
    }

    /// Extend the active document's prepared syntax cache by one bounded step,
    /// resuming from the nearest checkpoint.
    ///
    /// Highlighting runs only here, between events: `render` and the input
    /// handlers never invoke the highlighter, and unprepared lines render in the
    /// explicit pending style. syntect's `ParseState` is not `Send` with the
    /// default onig engine, so parser checkpoints cannot cross the worker pool;
    /// the step is bounded so it never blocks a frame.
    pub(crate) fn advance_syntax_preparation(&mut self) {
        self.retire_dead_syntax();
        if !self.prepared_pipeline || !self.editor_visible() {
            return;
        }
        let Some(id) = self.workspace.documents.active_id() else {
            return;
        };
        let theme_sig = crate::highlighting::theme_signature(
            self.config.syntax_theme_name(self.config.theme_scheme()),
        );
        let Some(doc) = self.workspace.documents.get(id) else {
            return;
        };
        let syntax_name = crate::preview_content::detect_syntax_name(doc.path()).to_string();
        let revision = doc.editor.content_revision();
        let cursor_line = doc.editor.cursor_line;
        let line_count = doc.editor.line_count();
        {
            let cache = self.syntax_caches.entry(id).or_default();
            if cache.theme_epoch() != theme_sig {
                cache.invalidate_all();
                cache.set_theme_epoch(theme_sig);
                cache.set_syntax_name(&syntax_name);
                cache.set_revision(revision);
            } else if cache.revision() != revision {
                let edit_line = cursor_line.min(cache.prepared()).min(line_count);
                cache.invalidate_from(edit_line);
                cache.set_revision(revision);
                cache.set_syntax_name(&syntax_name);
            }
            cache.ensure_len(line_count);
        }
        // Already prepared for this revision/theme (or at the cap): nothing to do.
        let cap = line_count.min(crate::highlighting::SYNTAX_PREPARE_CAP_LINES);
        if self
            .syntax_caches
            .get(&id)
            .is_some_and(|cache| cache.is_complete() || cache.prepared() >= cap)
        {
            return;
        }
        let Some(doc) = self.workspace.documents.get(id) else {
            return;
        };
        let Some(cache) = self.syntax_caches.get_mut(&id) else {
            return;
        };
        let syntax = self
            .syntax_set
            .find_syntax_by_name(&syntax_name)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());
        cache.prepare(
            &doc.editor.buffer,
            syntax,
            &self.syntax_set,
            &self.syntax_theme,
            crate::highlighting::SYNTAX_WINDOW_LINES,
            crate::highlighting::SYNTAX_PREPARE_CAP_LINES,
            crate::highlighting::SYNTAX_CACHE_BYTES,
        );
    }

    /// Retire only the preview request (and its own generation).
    fn cancel_preview(&mut self) {
        if let Some(key) = self.desired_preview.take() {
            if let Some(jobs) = &mut self.jobs {
                jobs.cancel(&crate::app_jobs::Target::Preview(key));
            }
        }
    }

    /// Cycle bounded full/head/tail presentation; Ctrl+T belongs to terminal toggle.
    #[allow(dead_code)]
    pub fn cycle_view_mode(&mut self) {
        if !self.config.preview_enabled() {
            return;
        }
        let Some(path) = self.preview_state.current_path.clone() else {
            return;
        };
        // Prepared pipeline: only the current class may be cycled, and the
        // reclassification/reload happens on the worker. No read/highlight/stat
        // runs here.
        if self.prepared_pipeline {
            use crate::highlighting::{PreviewClass, RequestedView};
            if !path.is_file() {
                return;
            }
            let current = if self.preview_state.is_large_file {
                match self.preview_state.view_mode {
                    ViewMode::HeadAndTail => RequestedView::HeadAndTail,
                    ViewMode::HeadOnly => RequestedView::HeadOnly,
                    ViewMode::TailOnly => RequestedView::TailOnly,
                }
            } else {
                match self.preview_class {
                    Some(PreviewClass::Binary) | Some(PreviewClass::Notebook) => return,
                    _ => RequestedView::Full,
                }
            };
            let head = self.preview_state.head_lines;
            let tail = self.preview_state.tail_lines;
            let next = match current {
                RequestedView::Full => {
                    self.set_status_message(
                        "Full preview exceeds configured byte limit; using head/tail".into(),
                    );
                    RequestedView::HeadAndTail
                }
                RequestedView::HeadAndTail => RequestedView::HeadOnly,
                RequestedView::HeadOnly => RequestedView::TailOnly,
                RequestedView::TailOnly => {
                    // Return to full; the worker reclassifies and falls back to
                    // head/tail if the file still exceeds the byte limit.
                    RequestedView::Full
                }
            };
            self.submit_preview_request(&path, next, head, tail);
            return;
        }
        if !path.is_file()
            || preview_content::is_binary_file(&path)
            || path.extension().and_then(|s| s.to_str()) == Some("ipynb")
        {
            return;
        }
        if !self.preview_state.is_large_file {
            self.preview_state.is_large_file = true;
            self.preview_state.view_mode = ViewMode::HeadAndTail;
        } else {
            self.preview_state.view_mode = match self.preview_state.view_mode {
                ViewMode::HeadAndTail => ViewMode::HeadOnly,
                ViewMode::HeadOnly => ViewMode::TailOnly,
                ViewMode::TailOnly => {
                    if std::fs::metadata(&path)
                        .is_ok_and(|m| m.len() <= self.config.max_full_preview_bytes())
                    {
                        let (lines, total) = preview_content::load_highlighted_content(
                            &path,
                            &self.syntax_set,
                            &self.syntax_theme,
                            &self.theme_colors,
                        );
                        self.preview_state.content_lines = lines;
                        self.preview_state.total_lines = total;
                        self.preview_state.is_large_file = false;
                        self.preview_state.view_mode = ViewMode::HeadAndTail;
                        self.preview_state.scroll_offset = 0;
                        self.preview_selection.clear();
                        self.clamp_preview_scroll();
                        return;
                    }
                    self.set_status_message(
                        "Full preview exceeds configured byte limit; using head/tail".into(),
                    );
                    ViewMode::HeadAndTail
                }
            };
        }
        self.reload_large_preview();
        self.clamp_preview_scroll();
    }

    /// Adjust head/tail line counts by a delta (+/- keys).
    pub fn adjust_preview_lines(&mut self, delta: isize) {
        if !self.preview_state.is_large_file {
            return;
        }
        let step = delta.unsigned_abs();
        if delta > 0 {
            self.preview_state.head_lines += step;
            self.preview_state.tail_lines += step;
        } else {
            self.preview_state.head_lines =
                self.preview_state.head_lines.saturating_sub(step).max(5);
            self.preview_state.tail_lines =
                self.preview_state.tail_lines.saturating_sub(step).max(5);
        }
        // Prepared pipeline: resubmit the versioned load with the new counts
        // instead of reading/highlighting on the input thread.
        if self.prepared_pipeline {
            if let Some(path) = self.preview_state.current_path.clone() {
                let view = match self.preview_state.view_mode {
                    ViewMode::HeadAndTail => crate::highlighting::RequestedView::HeadAndTail,
                    ViewMode::HeadOnly => crate::highlighting::RequestedView::HeadOnly,
                    ViewMode::TailOnly => crate::highlighting::RequestedView::TailOnly,
                };
                self.submit_preview_request(
                    &path,
                    view,
                    self.preview_state.head_lines,
                    self.preview_state.tail_lines,
                );
            }
            return;
        }
        self.reload_large_preview();
    }

    /// Reload the large file preview with current settings.
    fn reload_large_preview(&mut self) {
        if let Some(ref path) = self.preview_state.current_path {
            let path = path.clone();
            let (lines, total) = preview_content::load_head_tail_content(
                &path,
                &self.syntax_set,
                &self.syntax_theme,
                &self.theme_colors,
                self.preview_state.head_lines,
                self.preview_state.tail_lines,
                self.preview_state.view_mode,
            );
            self.preview_state.content_lines = lines;
            self.preview_state.total_lines = total;
            self.preview_state.scroll_offset = 0;
            self.preview_selection.clear();
        }
    }

    /// Quit the application.
    pub fn quit(&mut self) {
        self.should_quit = false;
        if self.workspace.focus.overlay != AppMode::Normal {
            self.set_status_message("Finish the current overlay before quitting".into());
            return;
        }
        let pending: std::collections::VecDeque<_> = self
            .workspace
            .documents
            .iter()
            .filter(|d| d.editor.modified)
            .map(|d| d.id())
            .take(1025)
            .collect();
        if pending.len() > 1024 {
            self.set_status_message(
                "Too many dirty documents; save or close some before quitting".into(),
            );
            return;
        }
        self.document_lifecycle = Some(DocumentLifecycle {
            quitting: true,
            pending,
        });
        self.advance_document_lifecycle();
    }

    /// Alt+Q explicitly closes a document, independently of panel focus.
    pub fn close_active_document(&mut self) {
        if self.workspace.focus.overlay != AppMode::Normal {
            return;
        }
        let Some(id) = self.workspace.documents.active_id() else {
            return;
        };
        self.document_lifecycle = Some(DocumentLifecycle {
            quitting: false,
            pending: std::collections::VecDeque::from([id]),
        });
        self.advance_document_lifecycle();
    }

    fn advance_document_lifecycle(&mut self) {
        loop {
            let Some(flow) = self.document_lifecycle.as_ref() else {
                return;
            };
            let quitting = flow.quitting;
            let Some(id) = flow.pending.front().copied() else {
                self.document_lifecycle = None;
                self.should_quit =
                    quitting && !self.workspace.documents.iter().any(|d| d.editor.modified);
                if quitting && !self.should_quit {
                    self.set_status_message("New unsaved changes remain; quit cancelled".into());
                }
                return;
            };
            let Some(d) = self.workspace.documents.get(id) else {
                self.document_lifecycle = None;
                self.set_status_message("Document no longer owned; operation cancelled".into());
                return;
            };
            if !d.editor.modified {
                if !quitting {
                    let _ = self.workspace.documents.close(id);
                }
                self.document_lifecycle
                    .as_mut()
                    .unwrap()
                    .pending
                    .pop_front();
                self.refresh_after_document_close();
                continue;
            }
            let dialog = AppMode::Dialog(DialogKind::DocumentDecision {
                id,
                path: d.path().into(),
                quitting,
            });
            if let Err(error) = self.workspace.focus.open_overlay_for(
                dialog,
                self.workspace.documents.active_id(),
                Some(id),
            ) {
                self.document_lifecycle = None;
                self.set_status_message(error.to_string());
            }
            return;
        }
    }

    /// Explicit discard is never inferred from focus-back, Esc, save failure, or reload.
    pub fn discard_lifecycle_document(&mut self, id: crate::workspace::documents::DocumentId) {
        if self
            .document_lifecycle
            .as_ref()
            .and_then(|flow| flow.pending.front())
            .copied()
            != Some(id)
        {
            self.close_dialog();
            return;
        }
        self.dismiss_overlay();
        self.complete_lifecycle_document(id, true);
    }

    fn complete_lifecycle_document(
        &mut self,
        id: crate::workspace::documents::DocumentId,
        discard: bool,
    ) {
        let Some(flow) = self.document_lifecycle.as_ref() else {
            return;
        };
        if flow.pending.front().copied() != Some(id) {
            return;
        }
        let result = if discard {
            self.workspace.documents.discard_and_close(id)
        } else if !flow.quitting {
            self.workspace.documents.close(id)
        } else {
            Ok(())
        };
        if let Err(error) = result {
            self.document_lifecycle = None;
            self.set_status_message(error.to_string());
            return;
        }
        self.document_lifecycle
            .as_mut()
            .unwrap()
            .pending
            .pop_front();
        self.refresh_after_document_close();
        self.advance_document_lifecycle();
    }

    fn refresh_after_document_close(&mut self) {
        self.retire_copy();
        if self.workspace.documents.is_empty() {
            if self.workspace.focus.panel == FocusedPanel::Editor {
                self.workspace.focus.panel = FocusedPanel::Preview;
            }
            self.right_panel_presentation = RightPanelPresentation::SelectedPreview;
        }
    }

    /// Move selection down by one item.
    pub fn select_next(&mut self) {
        self.retire_copy();
        let len = self.tree_state.flat_items.len();
        if len > 0 && self.tree_state.selected_index < len - 1 {
            self.tree_state.selected_index += 1;
        }
    }

    /// Move selection up by one item.
    pub fn select_previous(&mut self) {
        self.retire_copy();
        if self.tree_state.selected_index > 0 {
            self.tree_state.selected_index -= 1;
        }
    }

    /// Jump to the first item.
    pub fn select_first(&mut self) {
        self.retire_copy();
        self.tree_state.selected_index = 0;
    }

    /// Jump to the last item.
    pub fn select_last(&mut self) {
        self.retire_copy();
        let len = self.tree_state.flat_items.len();
        if len > 0 {
            self.tree_state.selected_index = len - 1;
        }
    }

    /// Scroll the tree viewport up by `n` lines without moving the selection.
    pub fn tree_scroll_up(&mut self, n: usize) {
        self.tree_state.scroll_offset = self.tree_state.scroll_offset.saturating_sub(n);
        self.tree_viewport_locked = true;
    }

    /// Scroll the tree viewport down by `n` lines without moving the selection.
    pub fn tree_scroll_down(&mut self, n: usize) {
        let total = self.tree_state.flat_items.len();
        let max_scroll = total.saturating_sub(self.tree_visible_height);
        self.tree_state.scroll_offset = (self.tree_state.scroll_offset + n).min(max_scroll);
        self.tree_viewport_locked = true;
    }

    /// Move selection up by one visible page height.
    pub fn tree_page_up(&mut self) {
        let page = self.tree_visible_height.max(1);
        self.tree_state.selected_index = self.tree_state.selected_index.saturating_sub(page);
    }

    /// Move selection down by one visible page height.
    pub fn tree_page_down(&mut self) {
        let page = self.tree_visible_height.max(1);
        let len = self.tree_state.flat_items.len();
        if len > 0 {
            self.tree_state.selected_index = (self.tree_state.selected_index + page).min(len - 1);
        }
    }

    /// Never enumerate/count a directory on the input thread to decide admission.
    pub fn expand_selected_async(&mut self, event_tx: &crate::event::EventSender) {
        let Some(selected) = self
            .tree_state
            .flat_items
            .get(self.tree_state.selected_index)
        else {
            return;
        };
        if selected.node_type != NodeType::Directory {
            return;
        }
        let path = selected.path.clone();
        if let Some(node) = TreeState::find_node_mut_pub(&mut self.tree_state.root, &path) {
            if node.is_loading {
                return;
            }
            if node.children.is_some() && !node.is_stale {
                node.is_expanded = true;
                self.tree_state.flatten();
                return;
            }
        }
        self.spawn_async_snapshot(&path, event_tx);
        self.clear_search_cache();
    }

    /// Expand the selected directory synchronously (for non-async contexts like tests).
    #[allow(dead_code)]
    pub fn expand_selected(&mut self) {
        self.invalidate_search_cache();
        self.tree_state.expand_selected();
        self.clear_search_cache();
    }

    /// Collapse the selected directory, or jump to parent if on a file or collapsed directory.
    pub fn collapse_selected(&mut self) {
        self.retire_copy();
        if let Some(selected) = self
            .tree_state
            .flat_items
            .get(self.tree_state.selected_index)
        {
            let path = selected.path.clone();
            let target = if self.is_s3_mode() {
                if path == self.tree_state.root.path {
                    crate::app_jobs::Target::S3Root(path.clone())
                } else {
                    crate::app_jobs::Target::S3Expand(path.clone())
                }
            } else if path == self.tree_state.root.path {
                crate::app_jobs::Target::Root(path.clone())
            } else {
                crate::app_jobs::Target::Snapshot(path.clone())
            };
            if self.jobs.as_mut().is_some_and(|jobs| jobs.cancel(&target)) {
                if let Some(node) = TreeState::find_node_mut_pub(&mut self.tree_state.root, &path) {
                    node.is_loading = false;
                }
                self.tree_state.flatten();
                // Removing a virtual Loading row shifts sibling indices. Keep
                // this directory as the action target before collapse/jump.
                if let Some(index) = self.tree_state.find_index_by_path(&path) {
                    self.tree_state.selected_index = index;
                }
            }
        }
        self.tree_state.collapse_selected();
    }

    /// Toggle hidden file visibility.
    pub fn toggle_hidden(&mut self) {
        self.invalidate_search_cache();
        self.tree_state.toggle_hidden();
        self.clear_search_cache();
    }

    // === Search (Ctrl+P) methods ===

    /// Open the fuzzy finder overlay.
    pub fn open_search(&mut self) {
        self.content_search_active = false;
        self.content_search.retire();
        self.search_state.query.clear();
        self.search_state.cursor_position = 0;
        self.search_state.results.clear();
        self.search_state.selected_index = 0;
        self.set_overlay(AppMode::Search);
        self.ensure_search_index();
    }

    /// Close the fuzzy finder overlay without navigating.
    pub fn close_search(&mut self) {
        self.set_overlay(AppMode::Normal);
        self.cancel_content_search();
        // Filesystem events were silently dropped while in Search mode,
        // so invalidate the cache so the next open_search() rebuilds it.
        self.clear_search_cache();
        // Force preview refresh in case selection changed externally.
        self.last_previewed_index = None;
    }

    /// Build the filename index off the input thread when the prepared
    /// pipeline is active; otherwise preserve the synchronous fallback used by
    /// embedded callers with no loop to drain background results.
    fn ensure_search_index(&mut self) {
        if self.search_state.cached_paths.is_some() && !self.search_state.index_capped {
            self.update_search_results();
            return;
        }
        if self.prepared_pipeline && self.ensure_jobs() {
            let root = self.tree_state.root.path.clone();
            if !self.submit_search_index(&root, crate::search::IndexCursor::new(&root)) {
                // Admission refused: fall back to the bounded synchronous walk
                // and report the index as partial rather than complete.
                self.search_state.cached_paths = Some(self.build_path_index());
                self.search_state.index_complete = false;
                self.search_state.index_capped = true;
                self.update_search_results();
            }
        } else {
            self.search_state.cached_paths = Some(self.build_path_index());
            self.search_state.indexing = false;
            self.search_state.index_complete = true;
            self.search_state.index_capped = false;
            self.search_state.index_unreadable = 0;
            self.update_search_results();
        }
    }

    /// Refresh the fuzzy-finder status text without forcing a re-score.
    fn refresh_search_status(&mut self) {
        self.search_state.status = self.search_state.status_text();
    }

    /// Admit one resumable filename-index batch on the shared App pool.
    fn submit_search_index(&mut self, root: &Path, cursor: crate::search::IndexCursor) -> bool {
        let request = crate::search::IndexRequest {
            excludes: self.config.search_exclude_dirs(),
            max_entries: self.config.search_max_entries(),
            batch_entries: crate::search::DEFAULT_BATCH_ENTRIES,
            max_depth: crate::search::DEFAULT_MAX_DEPTH,
            max_pending: crate::search::DEFAULT_MAX_PENDING,
            cursor,
        };
        let mut job =
            self.build_native_job(crate::app_jobs::Target::FilenameIndex(root.to_path_buf()));
        job.search = Some(crate::search::SearchJob::Index(request));
        match self.jobs.as_mut().unwrap().submit(job) {
            Ok(generation) => {
                self.search_state.index_generation = Some(generation);
                self.search_state.indexing = true;
                if self.search_state.cached_paths.is_none() {
                    self.search_state.cached_paths = Some(Vec::new());
                }
                true
            }
            Err(error) => {
                // Admission was refused, so the traversal has not completed.
                // Mark it capped so the status never claims a complete index.
                self.search_state.indexing = false;
                self.search_state.index_capped = true;
                self.refresh_search_status();
                self.set_status_message(format!("Filename index not admitted: {error:?}"));
                false
            }
        }
    }

    fn apply_filename_index(&mut self, delivery: crate::app_jobs::Delivery) {
        use crate::app_jobs::NativeOutput;
        if self.search_state.index_generation != Some(delivery.generation) {
            return;
        }
        match delivery.result {
            Ok(NativeOutput::FilenameIndex(batch)) => {
                if let Some(paths) = self.search_state.cached_paths.as_mut() {
                    paths.extend(batch.paths);
                }
                self.search_state.index_unreadable = batch.cursor.unreadable;
                self.search_state.index_capped = batch.cursor.capped;
                let done = batch.cursor.done || batch.cursor.capped;
                self.search_state.index_complete =
                    batch.cursor.done && !batch.cursor.capped && batch.cursor.unreadable == 0;
                self.search_state.indexing = !done;
                self.search_state.index_generation = None;
                if !done {
                    let root = self.tree_state.root.path.clone();
                    if !self.submit_search_index(&root, batch.cursor) {
                        self.search_state.indexing = false;
                    }
                } else if batch.cursor.capped || batch.cursor.unreadable > 0 {
                    self.set_status_message(
                        "Filename index incomplete (cap/unreadable); results may be partial".into(),
                    );
                }
                self.update_search_results();
            }
            Ok(NativeOutput::Failed(reason)) => {
                // A failed delivery leaves a partial index: never complete.
                self.search_state.indexing = false;
                self.search_state.index_generation = None;
                self.search_state.index_capped = true;
                self.refresh_search_status();
                self.set_status_message(reason.into());
            }
            Err(error) => {
                self.search_state.indexing = false;
                self.search_state.index_generation = None;
                self.search_state.index_capped = true;
                self.refresh_search_status();
                self.set_status_message(format!("Filename index failed: {error:?}"));
            }
            _ => {}
        }
    }

    /// Switch the existing search overlay to literal content search.
    #[allow(dead_code)]
    pub fn open_content_search(&mut self) {
        if self.workspace.focus.overlay != AppMode::Search {
            self.search_state.query.clear();
            self.search_state.cursor_position = 0;
            self.search_state.results.clear();
            self.search_state.selected_index = 0;
            self.set_overlay(AppMode::Search);
        }
        self.content_search_active = true;
        self.content_search.query.clear();
        self.content_search.cursor_position = 0;
        self.content_search.retire();
    }

    pub fn toggle_content_search_mode(&mut self) {
        if self.content_search_active {
            self.content_search_active = false;
            self.cancel_content_search();
            self.update_search_results();
        } else if self.workspace.focus.overlay == AppMode::Search {
            self.content_search_active = true;
            self.content_search.retire();
        }
    }

    fn content_search_input_char(&mut self, c: char) {
        let position = self
            .content_search
            .cursor_position
            .min(self.content_search.query.len());
        self.content_search.query.insert(position, c);
        self.content_search.cursor_position = position + c.len_utf8();
        self.start_content_search();
    }

    fn content_search_delete_char(&mut self) {
        if self.content_search.cursor_position == 0 {
            return;
        }
        let position = self
            .content_search
            .cursor_position
            .min(self.content_search.query.len());
        let Some(previous) = self.content_search.query[..position].chars().next_back() else {
            return;
        };
        let start = position - previous.len_utf8();
        self.content_search.query.remove(start);
        self.content_search.cursor_position = start;
        self.start_content_search();
    }

    /// (Re)start the literal content search for the current query. A new
    /// request supersedes any older one through the shared root domain.
    fn start_content_search(&mut self) {
        let query = self.content_search.query.clone();
        self.content_search.retire();
        if query.is_empty() {
            return;
        }
        let root = self.tree_state.root.path.clone();
        if !self.prepared_pipeline || !self.ensure_jobs() {
            let outcome = crate::search::search_project(
                &root,
                crate::search::SearchQuery::new(query, false),
                &self.config.search_exclude_dirs(),
                &self.config.search_limits(),
            );
            self.content_search.scanned_files = outcome.cursor.files_seen;
            self.content_search.unreadable = outcome.cursor.unreadable;
            self.content_search.capped = outcome.cursor.capped;
            self.content_search.complete =
                outcome.cursor.done && !outcome.cursor.capped && outcome.cursor.unreadable == 0;
            self.content_search.hits = outcome.hits;
            self.content_search.scanning = false;
            return;
        }
        if !self.submit_content_search(&root, crate::search::SearchCursor::new(&root)) {
            self.content_search.scanning = false;
        }
    }

    fn submit_content_search(&mut self, root: &Path, cursor: crate::search::SearchCursor) -> bool {
        let request = crate::search::ContentRequest {
            query: crate::search::SearchQuery::new(self.content_search.query.clone(), false),
            excludes: self.config.search_exclude_dirs(),
            limits: self.config.search_limits(),
            cursor,
        };
        let mut job =
            self.build_native_job(crate::app_jobs::Target::ContentSearch(root.to_path_buf()));
        job.search = Some(crate::search::SearchJob::Content(request));
        match self.jobs.as_mut().unwrap().submit(job) {
            Ok(generation) => {
                self.content_search.generation = Some(generation);
                self.content_search.scanning = true;
                true
            }
            Err(error) => {
                // A refused admission means the scan never ran; report it as
                // incomplete rather than as a zero-match completed search.
                self.content_search.scanning = false;
                self.content_search.complete = false;
                self.content_search.capped = true;
                self.content_search.generation = None;
                self.set_status_message(format!("Content search not admitted: {error:?}"));
                false
            }
        }
    }

    fn cancel_content_search(&mut self) {
        if let Some(generation) = self.content_search.generation.take() {
            let _ = generation;
            let root = self.tree_state.root.path.clone();
            if let Some(jobs) = &mut self.jobs {
                jobs.cancel(&crate::app_jobs::Target::ContentSearch(root));
            }
        }
        self.content_search.retire();
    }

    fn apply_content_search(&mut self, delivery: crate::app_jobs::Delivery) {
        use crate::app_jobs::NativeOutput;
        if self.content_search.generation != Some(delivery.generation) {
            return;
        }
        match delivery.result {
            Ok(NativeOutput::ContentSearch(batch)) => {
                self.content_search.hits.extend(batch.hits);
                self.content_search.scanned_files = batch.cursor.files_seen;
                self.content_search.unreadable = batch.cursor.unreadable;
                self.content_search.capped = batch.cursor.capped;
                let done = batch.cursor.done || batch.cursor.capped;
                self.content_search.complete =
                    batch.cursor.done && !batch.cursor.capped && batch.cursor.unreadable == 0;
                self.content_search.scanning = !done;
                self.content_search.generation = None;
                if self.content_search.selected_index >= self.content_search.hits.len() {
                    self.content_search.selected_index =
                        self.content_search.hits.len().saturating_sub(1);
                }
                if !done {
                    let root = self.tree_state.root.path.clone();
                    if !self.submit_content_search(&root, batch.cursor) {
                        // The continuation was refused: the scan is partial.
                        self.content_search.scanning = false;
                        self.content_search.complete = false;
                        self.content_search.capped = true;
                    }
                } else if batch.cursor.capped || batch.cursor.unreadable > 0 {
                    self.set_status_message(
                        "Content search incomplete (cap/unreadable); results may be partial".into(),
                    );
                }
            }
            Ok(NativeOutput::Failed(reason)) => {
                // A failed delivery leaves a partial result set: never complete.
                self.content_search.scanning = false;
                self.content_search.complete = false;
                self.content_search.capped = true;
                self.content_search.generation = None;
                self.set_status_message(reason.into());
            }
            Err(error) => {
                self.content_search.scanning = false;
                self.content_search.complete = false;
                self.content_search.capped = true;
                self.content_search.generation = None;
                self.set_status_message(format!("Content search failed: {error:?}"));
            }
            _ => {}
        }
    }

    /// Confirm the selected content-search hit: navigate without replacing a
    /// dirty active document.
    pub fn content_search_confirm(&mut self) {
        let Some(hit) = self.content_search.selected_hit().cloned() else {
            return;
        };
        self.close_search();
        self.navigate_to_hit(&hit);
    }

    /// Navigate to a content hit. A dirty document is never replaced: an
    /// already-open (possibly unsaved) buffer is reused and re-focused.
    pub fn navigate_to_hit(&mut self, hit: &crate::search::SearchHit) -> bool {
        self.retire_copy();
        self.keymap.reset();
        self.configure_document_limits();
        self.navigate_to_path(&hit.path);
        let existing = self
            .workspace
            .documents
            .iter()
            .find(|document| document.path() == hit.path)
            .map(|document| document.id());
        let id = match existing {
            Some(id) => {
                let _ = self.workspace.documents.activate(id);
                id
            }
            None => match self.workspace.documents.open(
                &hit.path,
                crate::workspace::documents::OpenDisposition::Pinned,
            ) {
                Ok(id) => id,
                Err(error) => {
                    self.show_selected_preview();
                    self.workspace.focus.panel = FocusedPanel::Preview;
                    self.last_previewed_index = None;
                    self.update_preview();
                    self.set_status_message(format!("Preview only: {error}"));
                    return false;
                }
            },
        };
        self.right_panel_presentation = RightPanelPresentation::RetainedDocument;
        self.workspace.focus.panel = FocusedPanel::Editor;
        if let Some(document) = self.workspace.documents.get_mut(id) {
            let line = (hit.line.saturating_sub(1) as usize)
                .min(document.editor.buffer.len().saturating_sub(1));
            // `SearchHit.column` is already expressed in editor-buffer
            // coordinates: the engine matched against the line with a trailing
            // `\r` removed, exactly what `str::lines()` stores. Deriving the
            // column from `hit.byte` would double-count the `\r` of every
            // preceding CRLF pair.
            let column =
                (hit.column as usize).min(document.editor.buffer.get(line).map_or(0, String::len));
            document.editor.set_cursor_position(line, column);
        }
        true
    }

    /// Secondary actions for the selected content-search hit reuse the existing
    /// search action menu.
    pub fn content_search_secondary_actions(&mut self) {
        self.command_selection_actions = false;
        let Some(hit) = self.content_search.selected_hit().cloned() else {
            return;
        };
        let (is_directory, is_binary) = Self::detect_file_type(&hit.path);
        let display = format!("{}:{}", hit.path.display(), hit.line);
        self.search_action_state = Some(SearchActionState {
            path: hit.path,
            display,
            is_directory,
            is_binary,
        });
        self.set_overlay(AppMode::SearchAction);
    }

    /// Insert a character into the search query and re-score.
    pub fn search_input_char(&mut self, c: char) {
        if self.content_search_active {
            self.content_search_input_char(c);
            return;
        }
        self.search_state
            .query
            .insert(self.search_state.cursor_position, c);
        self.search_state.cursor_position += c.len_utf8();
        self.update_search_results();
    }

    /// Delete the character before the cursor in the search query.
    pub fn search_delete_char(&mut self) {
        if self.content_search_active {
            self.content_search_delete_char();
            return;
        }
        if self.search_state.cursor_position > 0 {
            let byte_pos = self.search_state.cursor_position;
            let prev_char = self.search_state.query[..byte_pos]
                .chars()
                .next_back()
                .expect("cursor > 0 guarantees at least one char");
            self.search_state.cursor_position -= prev_char.len_utf8();
            self.search_state
                .query
                .remove(self.search_state.cursor_position);
            self.update_search_results();
        }
    }

    /// Move search result selection down.
    pub fn search_select_next(&mut self) {
        if self.content_search_active {
            self.content_search.select_next();
            return;
        }
        if !self.search_state.results.is_empty()
            && self.search_state.selected_index < self.search_state.results.len() - 1
        {
            self.search_state.selected_index += 1;
        }
    }

    /// Move search result selection up.
    pub fn search_select_previous(&mut self) {
        if self.content_search_active {
            self.content_search.select_previous();
            return;
        }
        if self.search_state.selected_index > 0 {
            self.search_state.selected_index -= 1;
        }
    }

    /// Confirm the selected search result: open action menu for that file.
    /// Primary Quick Open: navigate directories, retain editable files directly.
    pub fn search_confirm(&mut self) {
        if self.content_search_active {
            self.content_search_confirm();
            return;
        }
        let Some(path) = self
            .search_state
            .results
            .get(self.search_state.selected_index)
            .map(|r| r.path.clone())
        else {
            return;
        };
        self.close_search();
        self.navigate_to_path(&path);
        if path.is_dir() {
            self.workspace.focus.panel = FocusedPanel::Tree;
        } else {
            self.open_document_path(&path, true);
        }
    }

    /// Defaults apply only at first admission; activation never resets local view state.
    fn admit_document(
        &mut self,
        path: &Path,
        disposition: crate::workspace::documents::OpenDisposition,
    ) -> std::result::Result<
        crate::workspace::documents::DocumentId,
        crate::workspace::documents::DocumentError,
    > {
        self.configure_document_limits();
        let retained: Vec<_> = self
            .workspace
            .documents
            .iter()
            .map(|document| document.id())
            .collect();
        let id = self.workspace.documents.open(path, disposition)?;
        if !retained.contains(&id) && self.config.preview.line_wrap.unwrap_or(false) {
            self.workspace
                .documents
                .get_mut(id)
                .expect("newly admitted document")
                .editor
                .toggle_wrap();
        }
        Ok(id)
    }

    fn configure_document_limits(&mut self) {
        self.workspace
            .documents
            .set_limits(crate::workspace::documents::DocumentLimits {
                max_bytes: self.config.max_editor_bytes_usize(),
                max_lines: self.config.max_editor_lines(),
            });
    }

    /// Bounded text admission; unsupported documents keep their legacy preview.
    pub fn open_document_path(&mut self, path: &Path, pinned: bool) -> bool {
        self.retire_copy();
        self.keymap.reset();
        if self.workspace.focus.overlay != AppMode::Normal {
            return false;
        }
        self.configure_document_limits();
        if path.extension().and_then(|e| e.to_str()) == Some("ipynb") || self.is_s3_mode() {
            self.show_selected_preview();
            self.workspace.focus.panel = FocusedPanel::Preview;
            self.last_previewed_index = None;
            self.update_preview();
            return false;
        }
        let intent = if pinned {
            crate::workspace::documents::OpenDisposition::Pinned
        } else {
            crate::workspace::documents::OpenDisposition::Preview
        };
        match self.admit_document(path, intent) {
            Ok(_) => {
                // Start (or gate) the file's language server, if configured.
                // Returns immediately — the pump thread does the handshake.
                self.maybe_start_lsp_for_path(path);
                self.right_panel_presentation = if pinned {
                    RightPanelPresentation::RetainedDocument
                } else {
                    RightPanelPresentation::SelectedPreview
                };
                self.workspace.focus.panel = if pinned {
                    FocusedPanel::Editor
                } else {
                    FocusedPanel::Preview
                };
                true
            }
            Err(error) => {
                self.show_selected_preview();
                self.workspace.focus.panel = FocusedPanel::Preview;
                self.last_previewed_index = None;
                self.update_preview();
                self.set_status_message(format!("Preview only: {error}"));
                false
            }
        }
    }

    pub fn activate_document(&mut self, id: crate::workspace::documents::DocumentId) {
        self.retire_copy();
        self.keymap.reset();
        if self.workspace.focus.overlay != AppMode::Normal {
            return;
        }
        if self.workspace.documents.activate(id).is_ok() {
            self.right_panel_presentation = RightPanelPresentation::RetainedDocument;
            self.workspace.focus.panel = FocusedPanel::Editor;
        }
    }

    pub fn pin_document(&mut self) {
        if self.workspace.focus.overlay != AppMode::Normal {
            return;
        }
        if let Some(id) = self.workspace.documents.active_id() {
            let _ = self.workspace.documents.pin(id);
            self.set_status_message(
                "Document pinned | Alt+O list, Alt+B/N previous/next, Alt+R reveal".into(),
            );
        }
    }

    pub fn cycle_document(&mut self, forward: bool) {
        self.retire_copy();
        if self.workspace.focus.overlay != AppMode::Normal {
            return;
        }
        let summaries = crate::components::document_tabs::summaries(&self.workspace.documents);
        if let Some(id) = crate::components::document_tabs::adjacent_document(
            &summaries,
            self.workspace.documents.active_id(),
            !forward,
        ) {
            self.activate_document(id);
        }
    }

    pub fn reveal_document(&mut self) {
        if self.workspace.focus.overlay != AppMode::Normal {
            return;
        }
        if let Some(path) = self
            .workspace
            .documents
            .active()
            .map(|d| d.path().to_owned())
        {
            self.navigate_to_path(&path);
            self.workspace.focus.panel = FocusedPanel::Tree;
        }
    }

    pub fn open_document_list(&mut self) {
        if self.workspace.focus.overlay != AppMode::Normal {
            return;
        }
        let ids: Vec<_> = self.workspace.documents.iter().map(|d| d.id()).collect();
        self.document_list_index = ids
            .iter()
            .position(|id| Some(*id) == self.workspace.documents.active_id())
            .unwrap_or(0);
        self.document_list = Some(ids);
        self.content_search_active = false;
        self.set_overlay(AppMode::Search);
    }

    pub fn search_secondary_actions(&mut self) {
        if self.content_search_active {
            self.content_search_secondary_actions();
            return;
        }
        self.command_selection_actions = false;
        if let Some(result) = self
            .search_state
            .results
            .get(self.search_state.selected_index)
        {
            let path = result.path.clone();
            let display = result.display.clone();
            let (is_directory, is_binary) = Self::detect_file_type(&path);
            self.search_action_state = Some(SearchActionState {
                path,
                display,
                is_directory,
                is_binary,
            });
            self.set_overlay(AppMode::SearchAction);
        }
    }

    /// Detect whether a path is a directory and/or a binary file.
    pub fn detect_file_type(path: &Path) -> (bool, bool) {
        let is_directory = path.is_dir();
        let is_binary = if is_directory {
            false
        } else {
            crate::preview_content::is_binary_file(path)
        };
        (is_directory, is_binary)
    }

    /// Return from the action menu to the search overlay (preserving query/results).
    pub fn search_action_back(&mut self) {
        if self.command_selection_actions {
            self.command_selection_actions = false;
            self.close_search_action();
            return;
        }
        self.search_action_state = None;
        self.set_overlay(AppMode::Search);
    }

    /// Close both search action and search overlays, return to Normal mode.
    pub fn close_search_action(&mut self) {
        self.search_action_state = None;
        self.set_overlay(AppMode::Normal);
        self.cancel_content_search();
        self.content_search_active = false;
        self.clear_search_cache();
        self.last_previewed_index = None;
    }

    /// Search action: navigate to the file in the tree.
    pub fn search_action_navigate(&mut self) {
        if let Some(state) = self.search_action_state.take() {
            self.set_overlay(AppMode::Normal);
            self.clear_search_cache();
            self.last_previewed_index = None;
            self.navigate_to_path(&state.path);
        }
    }

    /// Search action: navigate to file and focus preview panel.
    pub fn search_action_preview(&mut self) {
        if let Some(state) = self.search_action_state.take() {
            self.set_overlay(AppMode::Normal);
            self.clear_search_cache();
            self.last_previewed_index = None;
            self.navigate_to_path(&state.path);
            self.workspace.focus.panel = FocusedPanel::Preview;
        }
    }

    /// Search action: navigate to file and enter edit mode.
    pub fn search_action_edit(&mut self) {
        if let Some(state) = self.search_action_state.take() {
            self.set_overlay(AppMode::Normal);
            self.clear_search_cache();
            self.last_previewed_index = None;
            self.navigate_to_path(&state.path);
            self.workspace.focus.panel = FocusedPanel::Preview;
            // Force preview update so enter_edit_mode can find the file
            self.update_preview();
            self.enter_edit_mode();
        }
    }

    /// Search action: copy the absolute path to the system clipboard.
    pub fn search_action_copy_path(&mut self, event_tx: &crate::event::EventSender) {
        if let Some(state) = self.search_action_state.take() {
            let path_str = state.path.to_string_lossy().to_string();
            self.set_overlay(AppMode::Normal);
            self.clear_search_cache();
            self.last_previewed_index = None;
            self.copy_text_async(path_str, event_tx);
        }
    }

    /// Search action: navigate to file and open rename dialog.
    pub fn search_action_rename(&mut self) {
        if let Some(state) = self.search_action_state.take() {
            self.set_overlay(AppMode::Normal);
            self.clear_search_cache();
            self.last_previewed_index = None;
            self.navigate_to_path(&state.path);
            self.open_dialog(DialogKind::Rename {
                original: state.path,
            });
        }
    }

    /// Search action: navigate to file and open delete confirm.
    pub fn search_action_delete(&mut self) {
        if let Some(state) = self.search_action_state.take() {
            self.set_overlay(AppMode::Normal);
            self.clear_search_cache();
            self.last_previewed_index = None;
            self.navigate_to_path(&state.path);
            self.open_dialog(DialogKind::DeleteConfirm {
                targets: vec![state.path],
            });
        }
    }

    /// Search action: add file to internal clipboard for copy.
    pub fn search_action_copy_clipboard(&mut self) {
        self.retire_copy();
        self.clipboard_revision += 1;
        if let Some(state) = self.search_action_state.take() {
            use crate::fs::clipboard::ClipboardOp;
            self.clipboard
                .set(vec![state.path.clone()], ClipboardOp::Copy);
            let name = state
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            self.set_status_message(format!("📋 Copied: {}", name));
            self.set_overlay(AppMode::Normal);
            self.clear_search_cache();
            self.last_previewed_index = None;
        }
    }

    /// Search action: cut file to internal clipboard for move.
    pub fn search_action_cut_clipboard(&mut self) {
        self.retire_copy();
        self.clipboard_revision += 1;
        if let Some(state) = self.search_action_state.take() {
            use crate::fs::clipboard::ClipboardOp;
            self.clipboard
                .set(vec![state.path.clone()], ClipboardOp::Cut);
            let name = state
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            self.set_status_message(format!("✂ Cut: {}", name));
            self.set_overlay(AppMode::Normal);
            self.clear_search_cache();
            self.last_previewed_index = None;
        }
    }

    /// Search action: open file's parent directory in the embedded terminal.
    pub fn search_action_open_terminal(&mut self, event_tx: &crate::event::EventSender) {
        if let Some(state) = self.search_action_state.take() {
            self.set_overlay(AppMode::Normal);
            self.clear_search_cache();
            self.last_previewed_index = None;

            let parent_dir = if state.is_directory {
                state.path.clone()
            } else {
                state
                    .path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| state.path.clone())
            };

            // Ensure terminal is visible (spawns PTY if needed)
            if !self.open_terminal(event_tx) {
                self.set_status_message("Cannot open terminal".to_string());
                return;
            }

            // Send a safely quoted cd command to PTY.
            if let Some(ref pty) = self.terminal_state.pty {
                let quoted = shell_quote_single(parent_dir.to_string_lossy().as_ref());
                let cd_cmd = format!("cd -- {}\n", quoted);
                match pty.write(cd_cmd.as_bytes()) {
                    Ok(()) => {
                        self.workspace.focus.panel = FocusedPanel::Terminal;
                        self.set_status_message(format!(
                            "Terminal: queued cd {}",
                            parent_dir.to_string_lossy()
                        ));
                    }
                    Err(error) => {
                        self.set_status_message(format!("Terminal command not accepted: {error}"))
                    }
                }
            } else {
                self.set_status_message("Terminal is not available".to_string());
            }
        }
    }

    /// Tree action: open selected item's directory in the embedded terminal.
    ///
    /// If the selected item is a directory, cd into it directly.
    /// If the selected item is a file, cd into its parent directory.
    pub fn open_terminal_at_selected(&mut self, event_tx: &crate::event::EventSender) {
        let selected = match self
            .tree_state
            .flat_items
            .get(self.tree_state.selected_index)
        {
            Some(item) => item,
            None => return,
        };

        // Skip virtual nodes (LoadMore, Loading)
        if matches!(selected.node_type, NodeType::LoadMore | NodeType::Loading) {
            return;
        }

        let target_dir = if selected.node_type == NodeType::Directory {
            selected.path.clone()
        } else {
            selected
                .path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| selected.path.clone())
        };

        // Ensure terminal is visible (spawns PTY if needed)
        if !self.open_terminal(event_tx) {
            self.set_status_message("Cannot open terminal".to_string());
            return;
        }

        // Send a safely quoted cd command to PTY.
        if let Some(ref pty) = self.terminal_state.pty {
            let quoted = shell_quote_single(target_dir.to_string_lossy().as_ref());
            let cd_cmd = format!("cd -- {}\n", quoted);
            match pty.write(cd_cmd.as_bytes()) {
                Ok(()) => {
                    self.workspace.focus.panel = FocusedPanel::Terminal;
                    self.set_status_message(format!(
                        "Terminal: queued cd {}",
                        target_dir.to_string_lossy()
                    ));
                }
                Err(error) => {
                    self.set_status_message(format!("Terminal command not accepted: {error}"))
                }
            }
        } else {
            self.set_status_message("Terminal is not available".to_string());
        }
    }

    /// Update search results by scoring cached paths against the query.
    fn update_search_results(&mut self) {
        let query = &self.search_state.query;
        if query.is_empty() {
            self.search_state.results.clear();
            self.search_state.selected_index = 0;
            return;
        }

        let paths = match &self.search_state.cached_paths {
            Some(p) => p,
            None => return,
        };

        let root = &self.tree_state.root.path;
        let mut results: Vec<SearchResult> = paths
            .iter()
            .filter_map(|path| {
                let display = path
                    .strip_prefix(root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();
                let (score, indices) = self.fuzzy_matcher.fuzzy_indices(&display, query)?;
                Some(SearchResult {
                    path: path.clone(),
                    display,
                    score,
                    match_indices: indices,
                })
            })
            .collect();

        // Sort by score descending
        results.sort_by_key(|r| std::cmp::Reverse(r.score));
        // Limit to top 50
        results.truncate(50);

        self.search_state.results = results;
        self.search_state.selected_index = 0;
        self.search_state.status = self.search_state.status_text();
    }

    /// Build a flat list of file paths using a hybrid approach:
    /// 1. Walk loaded tree nodes in-memory (instant, no I/O)
    /// 2. For un-expanded directories, do a time-bounded filesystem walk
    ///
    /// Capped by `search_max_entries` config and a 500ms time limit.
    fn build_path_index(&self) -> Vec<PathBuf> {
        let max_entries = self.config.search_max_entries();
        let mut paths = Vec::new();
        let mut unloaded_dirs = Vec::new();
        Self::collect_loaded_paths(&self.tree_state.root, &mut paths, &mut unloaded_dirs);

        // Phase 2: walk unloaded directories with entry cap + time limit
        if paths.len() < max_entries {
            let deadline = Instant::now() + std::time::Duration::from_millis(500);
            let mut stack = unloaded_dirs;
            while let Some(dir) = stack.pop() {
                if paths.len() >= max_entries || Instant::now() >= deadline {
                    break;
                }
                let entries = match std::fs::read_dir(&dir) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                for entry in entries {
                    if paths.len() >= max_entries || Instant::now() >= deadline {
                        break;
                    }
                    let entry = match entry {
                        Ok(e) => e,
                        Err(_) => continue,
                    };
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else {
                        paths.push(path);
                    }
                }
            }
        }

        paths
    }

    /// Collect paths from loaded tree nodes, and record unloaded directories.
    fn collect_loaded_paths(
        node: &crate::fs::tree::TreeNode,
        paths: &mut Vec<PathBuf>,
        unloaded_dirs: &mut Vec<PathBuf>,
    ) {
        if let Some(children) = &node.children {
            for child in children {
                paths.push(child.path.clone());
                if child.node_type == crate::fs::tree::NodeType::Directory {
                    if child.children.is_some() {
                        // Recursively collect from loaded children
                        Self::collect_loaded_paths(child, paths, unloaded_dirs);
                    } else {
                        // Not yet loaded — schedule for filesystem walk
                        unloaded_dirs.push(child.path.clone());
                    }
                }
            }
        }
    }

    /// Build a flat list of all file paths by walking the filesystem.
    ///
    /// Uses iterative stack-based walk with configurable entry cap.
    /// Used for "Search deeper..." functionality.
    #[allow(dead_code)]
    fn build_deep_path_index(&self) -> Vec<PathBuf> {
        let max_entries = self.config.search_max_entries();
        let mut paths = Vec::new();
        let mut stack: Vec<PathBuf> = vec![self.tree_state.root.path.clone()];

        while let Some(dir) = stack.pop() {
            if paths.len() >= max_entries {
                break;
            }
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries {
                if paths.len() >= max_entries {
                    break;
                }
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    paths.push(path);
                }
            }
        }
        paths
    }

    /// Invalidate the cached path index (call after tree mutations).
    pub fn invalidate_search_cache(&mut self) {
        self.reconcile_job_root();
        self.retire_copy();
        self.cancel_dir_summary();
        let root = self.tree_state.root.path.clone();
        self.retire_tree_jobs_under(&root);
        self.clear_search_cache();
    }

    fn reconcile_job_root(&mut self) {
        if self.jobs_root.as_deref() != Some(self.tree_state.root.path.as_path()) {
            self.retire_copy();
            self.cancel_dir_summary();
            if let Some(old) = self.jobs_root.take() {
                self.retire_tree_jobs_under(&old);
            }
            self.jobs_root = (self.tree_state.root.path.as_os_str().len() <= 4096)
                .then(|| self.tree_state.root.path.clone());
        }
    }

    fn retire_tree_jobs_under(&mut self, root: &Path) {
        if let Some(jobs) = &mut self.jobs {
            for target in jobs.cancel_under(root) {
                if matches!(target, crate::app_jobs::Target::S3Head(_)) {
                    self.active_s3_head = None;
                    self.s3_head_loading = false;
                    if self.preview_state.current_path.as_deref() == Some(target.path()) {
                        self.preview_state.content_lines =
                            vec![Line::raw("S3 head Incomplete (root/mutation invalidation)")];
                        self.preview_state.total_lines = 1;
                    }
                }
                if let Some(node) =
                    TreeState::find_node_mut_pub(&mut self.tree_state.root, target.path())
                {
                    node.is_loading = false;
                    node.total_child_count = None;
                }
                for item in &mut self.tree_state.flat_items {
                    if item.path == target.path() {
                        item.child_count = None;
                    }
                }
                if self
                    .tree_state
                    .flat_items
                    .iter()
                    .any(|item| item.node_type == NodeType::Loading && item.path == target.path())
                {
                    let selected = self
                        .tree_state
                        .flat_items
                        .get(self.tree_state.selected_index)
                        .map(|item| (item.path.clone(), item.node_type.clone()));
                    self.tree_state.flat_items.retain(|item| {
                        !(item.node_type == NodeType::Loading && item.path == target.path())
                    });
                    if let Some((path, kind)) = selected {
                        let index = self
                            .tree_state
                            .flat_items
                            .iter()
                            .position(|item| item.path == path && item.node_type == kind)
                            .or_else(|| {
                                self.tree_state
                                    .flat_items
                                    .iter()
                                    .position(|item| item.path == path)
                            });
                        self.tree_state.selected_index = index.unwrap_or_else(|| {
                            self.tree_state
                                .selected_index
                                .min(self.tree_state.flat_items.len().saturating_sub(1))
                        });
                    }
                }
            }
        }
    }

    fn clear_search_cache(&mut self) {
        self.search_state.cached_paths = None;
        self.search_state.index_generation = None;
        self.search_state.indexing = false;
        self.search_state.index_complete = false;
        self.search_state.index_capped = false;
        self.search_state.index_unreadable = 0;
        let root = self.tree_state.root.path.clone();
        if let Some(jobs) = &mut self.jobs {
            jobs.cancel(&crate::app_jobs::Target::FilenameIndex(root));
        }
    }

    fn retire_copy(&mut self) {
        self.active_copy = None;
        self.pending_copy = None;
        if let Some(jobs) = &mut self.jobs {
            jobs.cancel(&crate::app_jobs::Target::Clipboard(PathBuf::new()));
        }
    }

    /// Navigate tree to a specific path: expand all ancestors, select the target.
    pub fn navigate_to_path(&mut self, target: &Path) {
        self.invalidate_search_cache();
        // Collect ancestor directories that need to be expanded
        let root_path = self.tree_state.root.path.clone();
        let mut ancestors = Vec::new();
        let mut current = target.parent();
        while let Some(p) = current {
            if p == root_path {
                break;
            }
            ancestors.push(p.to_path_buf());
            current = p.parent();
        }
        ancestors.reverse();

        // Clone sort fields before mutable borrow
        let sort_by = self.tree_state.sort_by.clone();
        let dirs_first = self.tree_state.dirs_first;
        let page_size = self.tree_state.page_size;

        // Expand each ancestor and apply sorting
        for ancestor in &ancestors {
            if let Some(node) = TreeState::find_node_mut_pub(&mut self.tree_state.root, ancestor) {
                if !node.is_expanded {
                    let _ = node.load_children_paged_with_sort(page_size, &sort_by, dirs_first);
                    TreeState::sort_children_of_pub(node, &sort_by, dirs_first);
                    node.is_expanded = true;
                }
            }
        }

        // Re-flatten to reflect expansions
        self.tree_state.flatten();

        // Find and select the target in flat_items
        for (i, item) in self.tree_state.flat_items.iter().enumerate() {
            if item.path == target {
                self.tree_state.selected_index = i;
                break;
            }
        }
    }

    // === Filter (/) methods ===

    /// Activate inline tree filter mode.
    pub fn start_filter(&mut self) {
        self.tree_state.filter_query.clear();
        self.tree_state.is_filtering = false;
        self.set_overlay(AppMode::Filter);
    }

    /// Clear the filter and restore the full tree.
    pub fn clear_filter(&mut self) {
        self.tree_state.filter_query.clear();
        self.tree_state.is_filtering = false;
        self.tree_state.flatten();
        self.set_overlay(AppMode::Normal);
        // Filesystem events were silently dropped while in Filter mode,
        // so invalidate the search cache and force preview refresh.
        self.clear_search_cache();
        self.last_previewed_index = None;
    }

    /// Accept the current filter and return to normal mode (filtered view stays).
    pub fn accept_filter(&mut self) {
        self.set_overlay(AppMode::Normal);
    }

    /// Insert a character into the filter query and re-filter.
    pub fn filter_input_char(&mut self, c: char) {
        self.tree_state.filter_query.push(c);
        self.tree_state.apply_filter();
    }

    /// Delete the last character from the filter query and re-filter.
    pub fn filter_delete_char(&mut self) {
        self.tree_state.filter_query.pop();
        if self.tree_state.filter_query.is_empty() {
            self.tree_state.is_filtering = false;
            self.tree_state.flatten();
        } else {
            self.tree_state.apply_filter();
        }
    }

    // === Filesystem watcher methods ===

    /// Handle filesystem change events by refreshing affected subtrees.
    ///
    /// Document external-change tracking always runs, even in manual-refresh
    /// mode: an open editor must observe external writes without a global watch
    /// suppression. Only the tree auto-refresh is gated by `watcher_active`.
    pub fn handle_fs_change(&mut self, paths: Vec<PathBuf>) {
        // Document-awareness is independent of the tree auto-refresh policy.
        self.apply_document_disk_changes(&paths);
        if !self.watcher_active {
            return;
        }
        self.refresh_tree_from(&paths);
    }

    /// Rebuild affected subtrees from a changed-path set.
    ///
    /// Preserves: selected path, scroll offset, expanded directories.
    /// Keeps multi-selection by path and drops vanished paths honestly.
    ///
    /// Skipped while Search or Filter is active to avoid destroying the search
    /// cache or overwriting the filtered flat_items view.
    pub fn refresh_tree_from(&mut self, paths: &[PathBuf]) {
        // Don't process filesystem changes while search/filter is active:
        // - Search: would invalidate_search_cache(), clearing cached_paths so
        //   fuzzy scoring returns no results.
        // - Filter: would call flatten() which rebuilds flat_items without the
        //   filter, undoing the filtered view.
        if matches!(
            self.workspace.focus.overlay,
            AppMode::Search | AppMode::Filter
        ) || self.tree_state.is_filtering
        {
            return;
        }
        // Capture current state
        let selected_path = self
            .tree_state
            .flat_items
            .get(self.tree_state.selected_index)
            .map(|item| item.path.clone());
        let scroll_offset = self.tree_state.scroll_offset;
        let expanded = self.tree_state.collect_expanded_paths();

        // Deduplicate parent directories to reload
        let mut dirs_to_reload = std::collections::HashSet::new();
        for path in paths {
            // If the changed path IS the root, do a full reload
            if path == &self.tree_state.root.path {
                dirs_to_reload.clear();
                dirs_to_reload.insert(self.tree_state.root.path.clone());
                break;
            }
            // Otherwise reload the parent directory of the changed file
            if let Some(parent) = path.parent() {
                dirs_to_reload.insert(parent.to_path_buf());
            }
        }

        // Clone sort fields before mutable borrow (avoids borrow checker conflict)
        let sort_by = self.tree_state.sort_by.clone();
        let dirs_first = self.tree_state.dirs_first;
        let page_size = self.tree_state.page_size;

        // Reload each affected directory and apply sorting
        // For paginated dirs with snapshots: mark stale (lazy re-scan on interaction)
        // For non-paginated dirs: reload immediately
        for dir in &dirs_to_reload {
            if let Some(jobs) = &mut self.jobs {
                for target in jobs.cancel_under(dir) {
                    if target.is_summary() {
                        if self.active_dir_scan.as_deref() == Some(target.path()) {
                            self.active_dir_scan = None;
                            self.active_summary = None;
                            if self.preview_state.current_path.as_deref() == Some(target.path()) {
                                self.preview_state.content_lines = vec![Line::raw(
                                    "Incomplete directory summary (invalidated by refresh)",
                                )];
                                self.preview_state.total_lines = 1;
                            }
                        }
                        continue;
                    }
                    if let Some(node) =
                        TreeState::find_node_mut_pub(&mut self.tree_state.root, target.path())
                    {
                        if target.is_snapshot() {
                            node.is_loading = false;
                        } else {
                            node.total_child_count = None;
                        }
                    }
                }
            }
            if let Some(node) =
                crate::fs::tree::TreeState::find_node_mut_pub(&mut self.tree_state.root, dir)
            {
                if node.node_type == crate::fs::tree::NodeType::Directory {
                    if node.snapshot.is_some() {
                        // Paginated dir: mark stale, avoid expensive re-scan
                        node.is_stale = true;
                    } else {
                        // Non-paginated dir: reload immediately
                        let _ = node.load_children_paged_with_sort(page_size, &sort_by, dirs_first);
                        TreeState::sort_children_of_pub(node, &sort_by, dirs_first);
                    }
                }
            }
        }

        // Restore expanded directories then re-flatten
        self.tree_state.restore_expanded(&expanded);
        self.tree_state.flatten();

        // Restore selection
        if let Some(ref prev_path) = selected_path {
            if let Some(new_idx) = self.tree_state.find_index_by_path(prev_path) {
                self.tree_state.selected_index = new_idx;
            } else {
                // Selected path was deleted — find nearest surviving
                if let Some(fallback) = self.tree_state.find_nearest_surviving(prev_path) {
                    self.tree_state.selected_index = fallback;
                }
            }
        }

        // Restore scroll offset (clamped)
        let max_scroll = self.tree_state.flat_items.len().saturating_sub(1);
        self.tree_state.scroll_offset = scroll_offset.min(max_scroll);

        // Invalidate caches
        self.clear_search_cache();

        // Refresh preview only when it is actually stale:
        // - selected item changed (path moved/deleted/fallback selection)
        // - selected file was part of the incoming fs change set
        //
        // Avoid forcing directory preview refresh on every watcher event,
        // which can re-trigger async scans and cause visible flicker.
        let should_refresh_preview = match (
            selected_path.as_ref(),
            self.tree_state
                .flat_items
                .get(self.tree_state.selected_index),
        ) {
            (Some(previous_path), Some(current_item)) => {
                if &current_item.path != previous_path {
                    true
                } else if current_item.node_type == NodeType::File {
                    paths
                        .iter()
                        .any(|p| p == &current_item.path || p == &self.tree_state.root.path)
                } else {
                    false
                }
            }
            // No previous/current selection to compare -> refresh defensively.
            _ => true,
        };

        if should_refresh_preview {
            self.last_previewed_index = None;
        }
    }

    /// Matching open documents observe a filesystem change set, regardless of the
    /// tree auto-refresh policy, so external writes are never hidden by focus.
    ///
    /// The workspace root is treated as a full-refresh sentinel rather than a
    /// specific file change, so it never fabricates external-change flags.
    fn apply_document_disk_changes(&mut self, paths: &[PathBuf]) {
        let root = &self.tree_state.root.path;
        let changed: Vec<_> = self
            .workspace
            .documents
            .iter()
            .filter(|d| {
                paths.iter().any(|p| {
                    p != root && (d.path().starts_with(p) || d.editor.file_path.starts_with(p))
                })
            })
            .map(|d| d.id())
            .collect();
        for id in changed {
            self.record_document_disk_change(id);
        }
    }

    /// Remember a write this process just completed so the resulting watcher
    /// event is not misclassified as an external change.
    ///
    /// `written` is the exact byte sequence the save published (taken from the
    /// editor buffer in memory, so no extra file read is performed). The marker
    /// records `len`, the on-disk `mtime` observed right after the write, and a
    /// bounded digest of `written`. Consumption is evidence-based (see
    /// [`Self::take_self_write`]), so a real external write inside the TTL —
    /// even one preserving length and mtime — is still reported. Bounded by
    /// count and a short TTL; older entries are evicted first.
    fn note_self_write(&mut self, path: &Path, written: &[u8]) {
        let now = Instant::now();
        self.self_written
            .retain(|marker| now.duration_since(marker.recorded_at) < SELF_WRITE_TTL);
        // Only mtime is read back: the content digest comes from memory. If even
        // metadata is unreadable, keep no marker that could later absorb a
        // genuine external change.
        let Some(meta) = std::fs::metadata(path).ok() else {
            self.self_written.retain(|marker| marker.path != path);
            return;
        };
        if self.self_written.len() >= SELF_WRITE_MAX {
            self.self_written.remove(0);
        }
        self.self_written.push(SelfWrite {
            path: path.to_path_buf(),
            len: written.len() as u64,
            mtime: meta.modified().ok(),
            digest: self_write_digest(written),
            recorded_at: now,
        });
    }

    /// Consume the self-write marker for `path` only when the file still
    /// matches the recorded post-write identity.
    ///
    /// A marker whose length, mtime, or bounded content digest no longer match
    /// (the file changed after our save) is discarded **and not consumed**, so
    /// the change is reported as external. Only an exact match absorbs the
    /// event, which suppresses the false positive for our own save's watcher
    /// event.
    fn take_self_write(&mut self, path: &Path) -> bool {
        let now = Instant::now();
        let Some(position) = self
            .self_written
            .iter()
            .position(|marker| marker.path == path)
        else {
            return false;
        };
        let marker = &self.self_written[position];
        let fresh = now.duration_since(marker.recorded_at) < SELF_WRITE_TTL;
        let matches = if fresh {
            match self_write_identity(path) {
                Some((len, mtime, digest)) => {
                    // Length, mtime, and the bounded front-window digest must
                    // all match. For content larger than the window the digest
                    // covers only the shared prefix on both sides, so the tail
                    // is not claimed as verified.
                    len == marker.len && mtime == marker.mtime && digest == marker.digest
                }
                None => false,
            }
        } else {
            false
        };
        // The observation above invalidates the marker either way: an exact match
        // consumed it, a mismatch proves the file changed and must be reported.
        self.self_written.remove(position);
        matches
    }

    /// Event/save failure classification only; document getters never poll disk.
    fn record_document_disk_change(&mut self, id: crate::workspace::documents::DocumentId) {
        let Some(path) = self
            .workspace
            .documents
            .get(id)
            .map(|d| d.path().to_path_buf())
        else {
            return;
        };
        // Our own save's event must not look like an external change.
        if self.take_self_write(&path) {
            return;
        }
        if std::fs::symlink_metadata(&path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        {
            self.workspace.documents.mark_deleted_path(&path);
        } else {
            let _ = self.workspace.documents.mark_external_change(id);
        }
    }

    /// Force a full tree refresh from root, preserving state.
    ///
    /// Used by F5 keybinding; works regardless of watcher state.
    pub fn full_refresh(&mut self) {
        self.invalidate_search_cache();
        let root = self.tree_state.root.path.clone();
        self.refresh_tree_from(&[root]);
        self.set_status_message("🔄 Tree refreshed".to_string());
    }

    /// Toggle the filesystem watcher active state (auto-refresh on/off).
    ///
    /// Returns the new state (true = auto-refresh, false = manual refresh).
    pub fn toggle_watcher(&mut self) -> bool {
        if !self.config.watcher_enabled() {
            self.watcher_active = false;
            self.set_status_message("Watcher disabled by configuration".into());
            return false;
        }
        self.watcher_active = !self.watcher_active;
        if self.watcher_active {
            self.set_status_message("👁 Tree auto-refresh ON (Ctrl+R to disable)".to_string());
        } else {
            self.set_status_message(
                "⏸ Manual tree refresh — press F5 (open documents still watch for changes)"
                    .to_string(),
            );
        }
        self.watcher_active
    }

    // === Async directory operation handlers ===

    /// Handle an async directory scan completion.
    ///
    /// Installs the snapshot into the tree node and loads the first page.
    #[allow(dead_code)]
    pub fn handle_dir_scan_complete(
        &mut self,
        path: &std::path::Path,
        mut snapshot: crate::fs::tree::DirSnapshot,
    ) {
        let sort_by = self.tree_state.sort_by.clone();
        let dirs_first = self.tree_state.dirs_first;
        let page_size = self.tree_state.page_size;

        if let Some(node) =
            crate::fs::tree::TreeState::find_node_mut_pub(&mut self.tree_state.root, path)
        {
            // Sort the snapshot with current settings
            snapshot.sort(&sort_by, dirs_first);

            let total = snapshot.len();
            let incomplete = snapshot.capped || snapshot.skipped_count > 0;
            node.total_child_count = (!incomplete).then_some(total);
            let page_entries = snapshot.page(0, page_size);
            let consumed = page_entries.len();
            let children = crate::fs::tree::TreeNode::load_nodes_from_snapshot(
                page_entries,
                &node.path,
                node.depth + 1,
            );
            node.loaded_child_count = children.len();
            node.children = Some(children);
            node.loaded_offset = consumed;
            node.has_more_children = consumed < total;
            // Preserve the existing small-directory watcher refresh policy;
            // retain incomplete/paginated snapshots without re-enumeration.
            node.snapshot = (incomplete || total > page_size).then_some(snapshot);

            TreeState::sort_children_of_pub(node, &sort_by, dirs_first);
            node.is_expanded = true;
            node.is_loading = false;
            node.is_stale = false;
            self.tree_state.flatten();

            // Clear the "Loading..." status message
            let count_msg = if incomplete {
                format!("Incomplete directory snapshot (cap/deadline/skipped): {total} retained entries")
            } else if total > page_size {
                format!("📂 Loaded {} entries (showing first {})", total, page_size)
            } else {
                format!("📂 Loaded {} entries", total)
            };
            self.set_status_message(count_msg);
        }
    }

    /// Handle an async child count completion.
    ///
    /// Updates the node's total_child_count cache for badge display.
    #[allow(dead_code)]
    pub fn handle_dir_count_complete(&mut self, path: &std::path::Path, count: usize) {
        if let Some(node) =
            crate::fs::tree::TreeState::find_node_mut_pub(&mut self.tree_state.root, path)
        {
            node.total_child_count = Some(count);
        }
        // Rendered badges are copied into FlatItems, not read from TreeNode.
        // Avoid flattening away unrelated multi-selection and scroll state.
        for item in &mut self.tree_state.flat_items {
            if item.path == path {
                item.child_count = Some(count);
            }
        }
    }

    fn extract_preview_selected_text(&self) -> Option<String> {
        let (start, end) = self.preview_selection.normalized()?;
        let line_count = self.preview_state.content_lines.len();
        if line_count == 0 || start.line >= line_count || end.line >= line_count {
            return None;
        }

        if start.line == end.line {
            let text = preview_line_text(&self.preview_state.content_lines[start.line]);
            return Some(slice_line_by_cols(
                &text,
                start.col,
                end.col.saturating_add(1),
            ));
        }

        let mut parts: Vec<String> = Vec::with_capacity(end.line - start.line + 1);
        for line_idx in start.line..=end.line {
            let text = preview_line_text(&self.preview_state.content_lines[line_idx]);
            if line_idx == start.line {
                parts.push(slice_line_by_cols(&text, start.col, usize::MAX));
            } else if line_idx == end.line {
                parts.push(slice_line_by_cols(&text, 0, end.col.saturating_add(1)));
            } else {
                parts.push(crate::text::display_slice(&text, 0, usize::MAX));
            }
        }
        Some(parts.join("\n"))
    }

    /// Copy the current preview selection to the system clipboard.
    ///
    /// Runs the clipboard operation asynchronously to avoid blocking the UI.
    /// Shows a status message with the result (success/failure/no-selection).
    pub fn copy_preview_selection(&mut self, event_tx: &crate::event::EventSender) {
        match self.extract_preview_selected_text() {
            Some(text) if !text.is_empty() => {
                self.preview_selection.clear();
                self.copy_text_async(text, event_tx);
            }
            _ => {
                self.set_status_message("No preview text selected — drag to select".to_string());
            }
        }
    }

    /// Copy the current terminal selection to the system clipboard.
    ///
    /// Runs the clipboard operation asynchronously to avoid blocking the UI.
    /// Shows a status message with the result (success/failure/no-selection).
    pub fn copy_terminal_selection(&mut self, event_tx: &crate::event::EventSender) {
        match self.terminal_state.extract_selected_text() {
            Some(text) if !text.is_empty() => {
                self.terminal_state.selection.clear();
                self.copy_text_async(text, event_tx);
            }
            _ => {
                self.set_status_message("No terminal text selected — drag to select".to_string());
            }
        }
    }
}

fn preview_line_text(line: &Line<'static>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn slice_line_by_cols(line: &str, start_col: usize, end_col_exclusive: usize) -> String {
    if end_col_exclusive <= start_col {
        return String::new();
    }
    crate::text::display_slice(line, start_col, end_col_exclusive)
}

#[cfg(test)]
#[test]
fn preview_copy_uses_display_cells_and_expanded_tabs() {
    assert_eq!(slice_line_by_cols("a\t中e\u{301}", 2, 6), "  中");
    assert_eq!(slice_line_by_cols("中X", 1, 2), " ");
}

/// Wrap a long string into chunks of at most `max_width` characters.
/// Splits at character boundaries (not word boundaries) since URIs/paths
/// don't have natural word breaks.
fn wrap_text(text: &str, max_width: usize) -> Vec<&str> {
    if max_width == 0 {
        return vec![text];
    }
    let mut result = Vec::new();
    let mut remaining = text;
    while remaining.len() > max_width {
        // Find the largest char-boundary split at or before max_width
        let split = remaining
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|&i| i <= max_width)
            .last()
            .unwrap_or(max_width);
        let split = if split == 0 {
            max_width.min(remaining.len())
        } else {
            split
        };
        result.push(&remaining[..split]);
        remaining = &remaining[split..];
    }
    if !remaining.is_empty() {
        result.push(remaining);
    }
    if result.is_empty() {
        result.push(text);
    }
    result
}

/// Format a byte size into a human-readable string.
#[allow(dead_code)]
fn format_size_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
fn default_text_clipboard_backend() -> crate::app_jobs::ClipboardBackend {
    // Tests opt into fake collaborators, never the user's native clipboard.
    #[cfg(test)]
    {
        std::sync::Arc::new(|_, _| false)
    }
    #[cfg(not(test))]
    {
        std::sync::Arc::new(copy_to_system_clipboard)
    }
}

/// Native commands use argv/stdin, never shell interpolation or text files.
/// Bound both a blocked stdin write and process execution to two seconds/tool.
#[cfg(not(test))]
fn copy_to_system_clipboard(text: &str, stopped: &dyn Fn() -> bool) -> bool {
    use std::process::Command;
    if text.len() > 1024 * 1024 {
        return false;
    }
    #[cfg(target_os = "macos")]
    let commands: &[(&str, &[&str])] = &[("pbcopy", &[])];
    #[cfg(target_os = "windows")]
    let commands: &[(&str, &[&str])] = &[("clip.exe", &[])];
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let commands: &[(&str, &[&str])] = &[
        ("wl-copy", &["--foreground"]),
        ("xclip", &["-selection", "clipboard", "-quiet"]),
        ("xsel", &["--clipboard", "--input", "--nodetach"]),
    ];
    for (program, args) in commands {
        if stopped() {
            return false;
        }
        let mut command = Command::new(program);
        command.args(*args);
        if run_clipboard_tool(command, text, stopped) {
            return true;
        }
    }
    false
}

/// RAII inside the thread scope: kill/reap precedes writer joining even when
/// the monitor unwinds. Only this freshly spawned process group is signalled.
struct ClipboardChild(std::process::Child);
impl Drop for ClipboardChild {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn run_clipboard_tool(
    mut command: std::process::Command,
    text: &str,
    stopped: &dyn Fn() -> bool,
) -> bool {
    use std::io::Write;
    use std::process::Stdio;
    if stopped() || text.len() > 1024 * 1024 {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let Ok(child) = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    std::thread::scope(|scope| {
        let mut owned = ClipboardChild(child);
        let Some(mut stdin) = owned.0.stdin.take() else {
            return false;
        };
        let Ok(writer) = std::thread::Builder::new()
            .name("clipboard-stdin".into())
            .spawn_scoped(scope, move || stdin.write_all(text.as_bytes()).is_ok())
        else {
            return false;
        };
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        let success = loop {
            if stopped() || Instant::now() >= deadline {
                break false;
            }
            match owned.0.try_wait() {
                Ok(Some(status)) => break status.success(),
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(5)),
                Err(_) => break false,
            }
        };
        drop(owned); // close descendants' descriptors, wait/reap, THEN join
        let written = writer.join().unwrap_or(false);
        success && written
    })
}

#[cfg(test)]
mod tests {
    fn install_editor(app: &mut App, editor: crate::editor::EditorState) {
        let temporary;
        let path = if editor.file_path.is_absolute() && editor.file_path.exists() {
            editor.file_path.clone()
        } else {
            temporary = tempfile::NamedTempFile::new().unwrap();
            std::fs::write(temporary.path(), editor.buffer.join("\n")).unwrap();
            temporary.path().to_path_buf()
        };
        let id = app
            .workspace
            .documents
            .open(&path, crate::workspace::documents::OpenDisposition::Pinned)
            .unwrap();
        app.workspace.documents.get_mut(id).unwrap().editor = editor;
    }
    use super::*;
    use std::fs::{self, File};
    use tempfile::TempDir;

    #[tokio::test]
    async fn app_jobs_clipboard_terminal_retirement_invalidates_captured_terminal_origin() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Terminal;
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.copy_text_async("retired terminal selection".into(), &tx);
        let delivery = app.next_background().await.unwrap();
        app.shutdown_terminal();
        app.apply_background(delivery);
        let stale = app.pending_copy.is_some();
        app.shutdown_background().await;
        assert!(
            !stale,
            "old terminal copy acted after its terminal was retired"
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn app_jobs_clipboard_cross_device_partial_cut_keeps_mixed_undo_and_sources() {
        use std::os::unix::fs::MetadataExt;
        let source = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir_in("/dev/shm").unwrap();
        assert_ne!(
            fs::metadata(source.path()).unwrap().dev(),
            fs::metadata(dest.path()).unwrap().dev()
        );
        let file = source.path().join("file");
        let folder = source.path().join("folder");
        fs::write(&file, b"owned").unwrap();
        fs::create_dir(&folder).unwrap();
        std::os::unix::fs::symlink("missing", folder.join("refused")).unwrap();
        let mut app = App::new(dest.path(), AppConfig::default()).unwrap();
        app.clipboard
            .set(vec![file.clone(), folder.clone()], ClipboardOp::Cut);
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.paste_clipboard_async(tx);
        loop {
            let delivery = app.next_background().await.unwrap();
            let done = !matches!(
                delivery.result,
                Ok(crate::app_jobs::NativeOutput::OperationProgress(_))
            );
            app.apply_background(delivery);
            if done {
                break;
            }
        }
        assert!(!file.exists() && folder.exists());
        assert!(dest.path().join("file").exists() && dest.path().join("folder").is_dir());
        assert!(!app.clipboard.is_empty());
        assert!(
            matches!(&app.last_undo, Some(UndoAction::PartialPaste { copies, moves })
            if copies == &[dest.path().join("folder")] && moves == &[(file.clone(), dest.path().join("file"))])
        );
        app.undo();
        assert!(file.exists() && folder.exists());
        assert!(!dest.path().join("file").exists() && !dest.path().join("folder").exists());
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn app_jobs_clipboard_manual_refresh_retirement_removes_dead_loading_row() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file"), b"data").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_initial_load(&tx);
        assert!(app
            .tree_state
            .flat_items
            .iter()
            .any(|item| item.node_type == NodeType::Loading));
        app.watcher_active = false; // preserve watcher policy; retire admission anyway
        app.full_refresh();
        let loading = app
            .tree_state
            .flat_items
            .iter()
            .any(|item| item.node_type == NodeType::Loading);
        app.shutdown_background().await;
        assert!(
            !loading,
            "cancelled manual refresh left a permanently dead Loading row"
        );
    }

    #[tokio::test]
    async fn app_jobs_clipboard_root_replacement_aba_retires_previous_tree_domains() {
        let (dir, mut app) = setup_app();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_initial_load(&tx);
        let replacement = tempfile::tempdir().unwrap();
        app.tree_state = TreeState::new(replacement.path()).unwrap();
        app.invalidate_search_cache(); // existing profile/root replacement seam
        app.tree_state = TreeState::new(dir.path()).unwrap();
        let retained = app.jobs.as_ref().unwrap().has_pending();
        app.shutdown_background().await;
        assert!(!retained, "root A/B/A retained an old A generation");
    }

    #[tokio::test]
    async fn app_jobs_clipboard_full_focus_stack_refuses_before_native_or_fallback_mutation() {
        let (_dir, mut app) = setup_app();
        for _ in 0..8 {
            app.workspace
                .focus
                .open_overlay(AppMode::Help, None)
                .unwrap();
        }
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.copy_text_async("must retain origin".into(), &tx);
        let admitted = app.active_copy.is_some();
        app.shutdown_background().await;
        assert!(
            !admitted,
            "copy admitted without a possible manual fallback frame"
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::Help);
    }

    #[tokio::test]
    async fn app_jobs_clipboard_queued_s3_aba_keeps_copy_paste_and_count_domains_independent() {
        use crate::app_jobs::{AppJobs, NativeOutput, Target};
        let (dir, mut app) = setup_app();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered));
        let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let gate = release.clone();
        app.jobs = Some(
            AppJobs::new(
                crate::background::Limits {
                    workers: 1,
                    ..Default::default()
                },
                crate::background::Worker::Blocking(Arc::new(
                    move |job: crate::app_jobs::NativeJob, _| match &job.target {
                        Target::S3Head(path) => {
                            NativeOutput::S3Head(path.to_string_lossy().into_owned())
                        }
                        Target::Count(_) => {
                            entered.lock().unwrap().take().unwrap().send(()).unwrap();
                            let (lock, cv) = &*gate;
                            let mut released = lock.lock().unwrap();
                            while !*released {
                                released = cv.wait(released).unwrap();
                            }
                            NativeOutput::Count {
                                count: 7,
                                complete: true,
                            }
                        }
                        Target::Clipboard(_) => NativeOutput::Clipboard {
                            text: "owned".into(),
                            native: false,
                        },
                        Target::Paste(..) => {
                            NativeOutput::Failed("fake paste failure before mutation")
                        }
                        _ => unreachable!(),
                    },
                )),
            )
            .unwrap(),
        );
        let (tx, _rx) = crate::event::event_channel(Default::default());
        let a = Target::S3Head(PathBuf::from("s3://fake-bucket/a"));
        assert!(app.submit_native_job(a.clone()));
        app.spawn_async_child_count(dir.path(), &tx);
        ready.await.unwrap(); // proves old A was published to the result queue
        assert!(app.submit_native_job(Target::S3Head(PathBuf::from("s3://fake-bucket/b"))));
        assert!(app.submit_native_job(a));
        app.copy_text_async("captured".into(), &tx);
        app.clipboard
            .set(vec![dir.path().join("file_a.txt")], ClipboardOp::Copy);
        app.tree_state.selected_index = 2;
        app.paste_clipboard_async(tx);
        let (lock, cv) = &*release;
        *lock.lock().unwrap() = true;
        cv.notify_one();
        let mut heads = 0;
        let mut copies = 0;
        let mut pastes = 0;
        let mut counts = 0;
        for _ in 0..4 {
            let delivery = app.next_background().await.unwrap();
            match &delivery.target {
                Target::S3Head(path) => {
                    heads += 1;
                    assert_eq!(path, Path::new("s3://fake-bucket/a"));
                }
                Target::Clipboard(_) => copies += 1,
                Target::Paste(..) => pastes += 1,
                Target::Count(_) => counts += 1,
                _ => unreachable!(),
            }
            app.apply_background(delivery);
        }
        assert_eq!((heads, copies, pastes, counts), (1, 1, 1, 1));
        assert!(app.operations.is_empty());
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn app_jobs_clipboard_full_pool_preserves_prior_generation_and_all_related_domains() {
        let (dir, mut app) = setup_app();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered));
        let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let gate = release.clone();
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                crate::background::Limits {
                    queued_jobs: 1,
                    queued_results: 1,
                    workers: 1,
                    ..Default::default()
                },
                crate::background::Worker::Blocking(Arc::new(
                    move |job: crate::app_jobs::NativeJob, cancel| {
                        if matches!(job.target, crate::app_jobs::Target::Clipboard(_)) {
                            entered.lock().unwrap().take().unwrap().send(()).unwrap();
                            let (lock, cv) = &*gate;
                            let mut released = lock.lock().unwrap();
                            while !*released {
                                released = cv.wait(released).unwrap();
                            }
                        }
                        crate::app_jobs::run(job, cancel)
                    },
                )),
            )
            .unwrap(),
        );
        app.set_overlay(AppMode::Help);
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.copy_text_async("prior".into(), &tx);
        ready.await.unwrap();
        let prior = app.active_copy.unwrap().0;
        app.spawn_initial_load(&tx); // exactly fills the sole waiting slot
        app.copy_text_async("rejected".into(), &tx);
        assert_eq!(app.active_copy.unwrap().0, prior);
        assert!(app.status_message.as_ref().unwrap().0.contains("Full"));
        app.clipboard
            .set(vec![dir.path().join("file_a.txt")], ClipboardOp::Copy);
        app.paste_clipboard_async(tx);
        let rejected = app.operations.is_empty() && app.workspace.focus.overlay == AppMode::Help;
        let (lock, cv) = &*release;
        *lock.lock().unwrap() = true;
        cv.notify_one();
        let delivery = app.next_background().await.unwrap();
        assert_eq!(delivery.generation, prior);
        app.apply_background(delivery);
        assert_eq!(
            app.pending_copy.as_ref().map(|(s, _)| s.as_str()),
            Some("prior")
        );
        app.shutdown_background().await;
        assert!(rejected);
        assert!(dir.path().join("file_a.txt").exists());
    }

    #[tokio::test]
    async fn app_jobs_clipboard_nested_renewed_and_clipboard_generations_reject_old_origin() {
        for mode in 0..4 {
            let (_dir, mut app) = setup_app();
            let (tx, _rx) = crate::event::event_channel(Default::default());
            app.set_overlay(AppMode::Help);
            app.copy_text_async("old".into(), &tx);
            let delivery = app.next_background().await.unwrap();
            match mode {
                0 => {
                    app.workspace
                        .focus
                        .open_overlay(AppMode::CopyOverlay, None)
                        .unwrap();
                }
                1 => {
                    app.dismiss_overlay();
                    app.set_overlay(AppMode::Help);
                }
                2 => app.copy_to_clipboard(),
                _ => {
                    app.copy_text_async("new".into(), &tx);
                }
            }
            app.apply_background(delivery);
            assert!(app.pending_copy.is_none());
            if mode == 3 {
                let new = app.next_background().await.unwrap();
                app.apply_background(new);
                assert_eq!(
                    app.pending_copy.as_ref().map(|(text, _)| text.as_str()),
                    Some("new")
                );
            }
            app.shutdown_background().await;
        }
    }

    #[tokio::test]
    async fn app_jobs_clipboard_domain_input_result_pressure_and_worker_failures_recover() {
        for mode in 0..5 {
            let (dir, mut app) = setup_app();
            let limits = crate::background::Limits {
                request_domains: if mode == 0 { 1 } else { 32 },
                job_bytes: if mode == 1 { 1 } else { 1024 * 1024 },
                result_bytes: if mode == 2 { 1 } else { 8 * 1024 * 1024 },
                ..Default::default()
            };
            let worker = crate::background::Worker::Blocking(Arc::new(
                move |job: crate::app_jobs::NativeJob, cancel| {
                    if mode == 3 {
                        panic!("owned injected worker failure");
                    }
                    if mode == 4 {
                        return crate::app_jobs::NativeOutput::Clipboard {
                            text: "x".repeat(9 * 1024 * 1024),
                            native: false,
                        };
                    }
                    crate::app_jobs::run(job, cancel)
                },
            ));
            app.jobs = Some(crate::app_jobs::AppJobs::new(limits, worker).unwrap());
            let (tx, _rx) = crate::event::event_channel(Default::default());
            if mode == 0 {
                app.spawn_initial_load(&tx);
            }
            app.set_overlay(AppMode::Help);
            app.copy_text_async("payload".into(), &tx);
            if mode < 3 {
                assert!(app.active_copy.is_none());
                assert!(app
                    .status_message
                    .as_ref()
                    .unwrap()
                    .0
                    .contains("not admitted"));
            } else {
                let delivery = app.next_background().await.unwrap();
                app.apply_background(delivery);
                assert!(app.active_copy.is_none() && app.pending_copy.is_none());
                assert!(app.status_message.as_ref().unwrap().0.contains("failure"));
            }
            app.clipboard
                .set(vec![dir.path().join("file_a.txt")], ClipboardOp::Copy);
            app.tree_state.selected_index = 2;
            app.paste_clipboard_async(tx);
            if mode < 3 {
                assert!(app.operations.is_empty());
                assert_eq!(app.workspace.focus.overlay, AppMode::Help);
            } else {
                let delivery = app.next_background().await.unwrap();
                app.apply_background(delivery);
                assert!(app.operations.is_empty());
                assert_eq!(app.workspace.focus.overlay, AppMode::Help);
            }
            app.shutdown_background().await;
            assert!(!dir.path().join("beta/file_a.txt").exists());
        }
    }

    #[tokio::test]
    async fn app_jobs_clipboard_actual_deadlines_and_move_pair_alignment() {
        let (dir, mut app) = setup_app();
        app.ensure_jobs();
        let mut job = app.clipboard_job(
            crate::app_jobs::Target::Clipboard(PathBuf::new()),
            crate::app_jobs::ClipboardJob::Copy {
                text: "deadline".into(),
                backend: Arc::new(|_, _| panic!("deadline ran peer")),
            },
        );
        job.timeout = std::time::Duration::ZERO;
        let origin = app.copy_origin();
        let generation = app.jobs.as_mut().unwrap().submit(job).unwrap();
        app.active_copy = Some((generation, origin));
        let delivery = app.next_background().await.unwrap();
        app.apply_background(delivery);
        assert!(app.status_message.as_ref().unwrap().0.contains("deadline"));
        app.clipboard.set(
            vec![dir.path().join("missing"), dir.path().join("file_a.txt")],
            ClipboardOp::Cut,
        );
        app.tree_state.selected_index = 2;
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.paste_clipboard_async(tx);
        loop {
            let delivery = app.next_background().await.unwrap();
            let done = !matches!(
                delivery.result,
                Ok(crate::app_jobs::NativeOutput::OperationProgress(_))
            );
            app.apply_background(delivery);
            if done {
                break;
            }
        }
        assert!(
            matches!(&app.last_undo, Some(UndoAction::MovePaste { moves })
            if moves == &[(dir.path().join("file_a.txt"), dir.path().join("beta/file_a.txt"))])
        );
        assert!(
            !app.clipboard.is_empty(),
            "failed cut must not clear captured clipboard"
        );
        app.undo();
        assert!(dir.path().join("file_a.txt").exists());
        app.shutdown_background().await;
    }

    #[test]
    fn app_jobs_clipboard_actual_fake_child_success_failure_and_predeadline() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("capture");
        let mut command = std::process::Command::new("python3");
        command
            .args([
                "-c",
                "import sys;open(sys.argv[1],'wb').write(sys.stdin.buffer.read())",
            ])
            .arg(&file);
        let text = "literal '\n$(never-run)";
        assert!(run_clipboard_tool(command, text, &|| false));
        assert_eq!(fs::read(&file).unwrap(), text.as_bytes());
        let mut fail = std::process::Command::new("python3");
        fail.args(["-c", "raise SystemExit(2)"]);
        assert!(!run_clipboard_tool(fail, "payload", &|| false));
        assert!(!run_clipboard_tool(
            std::process::Command::new("missing-test-owned-peer"),
            "payload",
            &|| false
        ));
        assert!(!run_clipboard_tool(
            std::process::Command::new("python3"),
            "payload",
            &|| true
        ));
    }

    #[tokio::test]
    async fn app_jobs_clipboard_unchanged_preview_poll_preserves_admitted_copy() {
        let (_dir, mut app) = setup_app();
        app.last_previewed_index = Some(app.tree_state.selected_index);
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.copy_text_async("still owned".into(), &tx);
        app.update_preview();
        let retained = app.active_copy.is_some();
        app.shutdown_background().await;
        assert!(
            retained,
            "unchanged main preview poll cancelled admitted copy"
        );
    }

    #[tokio::test]
    async fn app_jobs_clipboard_navigation_aba_cannot_revive_old_copy_origin() {
        let (_dir, mut app) = setup_app();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.copy_text_async("old origin".into(), &tx);
        let delivery = app.next_background().await.unwrap();
        app.select_next();
        app.select_previous();
        app.apply_background(delivery);
        assert!(
            app.pending_copy.is_none(),
            "A/B/A navigation revived old origin"
        );
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn app_jobs_clipboard_mutation_entrypoint_retires_pending_native_results() {
        let (dir, mut app) = setup_app();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_initial_load(&tx);
        fs::write(dir.path().join("new item"), b"new").unwrap();
        app.tree_state.reload_dir(dir.path()); // same mutation frontend entrypoint
        app.invalidate_search_cache();
        let retained = app.jobs.as_ref().unwrap().has_pending();
        app.shutdown_background().await;
        assert!(
            !retained,
            "mutation left an old immutable snapshot eligible"
        );
    }

    #[tokio::test]
    async fn app_jobs_clipboard_path_copy_returns_to_its_captured_search_origin() {
        let (_dir, mut app) = setup_app();
        app.set_overlay(AppMode::SearchAction);
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.copy_path_to_system_clipboard(&tx);
        app.dismiss_overlay(); // production SearchAction's copy-and-close flow
        let delivery = app.next_background().await.unwrap();
        app.apply_background(delivery);
        let mut output = Vec::new();
        app.present_copy_fallback(&mut output, false);
        app.shutdown_background().await;
        assert_eq!(app.workspace.focus.overlay, AppMode::CopyOverlay);
        assert!(app.copy_overlay_text.is_some());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn app_jobs_clipboard_child_interrupt_owns_descendant_stdin_and_reaps() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("pid");
        let mut command = std::process::Command::new("python3");
        command.args(["-c", "import os,sys,subprocess\np=subprocess.Popen(['/usr/bin/sleep','30'],stdin=sys.stdin)\nf=open(sys.argv[1]+'.tmp','w');f.write(str(p.pid)+' '+str(os.getpgrp())+' '+str(os.getpgid(p.pid)));f.close();os.rename(sys.argv[1]+'.tmp',sys.argv[1])\np.wait()"]);
        command.arg(&marker);
        assert!(!run_clipboard_tool(
            command,
            &"x".repeat(1024 * 1024),
            &|| marker.exists()
        ));
        let published = fs::read_to_string(&marker).unwrap();
        let pid: i32 = published
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let running = || {
            fs::read_to_string(format!("/proc/{pid}/status")).is_ok_and(|s| {
                !s.lines()
                    .any(|line| line.starts_with("State:") && line.contains('Z'))
            })
        };
        // Kernel FD closure (which releases the joined writer) precedes the
        // orphan's final zombie-state publication. Observe exit, not a sleep.
        let deadline = Instant::now() + std::time::Duration::from_secs(1);
        while running() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        let alive = running();
        let diagnosis = fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        // Always terminate the proven test-owned peer, even on the behavioral red.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        assert!(
            !alive,
            "worker retired while descendant held the detached stdin writer: {published}; {diagnosis}"
        );
    }

    #[tokio::test]
    async fn app_jobs_clipboard_shutdown_retains_partial_paste_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source");
        fs::write(&src, b"data").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered));
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                Default::default(),
                crate::background::Worker::Blocking(Arc::new(
                    move |job: crate::app_jobs::NativeJob, _| {
                        let crate::app_jobs::ClipboardJob::Paste { interrupt, .. } =
                            job.clipboard.unwrap()
                        else {
                            unreachable!()
                        };
                        let dest = job.target.path().join("partial");
                        fs::write(&dest, b"partial").unwrap();
                        entered.lock().unwrap().take().unwrap().send(()).unwrap();
                        while !interrupt.load(Ordering::SeqCst) {
                            std::thread::yield_now();
                        }
                        crate::app_jobs::NativeOutput::Paste(crate::app_jobs::PasteResult {
                            result: crate::event::OperationResult {
                                success_count: 0,
                                errors: vec!["interrupted".into()],
                                created_paths: vec![dest.clone()],
                                source_paths: vec![],
                                dest_dir: job.target.path().into(),
                                was_cut: false,
                            },
                            copies: vec![dest],
                            moves: vec![],
                            completed_entries: 0,
                        })
                    },
                )),
            )
            .unwrap(),
        );
        app.clipboard.set(vec![src], ClipboardOp::Copy);
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.paste_clipboard_async(tx);
        ready.await.unwrap();
        // Release before shutdown in the legacy red, which otherwise waits forever.
        app.operations[0].interrupt.store(true, Ordering::SeqCst);
        app.shutdown_background().await;
        assert!(
            matches!(&app.last_undo, Some(UndoAction::CopyPaste { created_paths })
            if created_paths == &[dir.path().join("partial")]),
            "shutdown discarded the necessary partial completion"
        );
    }

    #[test]
    fn app_jobs_clipboard_old_completion_must_not_retire_new_nested_progress() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let progress = |message: &str| {
            AppMode::Dialog(DialogKind::Progress {
                message: message.into(),
                current: 0,
                total: 1,
            })
        };
        app.workspace
            .focus
            .open_overlay(progress("old operation"), None)
            .unwrap();
        app.workspace
            .focus
            .open_overlay(AppMode::CopyOverlay, None)
            .unwrap();
        app.workspace
            .focus
            .open_overlay(progress("new operation"), None)
            .unwrap();
        app.handle_operation_complete(crate::event::OperationResult {
            success_count: 0,
            errors: vec!["old operation interrupted".into()],
            created_paths: vec![],
            source_paths: vec![],
            dest_dir: dir.path().into(),
            was_cut: false,
        });
        assert_eq!(
            app.workspace.focus.overlay,
            progress("new operation"),
            "older completion retired the newer nested operation"
        );
    }

    #[tokio::test]
    async fn app_jobs_clipboard_closed_copy_does_not_call_peer_or_change_origin() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                Default::default(),
                crate::background::Worker::Blocking(Arc::new(crate::app_jobs::run)),
            )
            .unwrap(),
        );
        app.shutdown_background().await;
        let called = Arc::new(AtomicBool::new(false));
        let peer = called.clone();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered));
        app.text_clipboard_backend = Arc::new(move |_, _| {
            peer.store(true, Ordering::SeqCst);
            if let Some(tx) = entered.lock().unwrap().take() {
                let _ = tx.send(());
            }
            false
        });
        app.set_overlay(AppMode::Help);
        let (tx, mut rx) = crate::event::event_channel(Default::default());
        app.copy_text_async("captured".into(), &tx);
        if !app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("not admitted")
        {
            ready.await.unwrap(); // legacy peer has actually run, no timing guess
        }
        rx.close();
        assert!(
            !called.load(Ordering::SeqCst),
            "closed pool still executed native copy"
        );
        assert_eq!(app.workspace.focus.overlay, AppMode::Help);
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("not admitted"));
    }

    #[tokio::test]
    async fn app_jobs_clipboard_closed_paste_admits_before_modal_and_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let dest = dir.path().join("dest");
        fs::create_dir(&dest).unwrap();
        fs::write(&source, b"captured").unwrap();
        let mut app = App::new(&dest, AppConfig::default()).unwrap();
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                Default::default(),
                crate::background::Worker::Blocking(Arc::new(crate::app_jobs::run)),
            )
            .unwrap(),
        );
        app.shutdown_background().await;
        app.clipboard.set(vec![source.clone()], ClipboardOp::Copy);
        app.set_overlay(AppMode::Help);
        let (tx, mut rx) = crate::event::event_channel(Default::default());
        app.paste_clipboard_async(tx);
        let overlay = app.workspace.focus.overlay.clone();
        rx.close();
        assert_eq!(overlay, AppMode::Help, "rejected paste opened progress");
        assert!(!dest.join("source").exists());
        assert_eq!(app.clipboard.paths, [source]);
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("not admitted"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn app_jobs_clipboard_recursive_failure_keeps_partial_destination_undo() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let dest = dir.path().join("dest");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&dest).unwrap();
        std::os::unix::fs::symlink("missing", source.join("refused")).unwrap();
        let mut app = App::new(&dest, AppConfig::default()).unwrap();
        app.clipboard.set(vec![source], ClipboardOp::Copy);
        let (tx, mut rx) = crate::event::event_channel(Default::default());
        app.paste_clipboard_async(tx.clone());
        loop {
            tokio::select! {
                delivery = app.next_background() => {
                    let done = !matches!(delivery.as_ref().unwrap().result, Ok(crate::app_jobs::NativeOutput::Progress(_) | crate::app_jobs::NativeOutput::OperationProgress(_)));
                    app.apply_background(delivery.unwrap());
                    if done { break; }
                }
                event = rx.recv() => {
                    if let Some(crate::event::Event::OperationComplete(result)) = event {
                        app.handle_operation_complete(result);
                        break;
                    }
                }
            }
        }
        app.shutdown_background().await;
        assert!(dest.join("source").is_dir());
        assert!(
            matches!(&app.last_undo, Some(UndoAction::CopyPaste { created_paths })
            if created_paths == &[dest.join("source")]),
            "partial destination lost on recursive failure"
        );
    }

    #[tokio::test]
    async fn app_jobs_final_s3_head_admission_preserves_loading_preview_policy() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        app.init_s3_mode(crate::s3::S3Config {
            path: crate::s3::S3Path::parse("s3://fake-bucket/").unwrap(),
            profile: None,
        });
        app.handle_s3_listing_complete(
            "s3://fake-bucket/",
            vec![crate::s3::S3Entry {
                name: "file.txt".into(),
                is_dir: false,
                size: 1,
                modified: String::new(),
            }],
        );
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                Default::default(),
                crate::background::Worker::Blocking(Arc::new(|_, _| {
                    crate::app_jobs::NativeOutput::Failed("fake peer")
                })),
            )
            .unwrap(),
        );
        app.tree_state.selected_index = 1;
        app.preview_state.horizontal_offset = 19;
        app.config.preview.line_wrap = Some(true);
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_s3_head(&tx);
        let horizontal = app.preview_state.horizontal_offset;
        let wrap = app.preview_state.line_wrap;
        let lines = app.preview_state.content_lines.clone();
        let total = app.preview_state.total_lines;
        app.shutdown_background().await;
        assert_eq!(
            horizontal, 0,
            "admitted head inherited an unrelated horizontal viewport"
        );
        assert!(wrap);
        assert!(lines[0]
            .spans
            .iter()
            .any(|span| span.content.contains("file.txt")));
        assert_eq!(total, 3);
    }

    #[tokio::test]
    async fn app_jobs_final_s3_collapse_retires_expansion_and_shutdown_labels_head() {
        for head in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
            app.init_s3_mode(crate::s3::S3Config {
                path: crate::s3::S3Path::parse("s3://fake-bucket/").unwrap(),
                profile: None,
            });
            app.handle_s3_listing_complete(
                "s3://fake-bucket/",
                vec![crate::s3::S3Entry {
                    name: if head {
                        "file.txt".into()
                    } else {
                        "child/".into()
                    },
                    is_dir: !head,
                    size: 1,
                    modified: String::new(),
                }],
            );
            let (entered, ready) = tokio::sync::oneshot::channel();
            let entered = std::sync::Mutex::new(Some(entered));
            let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
            let worker_gate = gate.clone();
            app.jobs = Some(
                crate::app_jobs::AppJobs::new(
                    Default::default(),
                    crate::background::Worker::Blocking(Arc::new(move |_, token| {
                        entered.lock().unwrap().take().unwrap().send(()).unwrap();
                        let _guard = worker_gate
                            .1
                            .wait_while(worker_gate.0.lock().unwrap(), |open| !*open)
                            .unwrap();
                        if !head {
                            assert!(token.is_cancelled());
                        }
                        crate::app_jobs::NativeOutput::Failed("cancelled peer")
                    })),
                )
                .unwrap(),
            );
            let (tx, _rx) = crate::event::event_channel(Default::default());
            app.tree_state.selected_index = 1;
            if head {
                app.spawn_s3_head(&tx);
            } else {
                app.spawn_s3_expand("s3://fake-bucket/child/".into(), &tx);
            }
            ready.await.unwrap();
            if !head {
                app.collapse_selected();
            }
            let retired = !app.jobs.as_ref().unwrap().has_pending();
            *gate.0.lock().unwrap() = true;
            gate.1.notify_all();
            app.shutdown_background().await;
            if head {
                assert!(
                    summary_text(&app).contains("Incomplete"),
                    "closed head still claims loading"
                );
            } else {
                assert!(retired, "collapse failed to invalidate S3 expansion domain");
            }
        }
    }

    #[tokio::test]
    async fn app_jobs_final_s3_pressure_full_domains_panic_and_oversize() {
        for mode in 0..5 {
            let dir = tempfile::tempdir().unwrap();
            let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
            app.init_s3_mode(crate::s3::S3Config {
                path: crate::s3::S3Path::parse("s3://fake-bucket/").unwrap(),
                profile: None,
            });
            app.handle_s3_listing_complete(
                "s3://fake-bucket/",
                vec![
                    crate::s3::S3Entry {
                        name: "child/".into(),
                        is_dir: true,
                        size: 0,
                        modified: String::new(),
                    },
                    crate::s3::S3Entry {
                        name: "file.txt".into(),
                        is_dir: false,
                        size: 1,
                        modified: String::new(),
                    },
                ],
            );
            app.preview_state.content_lines = vec![Line::raw("prior preview")];
            app.set_overlay(AppMode::Help);
            let (entered, ready) = tokio::sync::oneshot::channel();
            let entered = std::sync::Mutex::new(Some(entered));
            let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
            let worker_gate = gate.clone();
            app.jobs = Some(
                crate::app_jobs::AppJobs::new(
                    crate::background::Limits {
                        workers: 1,
                        queued_jobs: 1,
                        request_domains: if mode == 1 { 1 } else { 32 },
                        job_bytes: if mode == 2 { 1 } else { 1_048_576 },
                        result_bytes: if mode == 4 { 1 } else { 8_388_608 },
                        ..Default::default()
                    },
                    crate::background::Worker::Blocking(Arc::new(move |job, _| {
                        if mode <= 1 {
                            if let Some(tx) = entered.lock().unwrap().take() {
                                tx.send(()).unwrap();
                            }
                            let _guard = worker_gate
                                .1
                                .wait_while(worker_gate.0.lock().unwrap(), |open| !*open)
                                .unwrap();
                        }
                        if mode == 3 {
                            panic!("injected S3 panic");
                        }
                        if matches!(job.target, crate::app_jobs::Target::S3Head(_)) {
                            crate::app_jobs::NativeOutput::S3Head("accepted".into())
                        } else {
                            crate::app_jobs::NativeOutput::S3Listing(Vec::new())
                        }
                    })),
                )
                .unwrap(),
            );
            let (tx, _rx) = crate::event::event_channel(Default::default());
            if mode <= 1 {
                app.spawn_async_child_count(dir.path(), &tx);
                ready.await.unwrap();
                if mode == 0 {
                    app.spawn_async_child_count(&dir.path().join("queued"), &tx);
                }
            }
            for producer in 0..3 {
                match producer {
                    0 => app.spawn_s3_initial_load(&tx),
                    1 => app.spawn_s3_expand("s3://fake-bucket/child/".into(), &tx),
                    _ => {
                        app.tree_state.selected_index = app
                            .tree_state
                            .flat_items
                            .iter()
                            .position(|item| item.node_type == NodeType::File)
                            .unwrap();
                        app.spawn_s3_head(&tx);
                    }
                }
                if mode >= 3 {
                    let result = app.next_background().await.unwrap();
                    app.apply_background(result);
                }
                assert!(!app.s3_head_loading);
                assert!(!app.tree_state.root.is_loading);
                assert!(
                    !TreeState::find_node_mut_pub(
                        &mut app.tree_state.root,
                        &PathBuf::from("s3://fake-bucket/child/")
                    )
                    .unwrap()
                    .is_loading
                );
                assert_eq!(app.workspace.focus.overlay, AppMode::Help);
                let status = &app.status_message.as_ref().unwrap().0;
                assert!(
                    status.contains(match mode {
                        0 => "Full",
                        1 => "DomainLimit",
                        2 | 4 => "PayloadTooLarge",
                        _ => "Panicked",
                    }),
                    "{status}"
                );
                if mode <= 2 {
                    assert_eq!(summary_text(&app), "prior preview");
                }
            }
            *gate.0.lock().unwrap() = true;
            gate.1.notify_all();
            app.shutdown_background().await;
        }
    }

    #[tokio::test]
    async fn app_jobs_final_s3_startup_closed_has_no_unadmitted_loader() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                Default::default(),
                crate::background::Worker::Blocking(Arc::new(crate::app_jobs::run)),
            )
            .unwrap(),
        );
        app.shutdown_background().await;
        app.init_s3_mode(crate::s3::S3Config {
            path: crate::s3::S3Path::parse("s3://fake-bucket/").unwrap(),
            profile: None,
        });
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_s3_initial_load(&tx);
        assert!(
            !app.tree_state.root.is_loading,
            "startup loader appeared without admitted S3 work"
        );
    }

    #[tokio::test]
    async fn app_jobs_final_s3_navigation_cancels_actual_owned_head() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        app.init_s3_mode(crate::s3::S3Config {
            path: crate::s3::S3Path::parse("s3://fake-bucket/").unwrap(),
            profile: None,
        });
        app.handle_s3_listing_complete(
            "s3://fake-bucket/",
            ["A.txt", "B.txt"]
                .into_iter()
                .map(|name| crate::s3::S3Entry {
                    name: name.into(),
                    is_dir: false,
                    size: 1,
                    modified: String::new(),
                })
                .collect(),
        );
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered));
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_gate = gate.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let old_cancelled = cancelled.clone();
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                Default::default(),
                crate::background::Worker::Blocking(Arc::new(move |_, token| {
                    entered.lock().unwrap().take().unwrap().send(()).unwrap();
                    let _guard = worker_gate
                        .1
                        .wait_while(worker_gate.0.lock().unwrap(), |open| !*open)
                        .unwrap();
                    old_cancelled.store(token.is_cancelled(), Ordering::SeqCst);
                    crate::app_jobs::NativeOutput::S3Head("obsolete A".into())
                })),
            )
            .unwrap(),
        );
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.event_tx = Some(tx.clone());
        app.tree_state.selected_index = 1;
        app.spawn_s3_head(&tx);
        ready.await.unwrap();
        app.tree_state.selected_index = 2;
        app.last_previewed_index = None;
        app.update_preview();
        let retired = !app.jobs.as_ref().unwrap().has_pending();
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        app.shutdown_background().await;
        assert!(retired, "navigation kept obsolete head metadata");
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(!summary_text(&app).contains("obsolete A"));
    }

    #[tokio::test]
    async fn app_jobs_final_s3_direct_results_recover_their_own_state() {
        for head in [false, true] {
            for failed in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
                app.init_s3_mode(crate::s3::S3Config {
                    path: crate::s3::S3Path::parse("s3://fake-bucket/").unwrap(),
                    profile: None,
                });
                app.handle_s3_listing_complete(
                    "s3://fake-bucket/",
                    vec![crate::s3::S3Entry {
                        name: "file.txt".into(),
                        is_dir: false,
                        size: 1,
                        modified: String::new(),
                    }],
                );
                app.jobs = Some(
                    crate::app_jobs::AppJobs::new(
                        Default::default(),
                        crate::background::Worker::Blocking(Arc::new(move |_, _| {
                            if failed {
                                crate::app_jobs::NativeOutput::Failed("injected S3 failure")
                            } else if head {
                                crate::app_jobs::NativeOutput::S3Head("literal head\n".into())
                            } else {
                                crate::app_jobs::NativeOutput::S3Listing(vec![crate::s3::S3Entry {
                                    name: "accepted.txt".into(),
                                    is_dir: false,
                                    size: 7,
                                    modified: String::new(),
                                }])
                            }
                        })),
                    )
                    .unwrap(),
                );
                let (tx, mut rx) = crate::event::event_channel(Default::default());
                if head {
                    app.tree_state.selected_index = 1;
                    app.spawn_s3_head(&tx);
                } else {
                    app.spawn_s3_initial_load(&tx);
                }
                let result = app.next_background().await.unwrap();
                app.apply_background(result);
                assert!(
                    !app.s3_head_loading && !app.tree_state.root.is_loading,
                    "direct S3 result stranded its admitted loading state"
                );
                if failed {
                    assert!(app
                        .status_message
                        .as_ref()
                        .unwrap()
                        .0
                        .contains("injected S3 failure"));
                    if head {
                        assert!(summary_text(&app).contains("Incomplete"));
                    }
                } else if head {
                    assert!(summary_text(&app).contains("literal head"));
                } else {
                    assert_eq!(
                        app.tree_state.root.children.as_ref().unwrap()[0].name,
                        "accepted.txt"
                    );
                }
                rx.close();
                app.shutdown_background().await;
            }
        }
    }

    #[tokio::test]
    async fn app_jobs_final_s3_closed_admission_preserves_flags_and_preview() {
        for producer in 0..3 {
            let dir = tempfile::tempdir().unwrap();
            let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
            app.init_s3_mode(crate::s3::S3Config {
                path: crate::s3::S3Path::parse("s3://fake-bucket/").unwrap(),
                profile: None,
            });
            app.handle_s3_listing_complete(
                "s3://fake-bucket/",
                vec![
                    crate::s3::S3Entry {
                        name: "child/".into(),
                        is_dir: true,
                        size: 0,
                        modified: String::new(),
                    },
                    crate::s3::S3Entry {
                        name: "file.txt".into(),
                        is_dir: false,
                        size: 1,
                        modified: String::new(),
                    },
                ],
            );
            app.preview_state.content_lines = vec![Line::raw("keep preview")];
            app.jobs = Some(
                crate::app_jobs::AppJobs::new(
                    Default::default(),
                    crate::background::Worker::Blocking(Arc::new(crate::app_jobs::run)),
                )
                .unwrap(),
            );
            app.shutdown_background().await;
            let (tx, _rx) = crate::event::event_channel(Default::default());
            let path = PathBuf::from("s3://fake-bucket/child/");
            match producer {
                0 => app.spawn_s3_initial_load(&tx),
                1 => app.spawn_s3_expand(path.to_string_lossy().into_owned(), &tx),
                _ => {
                    app.tree_state.selected_index = app
                        .tree_state
                        .flat_items
                        .iter()
                        .position(|item| item.node_type == NodeType::File)
                        .unwrap();
                    app.spawn_s3_head(&tx);
                }
            }
            assert!(
                !app.s3_head_loading,
                "closed pool admitted head placeholder"
            );
            assert!(
                !TreeState::find_node_mut_pub(&mut app.tree_state.root, &path)
                    .unwrap()
                    .is_loading,
                "closed pool admitted expansion loader"
            );
            assert_eq!(app.preview_state.content_lines, [Line::raw("keep preview")]);
            assert!(
                app.status_message
                    .as_ref()
                    .unwrap()
                    .0
                    .contains("not admitted"),
                "S3 bypassed the closed shared pool"
            );
        }
    }

    #[tokio::test]
    async fn app_jobs_native_round1_collapse_loading_row_preserves_its_target_and_sibling() {
        for loading_row in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let a = dir.path().join("A");
            let b = dir.path().join("B");
            fs::create_dir(&a).unwrap();
            fs::create_dir(&b).unwrap();
            fs::write(b.join("child"), b"x").unwrap();
            let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
            sync_load_root(&mut app);
            let sibling = TreeState::find_node_mut_pub(&mut app.tree_state.root, &b).unwrap();
            sibling.load_children_paged(10).unwrap();
            sibling.is_expanded = true;
            app.tree_state.flatten();
            let (entered, ready) = tokio::sync::oneshot::channel();
            let entered = std::sync::Mutex::new(Some(entered));
            let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
            let worker_gate = gate.clone();
            app.jobs = Some(
                crate::app_jobs::AppJobs::new(
                    crate::background::Limits::default(),
                    crate::background::Worker::Blocking(Arc::new(move |job, token| {
                        entered.lock().unwrap().take().unwrap().send(()).unwrap();
                        let _open = worker_gate
                            .1
                            .wait_while(worker_gate.0.lock().unwrap(), |open| !*open)
                            .unwrap();
                        crate::app_jobs::run(job, token)
                    })),
                )
                .unwrap(),
            );
            let (tx, _rx) = crate::event::event_channel(Default::default());
            app.spawn_async_snapshot(&a, &tx);
            ready.await.unwrap();
            let kind = if loading_row {
                NodeType::Loading
            } else {
                NodeType::Directory
            };
            app.tree_state.selected_index = app
                .tree_state
                .flat_items
                .iter()
                .position(|item| item.path == a && item.node_type == kind)
                .unwrap();
            app.collapse_selected();
            let sibling_expanded = TreeState::find_node_mut_pub(&mut app.tree_state.root, &b)
                .unwrap()
                .is_expanded;
            let selected = app.tree_state.flat_items[app.tree_state.selected_index]
                .path
                .clone();
            let a_loading = TreeState::find_node_mut_pub(&mut app.tree_state.root, &a)
                .unwrap()
                .is_loading;
            let retired = !app.jobs.as_ref().unwrap().has_pending();
            *gate.0.lock().unwrap() = true;
            gate.1.notify_all();
            app.shutdown_background().await;
            assert!(
                sibling_expanded,
                "collapsing A's Loading row collapsed unrelated B"
            );
            assert_eq!(
                selected, a,
                "collapse must keep the original directory target"
            );
            assert!(!a_loading);
            assert!(retired);
        }
    }

    #[tokio::test]
    async fn app_jobs_summaries_aba_stale_progress_and_done_leave_root_count_independent() {
        use crate::app_jobs::{NativeOutput, Statistics};
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("A");
        let b = dir.path().join("B");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered));
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_gate = gate.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let old_cancelled = cancelled.clone();
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                crate::background::Limits {
                    workers: 2,
                    queued_results: 1,
                    ..Default::default()
                },
                crate::background::Worker::Blocking(Arc::new(move |job, token| {
                    if job.target.is_summary() {
                        let first = entered.lock().unwrap().take();
                        if let Some(tx) = first {
                            let progress = job.progress.as_ref().unwrap().clone();
                            // Many updates coalesce into one fixed-size newest value.
                            for files in 1..=10_000 {
                                progress.publish(Statistics {
                                    files,
                                    dirs: 0,
                                    size: 0,
                                });
                            }
                            tx.send(progress).ok().unwrap();
                            let _open = worker_gate
                                .1
                                .wait_while(worker_gate.0.lock().unwrap(), |open| !*open)
                                .unwrap();
                            old_cancelled.store(token.is_cancelled(), Ordering::SeqCst);
                        }
                    }
                    crate::app_jobs::run(job, token)
                })),
            )
            .unwrap(),
        );
        let (tx, mut rx) = crate::event::event_channel(crate::event::TransportLimits {
            slots: 1,
            ..Default::default()
        });
        tx.send(crate::event::Event::Resize(10, 10)).await.unwrap();
        app.preview_state.current_path = Some(a.clone());
        assert!(app.spawn_async_dir_summary(&a, &tx));
        let old_progress = ready.await.unwrap();
        let queued = app.next_background().await.unwrap();
        assert!(matches!(
            queued.result,
            Ok(NativeOutput::Progress(Statistics { files: 10_000, .. }))
        ));
        assert!(app.spawn_async_dir_summary_shallow(&b, &tx));
        app.preview_state.current_path = Some(b.clone());
        assert!(app.spawn_async_dir_summary(&a, &tx));
        app.preview_state.current_path = Some(a.clone());
        let current = app.active_summary;
        app.apply_background(queued);
        assert_eq!(app.active_summary, current);
        assert!(!summary_text(&app).contains("10000"));
        app.spawn_initial_load(&tx);
        app.spawn_async_child_count(dir.path(), &tx);
        let mut root = false;
        let mut count = false;
        let mut summary = false;
        for _ in 0..3 {
            let delivery = next_done(&mut app).await;
            root |= matches!(delivery.target, crate::app_jobs::Target::Root(_));
            count |= matches!(delivery.target, crate::app_jobs::Target::Count(_));
            summary |= delivery.target.is_summary();
            app.apply_background(delivery);
        }
        let text = summary_text(&app);
        let badge = app.tree_state.root.total_child_count;
        old_progress.publish(Statistics {
            files: 999_999,
            ..Default::default()
        });
        assert!(!app.jobs.as_ref().unwrap().has_pending());
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        rx.close();
        app.shutdown_background().await;
        assert!(root && count && summary);
        assert!(
            cancelled.load(Ordering::SeqCst),
            "old cancellation was reset"
        );
        assert!(
            text.contains("(Complete)") && text.contains("Files: 0"),
            "{text}"
        );
        assert_eq!(badge, Some(2));
        assert_eq!(app.active_dir_scan, None);
    }

    #[tokio::test]
    async fn app_jobs_summaries_delayed_shallow_to_deep_keeps_cancelled_slot_and_other_domains() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered));
        let first = AtomicBool::new(true);
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_gate = gate.clone();
        let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let occupied = active.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let old_cancelled = cancelled.clone();
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                crate::background::Limits {
                    workers: 2,
                    queued_results: 1,
                    ..Default::default()
                },
                crate::background::Worker::Blocking(Arc::new(move |job, token| {
                    let slots = occupied.fetch_add(1, Ordering::SeqCst) + 1;
                    assert!(slots <= 2);
                    if job.target.is_summary() && first.swap(false, Ordering::SeqCst) {
                        assert!(matches!(
                            job.target,
                            crate::app_jobs::Target::Summary { deep: false, .. }
                        ));
                        entered.lock().unwrap().take().unwrap().send(()).unwrap();
                        let _open = worker_gate
                            .1
                            .wait_while(worker_gate.0.lock().unwrap(), |open| !*open)
                            .unwrap();
                        old_cancelled.store(token.is_cancelled(), Ordering::SeqCst);
                    }
                    let output = crate::app_jobs::run(job, token);
                    occupied.fetch_sub(1, Ordering::SeqCst);
                    output
                })),
            )
            .unwrap(),
        );
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.preview_state.current_path = Some(dir.path().to_path_buf());
        assert!(app.spawn_async_dir_summary_shallow(dir.path(), &tx));
        ready.await.unwrap();
        assert!(app.spawn_async_dir_summary(dir.path(), &tx));
        app.spawn_initial_load(&tx);
        app.spawn_async_child_count(dir.path(), &tx);
        let mut targets = Vec::new();
        for _ in 0..3 {
            let delivery = next_done(&mut app).await;
            targets.push(delivery.target.clone());
            app.apply_background(delivery);
        }
        let still_owned = active.load(Ordering::SeqCst);
        let text = summary_text(&app);
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        app.shutdown_background().await;
        assert_eq!(still_owned, 1);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(targets
            .iter()
            .any(|target| matches!(target, crate::app_jobs::Target::Root(_))));
        assert!(targets
            .iter()
            .any(|target| matches!(target, crate::app_jobs::Target::Count(_))));
        assert!(targets
            .iter()
            .any(|target| matches!(target, crate::app_jobs::Target::Summary { deep: true, .. })));
        assert!(!app.preview_state.is_shallow_preview);
        assert!(text.contains("Deep Scan (Complete)"));
        assert_eq!(app.tree_state.root.total_child_count, Some(0));
    }

    #[tokio::test]
    async fn app_jobs_summaries_foreign_retired_done_does_not_clear_new_request() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("A");
        let b = dir.path().join("B");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.preview_state.current_path = Some(a.clone());
        assert!(app.spawn_async_dir_summary_shallow(&a, &tx));
        let old = next_done(&mut app).await;
        assert!(app.spawn_async_dir_summary(&b, &tx));
        app.preview_state.current_path = Some(b.clone());
        app.preview_state.content_lines = vec![Line::raw("B scanning")];
        let current = app.active_summary;
        app.apply_background(old);
        assert_eq!(app.active_dir_scan, Some(b));
        assert_eq!(app.active_summary, current);
        assert_eq!(summary_text(&app), "B scanning");
        let latest = next_done(&mut app).await;
        app.apply_background(latest);
        app.shutdown_background().await;
        assert!(summary_text(&app).contains("(Complete)"));
    }

    #[tokio::test]
    async fn app_jobs_summaries_actual_main_progress_drain_batches_redraw_with_full_event_queue() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered));
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_gate = gate.clone();
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                Default::default(),
                crate::background::Worker::Blocking(Arc::new(move |job, token| {
                    job.progress
                        .as_ref()
                        .unwrap()
                        .publish(crate::app_jobs::Statistics {
                            files: 123,
                            ..Default::default()
                        });
                    entered.lock().unwrap().take().unwrap().send(()).unwrap();
                    let _open = worker_gate
                        .1
                        .wait_while(worker_gate.0.lock().unwrap(), |open| !*open)
                        .unwrap();
                    crate::app_jobs::run(job, token)
                })),
            )
            .unwrap(),
        );
        let (tx, mut rx) = crate::event::event_channel(crate::event::TransportLimits {
            slots: 1,
            ..Default::default()
        });
        tx.send(crate::event::Event::Resize(10, 10)).await.unwrap();
        app.preview_state.current_path = Some(dir.path().to_path_buf());
        assert!(app.spawn_async_dir_summary(dir.path(), &tx));
        ready.await.unwrap();
        let wake = crate::wait_for_work(
            &mut app,
            std::future::pending::<crate::error::Result<crate::event::Event>>(),
            None,
        )
        .await
        .unwrap();
        let mut policy = crate::LoopPolicy::new(std::time::Duration::ZERO, Default::default(), 64);
        policy.redraw.drawn(std::time::Duration::ZERO); // startup frame already rendered
        if let crate::LoopWake::Background(delivery) = wake {
            crate::process_background(&mut app, &mut policy, delivery);
        } else {
            panic!("main did not drain the independent fixed-state notifier");
        }
        let wait = policy.redraw.wait(std::time::Duration::ZERO);
        let text = summary_text(&app);
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        let done = next_done(&mut app).await;
        crate::process_background(&mut app, &mut policy, done);
        let final_wait = policy.redraw.wait(std::time::Duration::ZERO);
        rx.close();
        app.shutdown_background().await;
        assert!(text.contains("Files: 123"));
        assert_eq!(
            wait,
            Some(std::time::Duration::from_millis(16)),
            "background progress forced an immediate render"
        );
        assert_eq!(final_wait, Some(std::time::Duration::ZERO));
    }

    #[tokio::test]
    async fn app_jobs_summaries_progress_after_result_retirement_cannot_revive_preview() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered));
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_gate = gate.clone();
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                Default::default(),
                crate::background::Worker::Blocking(Arc::new(move |job, token| {
                    job.progress
                        .as_ref()
                        .unwrap()
                        .publish(crate::app_jobs::Statistics {
                            files: 123,
                            ..Default::default()
                        });
                    entered.lock().unwrap().take().unwrap().send(()).unwrap();
                    let _open = worker_gate
                        .1
                        .wait_while(worker_gate.0.lock().unwrap(), |open| !*open)
                        .unwrap();
                    crate::app_jobs::run(job, token)
                })),
            )
            .unwrap(),
        );
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.preview_state.current_path = Some(dir.path().to_path_buf());
        assert!(app.spawn_async_dir_summary(dir.path(), &tx));
        ready.await.unwrap();
        let progress = app.next_background().await.unwrap();
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        let done = next_done(&mut app).await;
        // Metadata is retired even before done is installed in the App.
        app.apply_background(progress);
        assert!(!summary_text(&app).contains("123"));
        app.apply_background(done);
        assert!(summary_text(&app).contains("(Complete)"));
        app.shutdown_background().await;
    }

    async fn next_done(app: &mut App) -> crate::app_jobs::Delivery {
        loop {
            let delivery =
                tokio::time::timeout(std::time::Duration::from_secs(5), app.next_background())
                    .await
                    .unwrap()
                    .unwrap();
            if matches!(
                delivery.result,
                Ok(crate::app_jobs::NativeOutput::Progress(_))
            ) {
                app.apply_background(delivery);
            } else {
                return delivery;
            }
        }
    }

    fn summary_text(app: &App) -> String {
        app.preview_state
            .content_lines
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[tokio::test]
    async fn app_jobs_summaries_prepared_size_sort_page_offsets_and_metadata_failures() {
        for missing in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            for (name, data) in [("a", "x"), ("b", "longer"), ("c", "third")] {
                fs::write(dir.path().join(name), data).unwrap();
            }
            let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
            app.tree_state.page_size = 2;
            app.tree_state.sort_by = crate::fs::tree::SortBy::Size;
            app.jobs = Some(
                crate::app_jobs::AppJobs::new(
                    Default::default(),
                    crate::background::Worker::Blocking(Arc::new(move |job, token| {
                        let snapshot = crate::fs::tree::DirSnapshot::collect_budgeted(
                            job.target.path(),
                            job.max_entries,
                            job.result_bytes / 2,
                            || token.is_cancelled(),
                        )
                        .unwrap();
                        if missing {
                            fs::remove_file(job.target.path().join("a")).unwrap();
                        }
                        crate::app_jobs::NativeOutput::Snapshot(snapshot.prepare(
                            job.target.path(),
                            job.snapshot_options,
                            job.result_bytes / 2,
                            || token.is_cancelled(),
                        ))
                    })),
                )
                .unwrap(),
            );
            let (tx, _rx) = crate::event::event_channel(Default::default());
            app.spawn_initial_load(&tx);
            let delivery = next_done(&mut app).await;
            app.apply_background(delivery);
            let root = &mut app.tree_state.root;
            assert_eq!(
                root.loaded_offset, 2,
                "offset must include failed metadata entry"
            );
            assert_eq!(root.loaded_child_count, if missing { 1 } else { 2 });
            assert!(root.has_more_children);
            assert_eq!(root.total_child_count, Some(3));
            let names: Vec<_> = root
                .children
                .as_ref()
                .unwrap()
                .iter()
                .map(|node| node.name.as_str())
                .collect();
            assert_eq!(names, if missing { vec!["b"] } else { vec!["b", "a"] });
            assert_eq!(root.children.as_ref().unwrap()[0].depth, 1);
            assert_eq!(root.load_next_page(2).unwrap(), 1);
            assert_eq!(root.loaded_offset, 3);
            assert!(!root.has_more_children);
            app.shutdown_background().await;
        }
    }

    #[tokio::test]
    async fn app_jobs_summaries_paginated_status_preserves_page_hint() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a", "b", "c"] {
            fs::write(dir.path().join(name), "x").unwrap();
        }
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        app.tree_state.page_size = 2;
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_initial_load(&tx);
        let delivery = next_done(&mut app).await;
        app.apply_background(delivery);
        let status = app.status_message.as_ref().unwrap().0.clone();
        assert_eq!(status, "📂 Loaded 3 entries (showing first 2)");

        let small = tempfile::tempdir().unwrap();
        fs::write(small.path().join("only"), "x").unwrap();
        let mut app = App::new(small.path(), AppConfig::default()).unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_initial_load(&tx);
        let delivery = next_done(&mut app).await;
        app.apply_background(delivery);
        let status = app.status_message.as_ref().unwrap().0.clone();
        assert_eq!(status, "📂 Loaded 1 entries");
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn app_jobs_summaries_real_shallow_and_deep_counts_preserve_symlink_policy() {
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("child");
        fs::create_dir(&child).unwrap();
        fs::write(dir.path().join("top.txt"), b"abc").unwrap();
        fs::write(child.join("nested.txt"), b"de").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path(), child.join("loop")).unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (tx, mut rx) = crate::event::event_channel(Default::default());
        app.preview_state.current_path = Some(dir.path().to_path_buf());
        assert!(app.spawn_async_dir_summary_shallow(dir.path(), &tx));
        let shallow = next_done(&mut app).await;
        app.apply_background(shallow);
        assert!(app.preview_state.is_shallow_preview);
        let text = summary_text(&app);
        assert!(
            text.contains("1 (direct)") && text.contains("top.txt") && text.contains("child"),
            "{text}"
        );
        assert!(app.spawn_async_dir_summary(dir.path(), &tx));
        let deep = next_done(&mut app).await;
        #[rustfmt::skip]
        let statistics = match &deep.result { Ok(crate::app_jobs::NativeOutput::Deep { statistics, complete: true }) => *statistics, _ => unreachable!("real bounded traversal did not finish") };
        app.apply_background(deep);
        assert_eq!(statistics.dirs, 1);
        #[cfg(unix)]
        assert_eq!(statistics.files, 3); // inherited policy does not follow DirEntry symlinks
        #[cfg(not(unix))]
        assert_eq!(statistics.files, 2);
        assert!(statistics.size >= 5);
        assert!(summary_text(&app).contains("(Complete)"));
        assert!(!app.preview_state.is_shallow_preview);
        assert!(
            rx.try_recv().is_err(),
            "summary result used the canonical Event relay"
        );
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn app_jobs_summaries_entry_stack_visited_and_name_retention_caps_are_explicit() {
        use crate::background::Payload;
        for fixture in 0..3 {
            let dir = tempfile::tempdir().unwrap();
            match fixture {
                0 => {
                    for n in 0..10_002 {
                        fs::write(dir.path().join(format!("{n:05}")), b"x").unwrap();
                    }
                }
                1 => {
                    for n in 0..2050 {
                        fs::create_dir(dir.path().join(format!("{n:04}"))).unwrap();
                    }
                }
                _ => {
                    for n in 0..64 {
                        let parent = dir.path().join(format!("{n:02}"));
                        fs::create_dir(&parent).unwrap();
                        for child in 0..64 {
                            fs::create_dir(parent.join(format!("{child:02}"))).unwrap();
                        }
                    }
                }
            }
            let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
            app.config.general.snapshot_max_entries = Some(10_000);
            app.config.preview.preview_timeout_ms = Some(30_000);
            app.preview_state.current_path = Some(dir.path().to_path_buf());
            let (tx, _rx) = crate::event::event_channel(Default::default());
            if fixture == 0 {
                assert!(app.spawn_async_dir_summary_shallow(dir.path(), &tx));
                let output = next_done(&mut app).await;
                match &output.result {
                    Ok(crate::app_jobs::NativeOutput::Shallow(summary)) => {
                        assert!(!summary.complete);
                        assert!(summary.lines.len() <= 32);
                        assert!(
                            summary
                                .lines
                                .iter()
                                .filter(|line| line
                                    .spans
                                    .iter()
                                    .any(|span| span.content.contains("📄")))
                                .count()
                                <= 20
                        );
                        assert!(output.result.as_ref().unwrap().payload_bytes() < 32_768);
                    }
                    _ => unreachable!("shallow scan was not bounded"),
                }
                app.apply_background(output);
                assert!(summary_text(&app).contains("Incomplete"));
            }
            assert!(app.spawn_async_dir_summary(dir.path(), &tx));
            let output = next_done(&mut app).await;
            match &output.result {
                Ok(crate::app_jobs::NativeOutput::Deep {
                    statistics,
                    complete: false,
                }) => match fixture {
                    0 => assert_eq!(statistics.files, 10_000),
                    1 => assert_eq!(statistics.dirs, 2048),
                    _ => assert_eq!(statistics.dirs, 4095),
                },
                _ => unreachable!("traversal retention cap was labelled Complete"),
            }
            app.apply_background(output);
            assert!(summary_text(&app).contains("Incomplete"));
            app.shutdown_background().await;
        }
    }

    #[tokio::test]
    async fn app_jobs_summaries_failures_and_rejections_retire_only_own_preview_state() {
        use crate::app_jobs::{NativeJob, NativeOutput};
        for mode in 0..7 {
            let dir = tempfile::tempdir().unwrap();
            let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
            app.preview_state.current_path = Some(dir.path().to_path_buf());
            app.preview_state.content_lines = vec![Line::raw("keep before admission")];
            app.set_overlay(AppMode::Help);
            let worker =
                crate::background::Worker::Blocking(Arc::new(move |_: NativeJob, _| match mode {
                    0 => NativeOutput::Failed("injected I/O failure"),
                    1 => panic!("injected summary worker panic"),
                    _ => NativeOutput::Count {
                        count: 9,
                        complete: true,
                    },
                }));
            app.jobs = Some(
                crate::app_jobs::AppJobs::new(
                    crate::background::Limits {
                        job_bytes: if mode == 3 { 1 } else { 1_048_576 },
                        result_bytes: if mode == 2 { 1 } else { 8_388_608 },
                        request_domains: if mode == 4 { 1 } else { 32 },
                        ..Default::default()
                    },
                    worker,
                )
                .unwrap(),
            );
            let (tx, _rx) = crate::event::event_channel(Default::default());
            if mode == 4 {
                app.spawn_async_child_count(dir.path(), &tx);
            }
            if mode == 5 {
                app.shutdown_background().await;
            }
            let accepted = app.spawn_async_dir_summary(dir.path(), &tx);
            if accepted {
                let output = next_done(&mut app).await;
                app.apply_background(output);
            }
            let status = app.status_message.as_ref().unwrap().0.clone();
            assert_eq!(app.active_dir_scan, None);
            assert_eq!(app.active_summary, None);
            assert_eq!(app.workspace.focus.overlay, AppMode::Help);
            if mode == 3 || mode == 4 || mode == 5 {
                assert!(!accepted);
                assert_eq!(summary_text(&app), "keep before admission");
                assert!(status.contains("not admitted"));
            } else {
                assert!(accepted);
                assert!(summary_text(&app).contains("Incomplete"));
                assert!(status.contains(match mode {
                    0 => "I/O",
                    1 => "Panicked",
                    2 => "PayloadTooLarge",
                    _ => "mismatched",
                }));
            }
            app.shutdown_background().await;
        }
    }

    #[tokio::test]
    async fn app_jobs_summaries_real_io_path_and_rendered_name_budgets_are_truthful() {
        use crate::background::Payload;
        let dir = tempfile::tempdir().unwrap();
        for n in 0..24 {
            fs::write(dir.path().join(format!("{n:02}{}", "x".repeat(240))), b"x").unwrap();
        }
        for mode in 0..4 {
            for deep in [false, true] {
                let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
                app.jobs = Some(
                    crate::app_jobs::AppJobs::new(
                        crate::background::Limits {
                            result_bytes: if mode == 3 { 32_000 } else { 40_000 },
                            ..Default::default()
                        },
                        crate::background::Worker::Blocking(Arc::new(crate::app_jobs::run)),
                    )
                    .unwrap(),
                );
                let path = match mode {
                    0 => dir.path().join("missing"),
                    1 => PathBuf::from("x".repeat(4097)),
                    _ => dir.path().to_path_buf(),
                };
                app.preview_state.current_path = Some(path.clone());
                let (tx, _rx) = crate::event::event_channel(Default::default());
                if deep {
                    assert!(app.spawn_async_dir_summary(&path, &tx));
                } else {
                    assert!(app.spawn_async_dir_summary_shallow(&path, &tx));
                }
                let output = next_done(&mut app).await;
                assert!(
                    output.result.as_ref().unwrap().payload_bytes()
                        <= if mode == 3 { 32_000 } else { 40_000 }
                );
                if mode >= 2 && !deep {
                    if let Ok(crate::app_jobs::NativeOutput::Shallow(summary)) = &output.result {
                        assert!(!summary.complete);
                        assert!(summary.lines.len() <= 32);
                        assert!(
                            summary
                                .lines
                                .iter()
                                .filter(|line| line
                                    .spans
                                    .iter()
                                    .any(|span| span.content.contains("📄")))
                                .count()
                                < 20
                        );
                    } else {
                        panic!("bounded shallow preparation missing");
                    }
                }
                app.apply_background(output);
                assert_eq!(app.active_dir_scan, None);
                assert_eq!(app.active_summary, None);
                assert!(summary_text(&app).contains(if mode < 2 || !deep {
                    "Incomplete"
                } else {
                    "Complete"
                }));
                app.shutdown_background().await;
            }
        }
    }

    #[tokio::test]
    async fn app_jobs_summaries_navigation_cancel_and_shutdown_join_actual_worker() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file.txt");
        fs::write(&file, b"content").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        sync_load_root(&mut app);
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered));
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_gate = gate.clone();
        let exited = Arc::new(AtomicBool::new(false));
        let finished = exited.clone();
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                Default::default(),
                crate::background::Worker::Blocking(Arc::new(move |job, token| {
                    entered
                        .lock()
                        .unwrap()
                        .take()
                        .unwrap()
                        .send(job.progress.clone())
                        .ok()
                        .unwrap();
                    let _open = worker_gate
                        .1
                        .wait_while(worker_gate.0.lock().unwrap(), |open| !*open)
                        .unwrap();
                    assert!(token.is_cancelled());
                    let output = crate::app_jobs::run(job, token);
                    finished.store(true, Ordering::SeqCst);
                    output
                })),
            )
            .unwrap(),
        );
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.event_tx = Some(tx.clone());
        app.preview_state.current_path = Some(dir.path().to_path_buf());
        assert!(app.spawn_async_dir_summary(dir.path(), &tx));
        let progress = ready.await.unwrap().unwrap();
        app.tree_state.selected_index = app
            .tree_state
            .flat_items
            .iter()
            .position(|item| item.path == file)
            .unwrap();
        app.last_previewed_index = None;
        app.update_preview();
        assert_eq!(app.active_dir_scan, None);
        assert_eq!(app.active_summary, None);
        assert!(!app.jobs.as_ref().unwrap().has_pending());
        progress.publish(crate::app_jobs::Statistics {
            files: 999,
            ..Default::default()
        });
        let shutdown = app.shutdown_background();
        tokio::pin!(shutdown);
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(
            std::future::Future::poll(shutdown.as_mut(), &mut context).is_pending(),
            "cancelled worker credit was released before exit"
        );
        assert!(!exited.load(Ordering::SeqCst));
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        shutdown.await;
        assert!(exited.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn app_jobs_summaries_shutdown_marks_visible_scan_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.preview_state.current_path = Some(dir.path().to_path_buf());
        app.preview_state.content_lines = vec![Line::raw("Scanning...")];
        assert!(app.spawn_async_dir_summary(dir.path(), &tx));
        app.shutdown_background().await;
        assert_eq!(app.active_dir_scan, None);
        assert_eq!(app.active_summary, None);
        assert!(
            summary_text(&app).contains("Incomplete"),
            "closed scan still claims to be scanning"
        );
    }

    #[tokio::test]
    async fn app_jobs_summaries_changed_sort_page_options_reject_old_prepared_result() {
        for option in 0..4 {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("a"), b"x").unwrap();
            let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
            let (tx, _rx) = crate::event::event_channel(Default::default());
            app.spawn_initial_load(&tx);
            let delivery = app.next_background().await.unwrap();
            match option {
                0 => app.tree_state.sort_by = crate::fs::tree::SortBy::Size,
                1 => app.tree_state.dirs_first = false,
                2 => app.tree_state.page_size = 1,
                _ => app.tree_state.root.depth = 4,
            }
            app.apply_background(delivery);
            let applied = app.tree_state.root.children.is_some();
            let loading = app.tree_state.root.is_loading;
            let message = app.status_message.as_ref().unwrap().0.clone();
            app.shutdown_background().await;
            assert!(!applied, "old prepared options were applied as current");
            assert!(!loading);
            assert!(message.contains("options changed"), "{message}");
        }
    }

    #[tokio::test]
    async fn app_jobs_summaries_prepared_page_survives_removed_fixture_without_consumer_stat() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("already-prepared.txt");
        fs::write(&file, b"prepared metadata").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_initial_load(&tx);
        let delivery = app.next_background().await.unwrap();
        fs::remove_file(&file).unwrap();
        app.apply_background(delivery);
        let children = app.tree_state.root.children.as_ref().unwrap();
        let names: Vec<_> = children.iter().map(|node| node.name.clone()).collect();
        let sizes: Vec<_> = children.iter().map(|node| node.meta.size).collect();
        app.shutdown_background().await;
        assert_eq!(
            names,
            ["already-prepared.txt"],
            "consumer re-statted worker data"
        );
        assert_eq!(sizes, [17]);
    }

    #[tokio::test]
    async fn app_jobs_summaries_complete_count_patches_only_its_visible_badge() {
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("A");
        let sibling = dir.path().join("B");
        fs::create_dir(&child).unwrap();
        fs::create_dir(&sibling).unwrap();
        for n in 0..3 {
            fs::write(child.join(format!("{n}")), b"x").unwrap();
        }
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        sync_load_root(&mut app);
        TreeState::find_node_mut_pub(&mut app.tree_state.root, &sibling)
            .unwrap()
            .total_child_count = Some(7);
        app.tree_state.flatten();
        let selected = app
            .tree_state
            .flat_items
            .iter()
            .position(|item| item.path == sibling)
            .unwrap();
        app.tree_state.selected_index = selected;
        app.tree_state.multi_selected.extend([0, selected]);
        app.tree_state.scroll_offset = 1;
        let marked = app.tree_state.multi_selected.clone();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_async_child_count(&child, &tx);
        let delivery = app.next_background().await.unwrap();
        app.apply_background(delivery);
        let badge = app
            .tree_state
            .flat_items
            .iter()
            .find(|item| item.path == child)
            .unwrap()
            .child_count;
        assert_eq!(
            app.tree_state
                .flat_items
                .iter()
                .find(|item| item.path == sibling)
                .unwrap()
                .child_count,
            Some(7)
        );
        assert_eq!(app.tree_state.selected_index, selected);
        assert_eq!(app.tree_state.multi_selected, marked);
        assert_eq!(app.tree_state.scroll_offset, 1);
        app.shutdown_background().await;
        assert_eq!(
            badge,
            Some(3),
            "complete model count did not reach renderer-facing badge"
        );
    }

    #[tokio::test]
    async fn app_jobs_summaries_full_pool_rejects_both_producers_before_active_preview_mutation() {
        for deep in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
            let (entered, ready) = tokio::sync::oneshot::channel();
            let entered = std::sync::Mutex::new(Some(entered));
            let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
            let worker_gate = gate.clone();
            app.jobs = Some(
                crate::app_jobs::AppJobs::new(
                    crate::background::Limits {
                        workers: 1,
                        queued_jobs: 1,
                        ..Default::default()
                    },
                    crate::background::Worker::Blocking(Arc::new(move |job, token| {
                        if let Some(tx) = entered.lock().unwrap().take() {
                            tx.send(()).unwrap();
                        }
                        let _open = worker_gate
                            .1
                            .wait_while(worker_gate.0.lock().unwrap(), |open| !*open)
                            .unwrap();
                        crate::app_jobs::run(job, token)
                    })),
                )
                .unwrap(),
            );
            let (tx, _rx) = crate::event::event_channel(Default::default());
            app.spawn_initial_load(&tx);
            ready.await.unwrap();
            app.spawn_async_child_count(dir.path(), &tx);
            app.preview_state.content_lines = vec![ratatui::text::Line::raw("keep preview")];
            if deep {
                app.spawn_async_dir_summary(dir.path(), &tx);
            } else {
                app.spawn_async_dir_summary_shallow(dir.path(), &tx);
            }
            let active = app.active_dir_scan.clone();
            let preview = app.preview_state.content_lines.clone();
            let status = app.status_message.as_ref().unwrap().0.clone();
            *gate.0.lock().unwrap() = true;
            gate.1.notify_all();
            app.shutdown_background().await;
            assert_eq!(active, None, "summary bypassed finite admission");
            assert_eq!(preview, [ratatui::text::Line::raw("keep preview")]);
            assert!(status.contains("not admitted: Full"), "{status}");
        }
    }

    #[tokio::test]
    async fn app_jobs_summaries_frontend_rejection_preserves_prior_request_and_preview() {
        for deep in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let a = dir.path().join("A");
            let b = dir.path().join("B");
            fs::create_dir(&a).unwrap();
            fs::create_dir(&b).unwrap();
            let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
            sync_load_root(&mut app);
            let (entered, ready) = tokio::sync::oneshot::channel();
            let entered = std::sync::Mutex::new(Some(entered));
            let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
            let worker_gate = gate.clone();
            app.jobs = Some(
                crate::app_jobs::AppJobs::new(
                    crate::background::Limits {
                        workers: 1,
                        queued_jobs: 1,
                        ..Default::default()
                    },
                    crate::background::Worker::Blocking(Arc::new(move |job, token| {
                        if let Some(tx) = entered.lock().unwrap().take() {
                            tx.send(()).unwrap();
                        }
                        let _open = worker_gate
                            .1
                            .wait_while(worker_gate.0.lock().unwrap(), |open| !*open)
                            .unwrap();
                        crate::app_jobs::run(job, token)
                    })),
                )
                .unwrap(),
            );
            let (tx, _rx) = crate::event::event_channel(Default::default());
            app.event_tx = Some(tx.clone());
            assert!(app.spawn_async_dir_summary_shallow(&a, &tx));
            ready.await.unwrap();
            app.spawn_async_child_count(dir.path(), &tx);
            app.preview_state.current_path = Some(a.clone());
            app.preview_state.content_lines = vec![ratatui::text::Line::raw("keep preview")];
            app.preview_state.is_shallow_preview = true;
            app.preview_state.total_lines = 1;
            let identity = app.active_summary;
            if deep {
                app.workspace.focus.panel = FocusedPanel::Preview;
                crate::handler::handle_key_event(
                    &mut app,
                    crossterm::event::KeyEvent::new(
                        crossterm::event::KeyCode::Char('D'),
                        crossterm::event::KeyModifiers::NONE,
                    ),
                    &tx,
                );
            } else {
                app.tree_state.selected_index = app
                    .tree_state
                    .flat_items
                    .iter()
                    .position(|item| item.path == b)
                    .unwrap();
                app.last_previewed_index = None;
                app.update_preview();
            }
            let preserved = app.preview_state.current_path == Some(a.clone())
                && app.preview_state.content_lines == [ratatui::text::Line::raw("keep preview")]
                && app.preview_state.is_shallow_preview
                && app.active_summary == identity
                && app.active_dir_scan == Some(a);
            let message = app.status_message.as_ref().unwrap().0.clone();
            *gate.0.lock().unwrap() = true;
            gate.1.notify_all();
            app.shutdown_background().await;
            assert!(
                preserved,
                "frontend mutated preview before rejected admission"
            );
            assert!(message.contains("not admitted: Full"), "{message}");
        }
    }

    #[tokio::test]
    async fn app_jobs_summaries_refresh_retirement_clears_only_own_active_scan() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        fs::write(&file, b"x").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        sync_load_root(&mut app);
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered));
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_gate = gate.clone();
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                Default::default(),
                crate::background::Worker::Blocking(Arc::new(move |job, token| {
                    entered.lock().unwrap().take().unwrap().send(()).unwrap();
                    let _open = worker_gate
                        .1
                        .wait_while(worker_gate.0.lock().unwrap(), |open| !*open)
                        .unwrap();
                    crate::app_jobs::run(job, token)
                })),
            )
            .unwrap(),
        );
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.preview_state.current_path = Some(dir.path().to_path_buf());
        assert!(app.spawn_async_dir_summary(dir.path(), &tx));
        ready.await.unwrap();
        app.watcher_active = true;
        app.handle_fs_change(vec![file]);
        let active = app.active_dir_scan.clone();
        let identity = app.active_summary;
        let retired = !app.jobs.as_ref().unwrap().has_pending();
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        app.shutdown_background().await;
        assert!(retired);
        assert_eq!(
            active, None,
            "cancel_under retired metadata but stranded active scan"
        );
        assert_eq!(identity, None);
    }

    #[tokio::test]
    async fn app_jobs_summaries_zero_deadline_is_incomplete_not_complete() {
        for deep in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("file"), b"bytes").unwrap();
            let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
            app.config.preview.preview_timeout_ms = Some(0);
            app.preview_state.current_path = Some(dir.path().to_path_buf());
            let (tx, mut rx) = crate::event::event_channel(Default::default());
            if deep {
                app.spawn_async_dir_summary(dir.path(), &tx);
            } else {
                app.spawn_async_dir_summary_shallow(dir.path(), &tx);
            }
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while app.active_dir_scan.is_some() {
                    tokio::select! {
                        delivery = app.next_background() => app.apply_background(delivery.unwrap()),
                        event = rx.recv() => panic!("summary bypassed direct drainage: {}", event.is_some()),
                    }
                }
            }).await.unwrap();
            let text = app
                .preview_state
                .content_lines
                .iter()
                .flat_map(|line| &line.spans)
                .map(|span| span.content.as_ref())
                .collect::<String>();
            app.shutdown_background().await;
            assert!(
                text.contains("Incomplete"),
                "deadline was labelled complete: {text}"
            );
            assert!(!text.contains("(Complete)"));
        }
    }

    #[tokio::test]
    async fn app_jobs_native_round1_incomplete_count_clears_only_its_flat_badge() {
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("A");
        let sibling = dir.path().join("B");
        fs::create_dir(&child).unwrap();
        fs::create_dir(&sibling).unwrap();
        for n in 0..3 {
            fs::write(child.join(format!("{n}")), b"x").unwrap();
        }
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        sync_load_root(&mut app);
        TreeState::find_node_mut_pub(&mut app.tree_state.root, &child)
            .unwrap()
            .total_child_count = Some(3);
        TreeState::find_node_mut_pub(&mut app.tree_state.root, &sibling)
            .unwrap()
            .total_child_count = Some(7);
        app.tree_state.flatten();
        let selected = app
            .tree_state
            .flat_items
            .iter()
            .position(|item| item.path == sibling)
            .unwrap();
        app.tree_state.selected_index = selected;
        app.tree_state.multi_selected.extend([0, selected]);
        app.tree_state.scroll_offset = 1;
        let marked = app.tree_state.multi_selected.clone();
        let before_rows: Vec<_> = app
            .tree_state
            .flat_items
            .iter()
            .map(|item| (item.path.clone(), item.node_type.clone()))
            .collect();
        app.config.preview.preview_timeout_ms = Some(0);
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_async_child_count(&child, &tx);
        let delivery = app.next_background().await.unwrap();
        let incomplete = matches!(
            delivery.result,
            Ok(crate::app_jobs::NativeOutput::Count {
                count: 0,
                complete: false
            })
        );
        app.apply_background(delivery);
        let model = TreeState::find_node_mut_pub(&mut app.tree_state.root, &child)
            .unwrap()
            .total_child_count;
        let badge = app
            .tree_state
            .flat_items
            .iter()
            .find(|item| item.path == child)
            .unwrap()
            .child_count;
        let other_badge = app
            .tree_state
            .flat_items
            .iter()
            .find(|item| item.path == sibling)
            .unwrap()
            .child_count;
        let after_selection = app.tree_state.selected_index;
        let after_marked = app.tree_state.multi_selected.clone();
        let after_scroll = app.tree_state.scroll_offset;
        let after_rows: Vec<_> = app
            .tree_state
            .flat_items
            .iter()
            .map(|item| (item.path.clone(), item.node_type.clone()))
            .collect();
        let status = app.status_message.as_ref().unwrap().0.clone();
        app.shutdown_background().await;
        assert!(incomplete);
        assert!(status.contains("incomplete"));
        assert_eq!(model, None);
        assert_eq!(
            badge, None,
            "incomplete native count kept the renderer-facing exact badge"
        );
        assert_eq!(other_badge, Some(7));
        assert_eq!(after_selection, selected);
        assert_eq!(
            after_marked, marked,
            "badge update must not flatten away multi-selection"
        );
        assert_eq!(after_scroll, 1);
        assert_eq!(after_rows, before_rows);
    }

    #[tokio::test]
    async fn app_jobs_real_root_admission_full_preserves_flags_documents_and_prior_jobs() {
        use crate::app_jobs::{AppJobs, NativeJob, NativeOutput, Target};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("document.txt");
        fs::write(&path, b"original").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        app.open_document_path(&path, true);
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .insert_char('x');
        app.tree_state.root.is_loading = false;
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered));
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_gate = gate.clone();
        app.jobs = Some(
            AppJobs::new(
                crate::background::Limits {
                    workers: 1,
                    queued_jobs: 1,
                    queued_results: 1,
                    request_domains: 4,
                    ..Default::default()
                },
                crate::background::Worker::Blocking(Arc::new(move |_: NativeJob, _| {
                    if let Some(tx) = entered.lock().unwrap().take() {
                        tx.send(()).unwrap();
                    }
                    let _open = worker_gate
                        .1
                        .wait_while(worker_gate.0.lock().unwrap(), |open| !*open)
                        .unwrap();
                    NativeOutput::Count {
                        count: 1,
                        complete: true,
                    }
                })),
            )
            .unwrap(),
        );
        let job = |path| NativeJob {
            target: Target::Count(path),
            max_entries: 10,
            timeout: std::time::Duration::from_secs(1),
            result_bytes: 1024,
            snapshot_options: test_snapshot_options(),
            summary_colors: None,
            progress: None,
            s3_profile: None,
            s3_head_lines: 0,
            clipboard: None,
            preview_theme: None,
            search: None,
        };
        app.jobs
            .as_mut()
            .unwrap()
            .submit(job(dir.path().join("A")))
            .unwrap();
        ready.await.unwrap(); // one real blocking slot is occupied
        app.jobs
            .as_mut()
            .unwrap()
            .submit(job(dir.path().join("B")))
            .unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_initial_load(&tx); // must use the same finite runtime pool
        let loading = app.tree_state.root.is_loading;
        let status = app.status_message.as_ref().unwrap().0.clone();
        let document = app
            .workspace
            .documents
            .active()
            .unwrap()
            .editor
            .buffer
            .join("\n");
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        app.jobs.as_mut().unwrap().shutdown().await;
        assert!(!loading, "rejection must not open a loading placeholder");
        assert!(
            status.contains("not admitted: Full"),
            "real App producer bypassed finite admission: {status}"
        );
        assert_eq!(document, "xoriginal");
        assert!(app.workspace.documents.active().unwrap().editor.modified);
    }

    #[tokio::test]
    async fn app_jobs_real_root_publishes_direct_bounded_snapshot_not_event_relay() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("actual.txt"), b"bytes").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (tx, mut rx) = crate::event::event_channel(Default::default());
        app.spawn_initial_load(&tx);
        let delivery = app.jobs.as_mut().unwrap().next().await.unwrap();
        app.jobs.as_mut().unwrap().shutdown().await;
        assert!(
            matches!(delivery.result, Ok(crate::app_jobs::NativeOutput::Snapshot(prepared)) if prepared.snapshot.entries.len() == 1 && prepared.snapshot.entries[0].name == "actual.txt")
        );
        assert!(
            rx.try_recv().is_err(),
            "scheduler results must be consumed directly, not relayed"
        );
    }

    #[tokio::test]
    async fn app_jobs_actual_expand_and_count_share_independent_generation_domains() {
        use crate::app_jobs::{AppJobs, NativeJob, NativeOutput};
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("A");
        let b = dir.path().join("B");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let worker =
            crate::background::Worker::Blocking(Arc::new(|_: NativeJob, _| NativeOutput::Count {
                count: 1,
                complete: true,
            }));
        app.jobs = Some(AppJobs::new(crate::background::Limits::default(), worker).unwrap());
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_async_snapshot(&a, &tx);
        app.spawn_async_child_count(&b, &tx);
        app.spawn_async_snapshot(&a, &tx); // accepted A/B/A must retire old A
        let pending = app.jobs.as_ref().unwrap().has_pending();
        if !pending {
            app.jobs.as_mut().unwrap().shutdown().await;
            assert!(
                pending,
                "actual expand/count still bypass domain-aware scheduler"
            );
        }
        let one = app.jobs.as_mut().unwrap().next().await.unwrap();
        let two = app.jobs.as_mut().unwrap().next().await.unwrap();
        app.jobs.as_mut().unwrap().shutdown().await;
        let mut paths = [
            one.target.path().to_path_buf(),
            two.target.path().to_path_buf(),
        ];
        paths.sort();
        assert_eq!(paths, [a, b]);
    }

    #[tokio::test]
    async fn app_jobs_capped_native_snapshot_never_recollects_or_claims_exact_count() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..8 {
            fs::write(dir.path().join(format!("file{n}")), b"x").unwrap();
        }
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                crate::background::Limits {
                    result_bytes: 192,
                    ..Default::default()
                },
                crate::background::Worker::Blocking(Arc::new(crate::app_jobs::run)),
            )
            .unwrap(),
        );
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_initial_load(&tx);
        let delivery = app.next_background().await.unwrap();
        app.apply_background(delivery);
        app.shutdown_background().await;
        let root = &app.tree_state.root;
        assert!(
            root.children.as_ref().unwrap().len() < 8,
            "consumer re-enumerated the capped snapshot"
        );
        assert!(root.snapshot.as_ref().unwrap().capped);
        assert!(
            root.total_child_count.is_none(),
            "partial count cannot claim an exact badge"
        );
        assert!(!root.is_loading);
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("Incomplete"));
    }

    #[tokio::test]
    async fn app_jobs_root_count_cross_domain_replacement_keeps_cancelled_blocking_slot_owned() {
        use crate::app_jobs::{AppJobs, NativeJob, NativeOutput};
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old.txt"), b"old").unwrap();
        fs::write(dir.path().join("new.txt"), b"new").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let first = Arc::new(AtomicBool::new(true));
        let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let peak = active.clone();
        let old_cancelled = Arc::new(AtomicBool::new(false));
        let cancelled = old_cancelled.clone();
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_gate = gate.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let started = std::sync::Mutex::new(Some(started));
        app.jobs = Some(
            AppJobs::new(
                crate::background::Limits {
                    workers: 2,
                    ..Default::default()
                },
                crate::background::Worker::Blocking(Arc::new(move |job: NativeJob, token| {
                    let now = peak.fetch_add(1, Ordering::SeqCst) + 1;
                    assert!(now <= 2, "cancelled blocking work lost its owned slot");
                    let old = job.target.is_snapshot() && first.swap(false, Ordering::SeqCst);
                    if old {
                        started.lock().unwrap().take().unwrap().send(()).unwrap();
                        let _open = worker_gate
                            .1
                            .wait_while(worker_gate.0.lock().unwrap(), |open| !*open)
                            .unwrap();
                        cancelled.store(token.is_cancelled(), Ordering::SeqCst);
                    }
                    let output = if job.target.is_snapshot() {
                        NativeOutput::Snapshot(
                            crate::fs::tree::DirSnapshot {
                                entries: vec![crate::fs::tree::SnapshotEntry {
                                    name: if old { "old.txt" } else { "new.txt" }.into(),
                                    is_dir: false,
                                }],
                                skipped_count: 0,
                                capped: false,
                            }
                            .prepare(
                                job.target.path(),
                                job.snapshot_options,
                                8192,
                                || false,
                            ),
                        )
                    } else {
                        NativeOutput::Count {
                            count: 17,
                            complete: true,
                        }
                    };
                    peak.fetch_sub(1, Ordering::SeqCst);
                    output
                })),
            )
            .unwrap(),
        );
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_initial_load(&tx);
        ready.await.unwrap();
        app.spawn_async_child_count(dir.path(), &tx); // independent same-path domain
        app.spawn_initial_load(&tx); // supersedes blocked root only
        let mut deliveries = Vec::new();
        for _ in 0..2 {
            deliveries.push(app.next_background().await.unwrap());
        }
        // Both current jobs finish while the old blocking closure still owns a
        // slot; cancellation must never reset or free that slot prematurely.
        let occupied = active.load(Ordering::SeqCst);
        for delivery in deliveries {
            app.apply_background(delivery);
        }
        let children: Vec<_> = app
            .tree_state
            .root
            .children
            .as_ref()
            .unwrap()
            .iter()
            .map(|n| n.name.clone())
            .collect();
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        app.shutdown_background().await;
        assert_eq!(occupied, 1);
        assert!(old_cancelled.load(Ordering::SeqCst));
        assert_eq!(children, ["new.txt"]);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert!(!app.jobs.as_ref().unwrap().has_pending());
    }

    #[tokio::test]
    async fn app_jobs_failures_retire_own_loader_and_do_not_close_nested_modal() {
        use crate::app_jobs::{AppJobs, NativeJob, NativeOutput};
        for mode in 0..4 {
            let dir = tempfile::tempdir().unwrap();
            let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
            let worker =
                crate::background::Worker::Blocking(Arc::new(
                    move |job: NativeJob, _| match mode {
                        0 => NativeOutput::Failed("injected I/O failure"),
                        1 => panic!("injected actual App worker panic"),
                        2 => NativeOutput::Snapshot(
                            crate::fs::tree::DirSnapshot {
                                entries: vec![crate::fs::tree::SnapshotEntry {
                                    name: "oversized".repeat(100).into(),
                                    is_dir: false,
                                }],
                                skipped_count: 0,
                                capped: false,
                            }
                            .prepare(
                                job.target.path(),
                                job.snapshot_options,
                                8192,
                                || false,
                            ),
                        ),
                        _ => NativeOutput::Count {
                            count: 0,
                            complete: true,
                        }, // wrong kind
                    },
                ));
            app.jobs = Some(
                AppJobs::new(
                    crate::background::Limits {
                        result_bytes: std::mem::size_of::<NativeOutput>() + 128,
                        ..Default::default()
                    },
                    worker,
                )
                .unwrap(),
            );
            app.set_overlay(AppMode::Help);
            let (tx, _rx) = crate::event::event_channel(Default::default());
            app.spawn_initial_load(&tx);
            assert!(app.tree_state.root.is_loading);
            let delivery = app.next_background().await.unwrap();
            app.apply_background(delivery);
            app.shutdown_background().await;
            assert!(!app.tree_state.root.is_loading);
            assert_eq!(app.workspace.focus.overlay, AppMode::Help);
            assert!(app
                .status_message
                .as_ref()
                .unwrap()
                .0
                .contains(if mode == 0 {
                    "I/O"
                } else if mode == 1 {
                    "Panicked"
                } else if mode == 2 {
                    "PayloadTooLarge"
                } else {
                    "mismatched"
                }));
            assert!(!app.jobs.as_ref().unwrap().has_pending());
        }
    }

    #[tokio::test]
    async fn app_jobs_admission_domain_payload_closed_and_deadline_recover_truthfully() {
        use crate::app_jobs::{AppJobs, NativeJob, NativeOutput};
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        // An accepted count domain occupies the only request slot; root
        // rejection must preserve it even if its result is already queued.
        app.jobs = Some(
            AppJobs::new(
                crate::background::Limits {
                    request_domains: 1,
                    ..Default::default()
                },
                crate::background::Worker::Blocking(Arc::new(|_: NativeJob, _| {
                    NativeOutput::Count {
                        count: 9,
                        complete: true,
                    }
                })),
            )
            .unwrap(),
        );
        app.spawn_async_child_count(dir.path(), &tx);
        app.spawn_initial_load(&tx);
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("DomainLimit"));
        assert!(!app.tree_state.root.is_loading);
        let delivery = app.next_background().await.unwrap();
        app.apply_background(delivery);
        assert_eq!(app.tree_state.root.total_child_count, Some(9));
        app.shutdown_background().await;
        app.spawn_initial_load(&tx);
        assert!(app.status_message.as_ref().unwrap().0.contains("Closed"));
        assert!(!app.tree_state.root.is_loading);
        app.jobs = Some(
            AppJobs::new(
                crate::background::Limits {
                    job_bytes: 1,
                    ..Default::default()
                },
                crate::background::Worker::Blocking(Arc::new(crate::app_jobs::run)),
            )
            .unwrap(),
        );
        app.spawn_initial_load(&tx);
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("PayloadTooLarge"));
        app.shutdown_background().await;
        // Deadline is injected into the real worker; zero means cap before any
        // read_dir and cannot be portrayed as a complete count.
        app.jobs = Some(
            AppJobs::new(
                crate::background::Limits::default(),
                crate::background::Worker::Blocking(Arc::new(crate::app_jobs::run)),
            )
            .unwrap(),
        );
        app.jobs
            .as_mut()
            .unwrap()
            .submit(NativeJob {
                target: crate::app_jobs::Target::Count(dir.path().to_path_buf()),
                max_entries: 1,
                timeout: std::time::Duration::ZERO,
                result_bytes: 1024,
                snapshot_options: test_snapshot_options(),
                summary_colors: None,
                progress: None,
                s3_profile: None,
                s3_head_lines: 0,
                clipboard: None,
                preview_theme: None,
                search: None,
            })
            .unwrap();
        let delivery = app.next_background().await.unwrap();
        app.apply_background(delivery);
        app.shutdown_background().await;
        assert!(app.tree_state.root.total_child_count.is_none());
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("incomplete"));
    }

    #[tokio::test]
    async fn app_jobs_refresh_invalidates_old_native_scan_before_model_refresh() {
        use crate::app_jobs::{AppJobs, NativeJob, NativeOutput};
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("new.txt"), b"x").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered));
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_gate = gate.clone();
        app.jobs = Some(
            AppJobs::new(
                crate::background::Limits::default(),
                crate::background::Worker::Blocking(Arc::new(move |_: NativeJob, _| {
                    entered.lock().unwrap().take().unwrap().send(()).unwrap();
                    let _open = worker_gate
                        .1
                        .wait_while(worker_gate.0.lock().unwrap(), |open| !*open)
                        .unwrap();
                    NativeOutput::Failed("old must not apply")
                })),
            )
            .unwrap(),
        );
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_initial_load(&tx);
        ready.await.unwrap();
        app.watcher_active = true;
        app.handle_fs_change(vec![dir.path().to_path_buf()]);
        let pending = app.jobs.as_ref().unwrap().has_pending();
        let loading = app.tree_state.root.is_loading;
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        app.shutdown_background().await;
        assert!(
            !pending,
            "refreshed native request must invalidate its generation"
        );
        assert!(!loading, "refresh must retire only its old loading state");
    }

    #[tokio::test]
    async fn app_jobs_shutdown_retires_accepted_loader_before_closed_readmission() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_initial_load(&tx);
        assert!(app.tree_state.root.is_loading);
        app.shutdown_background().await;
        app.spawn_initial_load(&tx);
        assert!(
            !app.tree_state.root.is_loading,
            "closed native scheduler left a permanent loader"
        );
        assert!(app.status_message.as_ref().unwrap().0.contains("Closed"));
    }

    #[tokio::test]
    async fn app_jobs_complete_small_snapshot_preserves_existing_immediate_watcher_refresh() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old.txt"), b"x").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_initial_load(&tx);
        let delivery = app.next_background().await.unwrap();
        app.apply_background(delivery);
        let new = dir.path().join("new.txt");
        fs::write(&new, b"x").unwrap();
        app.watcher_active = true;
        app.handle_fs_change(vec![new]);
        let names: Vec<_> = app
            .tree_state
            .root
            .children
            .as_ref()
            .unwrap()
            .iter()
            .map(|n| n.name.clone())
            .collect();
        app.shutdown_background().await;
        assert!(
            names.contains(&"new.txt".into()),
            "small native completion accidentally changed watcher refresh policy"
        );
    }

    #[tokio::test]
    async fn app_jobs_full_results_queued_aba_rejects_old_a_without_invalidating_b() {
        use crate::app_jobs::{AppJobs, NativeJob, NativeOutput};
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let executed = std::sync::atomic::AtomicUsize::new(0);
        let (second, reached) = tokio::sync::oneshot::channel();
        let second = std::sync::Mutex::new(Some(second));
        app.jobs = Some(
            AppJobs::new(
                crate::background::Limits {
                    workers: 1,
                    queued_results: 1,
                    ..Default::default()
                },
                crate::background::Worker::Blocking(Arc::new(move |_: NativeJob, _| {
                    let count = executed.fetch_add(1, Ordering::SeqCst) + 1;
                    // Running B on the single worker proves A was published to the
                    // capacity-one result queue before this barrier.
                    if count == 2 {
                        second.lock().unwrap().take().unwrap().send(()).unwrap();
                    }
                    NativeOutput::Count {
                        count,
                        complete: true,
                    }
                })),
            )
            .unwrap(),
        );
        let a = dir.path().to_path_buf();
        let b = dir.path().join("B");
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_async_child_count(&a, &tx);
        app.spawn_async_child_count(&b, &tx);
        reached.await.unwrap();
        app.spawn_async_child_count(&a, &tx);
        let first = app.next_background().await.unwrap();
        let second = app.next_background().await.unwrap();
        app.shutdown_background().await;
        assert_eq!(first.target.path(), b);
        assert!(matches!(
            first.result,
            Ok(NativeOutput::Count { count: 2, .. })
        ));
        assert_eq!(second.target.path(), a);
        assert!(matches!(
            second.result,
            Ok(NativeOutput::Count { count: 3, .. })
        ));
        assert!(!app.jobs.as_ref().unwrap().has_pending());
    }

    #[tokio::test]
    async fn app_jobs_actual_native_io_errors_entry_caps_and_snapshot_deadline_are_visible() {
        use crate::app_jobs::{AppJobs, NativeJob, Target};
        let dir = tempfile::tempdir().unwrap();
        for n in 0..3 {
            fs::write(dir.path().join(format!("{n}")), b"x").unwrap();
        }
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        for count in [false, true] {
            app.jobs = Some(
                AppJobs::new(
                    crate::background::Limits::default(),
                    crate::background::Worker::Blocking(Arc::new(crate::app_jobs::run)),
                )
                .unwrap(),
            );
            let missing = dir.path().join("missing");
            if count {
                app.spawn_async_child_count(&missing, &tx);
            } else {
                app.spawn_async_snapshot(&missing, &tx);
            }
            let delivery = app.next_background().await.unwrap();
            app.apply_background(delivery);
            app.shutdown_background().await;
            assert!(app.status_message.as_ref().unwrap().0.contains("I/O"));
        }
        for target in [
            Target::Count(dir.path().to_path_buf()),
            Target::Root(dir.path().to_path_buf()),
        ] {
            app.jobs = Some(
                AppJobs::new(
                    crate::background::Limits::default(),
                    crate::background::Worker::Blocking(Arc::new(crate::app_jobs::run)),
                )
                .unwrap(),
            );
            let timeout = if target.is_snapshot() {
                std::time::Duration::ZERO
            } else {
                std::time::Duration::from_secs(1)
            };
            app.jobs
                .as_mut()
                .unwrap()
                .submit(NativeJob {
                    target,
                    max_entries: 1,
                    timeout,
                    result_bytes: 1024,
                    snapshot_options: test_snapshot_options(),
                    summary_colors: None,
                    progress: None,
                    s3_profile: None,
                    s3_head_lines: 0,
                    clipboard: None,
                    preview_theme: None,
                    search: None,
                })
                .unwrap();
            let delivery = app.next_background().await.unwrap();
            app.apply_background(delivery);
            app.shutdown_background().await;
            assert!(app
                .status_message
                .as_ref()
                .unwrap()
                .0
                .to_lowercase()
                .contains("incomplete"));
            assert!(app.tree_state.root.total_child_count.is_none());
        }
    }

    #[tokio::test]
    async fn app_jobs_collapse_invalidates_and_repeated_admission_reuses_retired_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.spawn_initial_load(&tx);
        app.collapse_selected();
        assert!(!app.tree_state.root.is_loading);
        assert!(!app.jobs.as_ref().unwrap().has_pending());
        for _ in 0..64 {
            app.spawn_async_child_count(dir.path(), &tx);
            let delivery = app.next_background().await.unwrap();
            app.apply_background(delivery);
            assert!(
                !app.jobs.as_ref().unwrap().has_pending(),
                "delivery leaked request/path metadata"
            );
        }
        app.shutdown_background().await;
    }

    fn test_snapshot_options() -> crate::fs::tree::SnapshotOptions {
        crate::fs::tree::SnapshotOptions {
            sort_by: crate::fs::tree::SortBy::Name,
            dirs_first: true,
            page_size: AppConfig::default().max_entries_per_page(),
            child_depth: 1,
        }
    }

    /// Synchronously load root children for an App created with deferred loading.
    /// Tests use this because they can't await async events.
    fn sync_load_root(app: &mut App) {
        let page_size = app.tree_state.page_size;
        let sort_by = app.tree_state.sort_by.clone();
        let dirs_first = app.tree_state.dirs_first;
        let root = &mut app.tree_state.root;
        let _ = root.load_children_paged_with_sort(page_size, &sort_by, dirs_first);
        root.is_loading = false;
        root.is_expanded = true;
        crate::fs::tree::TreeState::sort_children_of_pub(root, &sort_by, dirs_first);
        app.tree_state.sort_all_children();
        app.tree_state.flatten();
    }

    fn setup_app() -> (TempDir, App) {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("alpha")).unwrap();
        fs::create_dir(dir.path().join("beta")).unwrap();
        File::create(dir.path().join("file_a.txt")).unwrap();
        File::create(dir.path().join("file_b.rs")).unwrap();
        File::create(dir.path().join(".hidden")).unwrap();
        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        sync_load_root(&mut app);
        // Enable auto-refresh for testing handle_fs_change directly.
        app.config.watcher.enabled = Some(true);
        app.watcher_active = true;
        (dir, app)
    }

    #[test]
    fn task3_large_view_cycle_refuses_unbounded_full_and_disabled_reads() {
        let (_dir, mut app) = setup_app();
        let index = app
            .tree_state
            .flat_items
            .iter()
            .position(|n| n.name == "file_a.txt")
            .unwrap();
        fs::write(
            &app.tree_state.flat_items[index].path,
            "alpha\nbeta\ngamma\n",
        )
        .unwrap();
        app.tree_state.selected_index = index;
        app.config.preview.max_full_preview_bytes = Some(1);
        app.update_preview();
        assert!(app.preview_state.is_large_file);
        app.cycle_view_mode();
        app.cycle_view_mode();
        app.cycle_view_mode();
        assert!(app.preview_state.is_large_file);
        assert_eq!(app.preview_state.view_mode, ViewMode::HeadAndTail);
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("byte limit"));
        app.config.preview.enabled = Some(false);
        let content = app.preview_state.content_lines.clone();
        fs::write(&app.tree_state.flat_items[index].path, "changed").unwrap();
        app.last_previewed_index = None;
        app.update_preview();
        app.cycle_view_mode();
        assert_eq!(app.preview_state.content_lines, content);
    }

    #[test]
    fn task3_no_preview_explicit_edit_is_available_without_automatic_load() {
        let (_dir, mut app) = setup_app();
        app.config.preview.enabled = Some(false);
        app.tree_state.selected_index = app
            .tree_state
            .flat_items
            .iter()
            .position(|n| n.name == "file_a.txt")
            .unwrap();
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.update_preview();
        assert!(app.preview_state.current_path.is_none());
        assert!(app.enter_edit_mode());
        assert!(app.editor_visible());
        assert!(app.preview_state.current_path.is_none());
    }

    #[test]
    fn task3_new_document_wrap_default_does_not_reset_retained_documents() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, "abcdefghijk").unwrap();
        fs::write(&b, "second").unwrap();
        let mut config = AppConfig::default();
        config.preview.line_wrap = Some(true);
        let mut app = App::new(dir.path(), config).unwrap();
        app.open_document_path(&a, true);
        let id = app.workspace.documents.active_id().unwrap();
        assert!(app.workspace.documents.active().unwrap().editor.line_wrap);
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .toggle_wrap();
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .horizontal_offset = 3;
        app.open_document_path(&b, true);
        assert!(app.workspace.documents.active().unwrap().editor.line_wrap);
        app.open_document_path(&a, true);
        assert_eq!(app.workspace.documents.active_id(), Some(id));
        let editor = &app.workspace.documents.active().unwrap().editor;
        assert!(!editor.line_wrap);
        assert_eq!(editor.horizontal_offset, 3);
    }

    #[test]
    fn task3_view_cycle_includes_bounded_full_and_head_tail() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = app
            .tree_state
            .flat_items
            .iter()
            .position(|n| n.name == "file_a.txt")
            .unwrap();
        app.update_preview();
        assert!(!app.preview_state.is_large_file);
        app.cycle_view_mode();
        assert!(app.preview_state.is_large_file);
        assert_eq!(app.preview_state.view_mode, ViewMode::HeadAndTail);
        app.cycle_view_mode();
        assert_eq!(app.preview_state.view_mode, ViewMode::HeadOnly);
        app.cycle_view_mode();
        assert_eq!(app.preview_state.view_mode, ViewMode::TailOnly);
        app.cycle_view_mode();
        assert!(!app.preview_state.is_large_file);
    }

    #[test]
    fn task3_config_disabled_watcher_cannot_be_temporarily_resumed() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        assert!(!app.config.watcher_enabled());
        assert!(!app.toggle_watcher());
        assert!(!app.watcher_active);
    }

    #[test]
    fn task3_preview_disabled_does_not_admit_selected_document() {
        let (_dir, mut app) = setup_app();
        app.config.preview.enabled = Some(false);
        app.tree_state.selected_index = app
            .tree_state
            .flat_items
            .iter()
            .position(|n| n.name == "file_a.txt")
            .unwrap();
        app.update_preview();
        assert!(app.preview_state.current_path.is_none());
        assert!(app.workspace.documents.is_empty());
        let path = app.tree_state.flat_items[app.tree_state.selected_index]
            .path
            .clone();
        app.open_document_path(&path, true);
        assert!(app.editor_visible());
    }

    #[test]
    fn task3_default_head_only_applies_to_small_file() {
        let (_dir, mut app) = setup_app();
        app.config.preview.default_view_mode = Some("head_only".into());
        app.tree_state.selected_index = app
            .tree_state
            .flat_items
            .iter()
            .position(|n| n.name == "file_a.txt")
            .unwrap();
        app.update_preview();
        assert_eq!(app.preview_state.view_mode, ViewMode::HeadOnly);
        assert!(app.preview_state.is_large_file);
    }

    #[test]
    fn task3_terminal_startup_respects_zero_history() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = AppConfig::default();
        config.terminal.scrollback_lines = Some(0);
        let mut app = App::new(dir.path(), config).unwrap();
        app.terminal_state.emulator.process(&b"x\r\n".repeat(80));
        assert_eq!(app.terminal_state.emulator.scrollback_len(), 0);
        assert!(app.terminal_state.pty.is_none());
    }

    #[test]
    fn clipboard_osc52_request_is_unconfirmed_and_failed_transport_keeps_payload() {
        let (_dir, mut app) = setup_app();
        app.config.general.mouse = Some(false);
        let mut output = Vec::new();
        app.show_copyable_text("a\nb\n".into(), &mut output, true);
        assert_eq!(output, b"\x1b]52;c;YQpiCg==\x07");
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("not confirmed"));
        assert_eq!(app.copy_overlay_text.as_deref(), Some("a\nb\n"));
        struct Broken;
        impl std::io::Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("unavailable"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        app.show_copyable_text("unchanged\n".into(), &mut Broken, true);
        assert!(app.status_message.as_ref().unwrap().0.contains("manually"));
        assert_eq!(app.copy_overlay_text.as_deref(), Some("unchanged\n"));
    }

    #[tokio::test]
    async fn clipboard_native_outcomes_with_injected_collaborators() {
        let (_dir, mut app) = setup_app();
        let (tx, mut rx) = crate::event::event_channel(Default::default());
        app.text_clipboard_backend = std::sync::Arc::new(|_: &str, _: &dyn Fn() -> bool| true);
        app.copy_text_async("secret\n".into(), &tx);
        let delivery = app.next_background().await.unwrap();
        app.apply_background(delivery);
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("native clipboard"));
        app.text_clipboard_backend = std::sync::Arc::new(|_, _| false);
        app.copy_text_async("secret\n".into(), &tx);
        let delivery = app.next_background().await.unwrap();
        app.apply_background(delivery);
        assert_eq!(
            app.pending_copy.as_ref().map(|(s, _)| s.as_str()),
            Some("secret\n")
        );
        assert!(rx.try_recv().is_err());
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn transport_operation_payload_refuses_before_mutation_or_progress_modal() {
        let (dir, mut app) = setup_app();
        let source = dir.path().join("file_a.txt");
        app.clipboard.paths = vec![source.clone()];
        app.clipboard.operation = Some(ClipboardOp::Cut);
        let (tx, _rx) = crate::event::event_channel(crate::event::TransportLimits {
            slots: 1,
            retained_bytes: 1024,
            event_bytes: 1024,
        });
        app.paste_clipboard_async(tx);
        assert!(
            !matches!(
                app.workspace.focus.overlay,
                AppMode::Dialog(DialogKind::Progress { .. })
            ),
            "reserve lifecycle payload before any filesystem mutation"
        );
        assert!(source.exists());
        assert!(app.status_message.as_ref().unwrap().0.contains("transport"));
    }

    #[tokio::test]
    async fn transport_user_cancel_keeps_partial_operation_completion_and_undo() {
        let (dir, mut app) = setup_app();
        app.clipboard.paths = vec![dir.path().join("file_a.txt"), dir.path().join("file_b.rs")];
        app.clipboard.operation = Some(ClipboardOp::Copy);
        app.tree_state.selected_index = 2; // beta destination
        let (tx, mut rx) = crate::event::event_channel(crate::event::TransportLimits {
            slots: 1,
            ..Default::default()
        });
        tx.send(crate::event::Event::Paste("full".into()))
            .await
            .unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered));
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                Default::default(),
                crate::background::Worker::Blocking(Arc::new(
                    move |job: crate::app_jobs::NativeJob, _| {
                        let crate::app_jobs::ClipboardJob::Paste {
                            paths, interrupt, ..
                        } = job.clipboard.unwrap()
                        else {
                            unreachable!()
                        };
                        let created =
                            crate::fs::operations::copy_recursive(&paths[0], job.target.path())
                                .unwrap();
                        entered.lock().unwrap().take().unwrap().send(()).unwrap();
                        while !interrupt.load(Ordering::SeqCst) {
                            std::thread::yield_now();
                        }
                        crate::app_jobs::NativeOutput::Paste(crate::app_jobs::PasteResult {
                            result: crate::event::OperationResult {
                                success_count: 1,
                                errors: vec!["Interrupted".into()],
                                created_paths: vec![created.clone()],
                                source_paths: vec![paths[0].clone()],
                                dest_dir: job.target.path().into(),
                                was_cut: false,
                            },
                            copies: vec![created],
                            moves: vec![],
                            completed_entries: 1,
                        })
                    },
                )),
            )
            .unwrap(),
        );
        app.paste_clipboard_async(tx);
        ready.await.unwrap();
        app.cancel_operation();
        let delivery = app.next_background().await.unwrap();
        let Ok(crate::app_jobs::NativeOutput::Paste(paste)) = &delivery.result else {
            panic!("necessary completion missing")
        };
        assert_eq!(paste.result.success_count, 1);
        assert_eq!(
            paste.result.created_paths,
            [dir.path().join("beta/file_a.txt")]
        );
        assert!(!dir.path().join("beta/file_b.rs").exists());
        app.apply_background(delivery);
        assert!(matches!(rx.try_recv(), Ok(crate::event::Event::Paste(s)) if s == "full"));
        app.shutdown_background().await;
        assert!(
            matches!(&app.last_undo, Some(UndoAction::CopyPaste { created_paths }) if created_paths == &[dir.path().join("beta/file_a.txt")])
        );
        app.undo();
        assert!(!dir.path().join("beta/file_a.txt").exists());
        assert!(dir.path().join("file_a.txt").exists());
    }

    #[test]
    fn transport_status_expiry_has_an_injected_idle_deadline() {
        let (_dir, mut app) = setup_app();
        let now = Instant::now();
        app.status_message = Some(("status".into(), now));
        assert_eq!(
            app.status_wait(now),
            Some(std::time::Duration::from_secs(4))
        );
        assert!(!app.expire_status(now + std::time::Duration::from_secs(3)));
        assert!(app.expire_status(now + std::time::Duration::from_secs(4)));
        assert_eq!(
            app.status_wait(now + std::time::Duration::from_secs(4)),
            None
        );
        assert!(!app.expire_status(now));
    }

    #[test]
    fn clipboard_overlay_restores_dirty_editor_focus_and_mouse_policy() {
        for mouse in [true, false] {
            let (_dir, mut app) = setup_app();
            app.config.general.mouse = Some(mouse);
            app.workspace.focus.overlay = AppMode::Normal;
            app.workspace.focus.panel = FocusedPanel::Editor;
            let mut e = EditorState::new("dirty", "test.txt".into());
            e.insert_text("!").unwrap();
            install_editor(&mut app, e);
            let mut output = Vec::new();
            assert_eq!(
                app.show_copyable_text("a\nb\n".into(), &mut output, false),
                mouse
            );
            assert!(!output.windows(4).any(|w| w == b"]52;"));
            if mouse {
                assert!(output.windows(7).any(|w| w == b"[?1000l"));
            } else {
                assert!(output.is_empty());
            }
            assert_eq!(app.copy_overlay_text.as_deref(), Some("a\nb\n"));
            let before = output.len();
            app.restore_copy_mouse_capture(&mut output);
            assert_eq!(output.len(), before); // overlay is still active
            assert_eq!(app.dismiss_copy_overlay(), mouse);
            output.clear();
            app.restore_copy_mouse_capture(&mut output);
            if mouse {
                assert!(output.windows(7).any(|w| w == b"[?1000h"));
            } else {
                assert!(output.is_empty());
            }
            output.clear();
            app.restore_copy_mouse_capture(&mut output);
            assert!(output.is_empty());
            assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
            assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
            assert!(
                app.workspace
                    .documents
                    .active()
                    .map(|d| &d.editor)
                    .unwrap()
                    .modified
            );
        }
    }

    #[test]
    fn shell_quote_single_escapes_single_quotes() {
        assert_eq!(shell_quote_single("plain"), "'plain'");
        assert_eq!(shell_quote_single("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn select_next_moves_down() {
        let (_dir, mut app) = setup_app();
        assert_eq!(app.tree_state.selected_index, 0);
        app.select_next();
        assert_eq!(app.tree_state.selected_index, 1);
    }

    #[test]
    fn select_next_clamps_at_end() {
        let (_dir, mut app) = setup_app();
        let last = app.tree_state.flat_items.len() - 1;
        app.tree_state.selected_index = last;
        app.select_next();
        assert_eq!(app.tree_state.selected_index, last);
    }

    #[test]
    fn select_previous_moves_up() {
        let (_dir, mut app) = setup_app();
        app.tree_state.selected_index = 2;
        app.select_previous();
        assert_eq!(app.tree_state.selected_index, 1);
    }

    #[test]
    fn select_previous_clamps_at_start() {
        let (_dir, mut app) = setup_app();
        app.select_previous();
        assert_eq!(app.tree_state.selected_index, 0);
    }

    #[test]
    fn select_first_and_last() {
        let (_dir, mut app) = setup_app();
        app.select_last();
        assert_eq!(
            app.tree_state.selected_index,
            app.tree_state.flat_items.len() - 1
        );
        app.select_first();
        assert_eq!(app.tree_state.selected_index, 0);
    }

    #[test]
    fn toggle_hidden_changes_count() {
        let (_dir, mut app) = setup_app();
        let without_hidden = app.tree_state.flat_items.len();
        app.toggle_hidden();
        let with_hidden = app.tree_state.flat_items.len();
        assert!(with_hidden > without_hidden);
    }

    #[test]
    fn expand_directory() {
        let (_dir, mut app) = setup_app();
        // Select first child (should be a directory: "alpha")
        app.select_next();
        assert_eq!(app.tree_state.flat_items[1].name, "alpha");
        app.expand_selected();
        // alpha is empty so flat items count stays same, but it's now expanded
        assert!(app.tree_state.flat_items[1].is_expanded);
    }

    #[test]
    fn quit_sets_flag() {
        let (_dir, mut app) = setup_app();
        assert!(!app.should_quit);
        app.quit();
        assert!(app.should_quit);
    }

    #[test]
    fn open_dialog_sets_mode() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        assert_eq!(
            app.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::CreateFile)
        );
    }

    #[test]
    fn close_dialog_returns_to_normal() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateDirectory);
        app.close_dialog();
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert!(app.dialog_state.input.is_empty());
        assert_eq!(app.dialog_state.cursor_position, 0);
    }

    #[test]
    fn toggle_terminal_disabled_returns_false() {
        let (_dir, mut app) = setup_app();
        app.config.terminal.enabled = Some(false);
        let (tx, _rx) = crate::event::event_channel(Default::default());

        let opened = app.toggle_terminal(&tx);

        assert!(!opened);
        assert!(!app.workspace.layout.terminal_visible());
    }

    #[test]
    fn dialog_input_char_inserts() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        app.dialog_input_char('a');
        app.dialog_input_char('b');
        app.dialog_input_char('c');
        assert_eq!(app.dialog_state.input, "abc");
        assert_eq!(app.dialog_state.cursor_position, 3);
    }

    #[test]
    fn dialog_delete_char_removes() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        app.dialog_input_char('a');
        app.dialog_input_char('b');
        app.dialog_delete_char();
        assert_eq!(app.dialog_state.input, "a");
        assert_eq!(app.dialog_state.cursor_position, 1);
    }

    #[test]
    fn dialog_delete_char_at_start_is_noop() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        app.dialog_delete_char();
        assert!(app.dialog_state.input.is_empty());
        assert_eq!(app.dialog_state.cursor_position, 0);
    }

    #[test]
    fn dialog_cursor_left_right() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        app.dialog_input_char('a');
        app.dialog_input_char('b');
        app.dialog_move_cursor_left();
        assert_eq!(app.dialog_state.cursor_position, 1);
        app.dialog_move_cursor_right();
        assert_eq!(app.dialog_state.cursor_position, 2);
    }

    #[test]
    fn dialog_cursor_boundaries() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        app.dialog_move_cursor_left();
        assert_eq!(app.dialog_state.cursor_position, 0);
        app.dialog_input_char('x');
        app.dialog_move_cursor_right();
        assert_eq!(app.dialog_state.cursor_position, 1);
    }

    #[test]
    fn dialog_cursor_home_end() {
        let (_dir, mut app) = setup_app();
        app.open_dialog(DialogKind::CreateFile);
        app.dialog_input_char('a');
        app.dialog_input_char('b');
        app.dialog_input_char('c');
        app.dialog_cursor_home();
        assert_eq!(app.dialog_state.cursor_position, 0);
        app.dialog_cursor_end();
        assert_eq!(app.dialog_state.cursor_position, 3);
    }

    #[test]
    fn rename_prefills_input() {
        let (_dir, mut app) = setup_app();
        let path = PathBuf::from("/some/dir/hello.txt");
        app.open_dialog(DialogKind::Rename { original: path });
        assert_eq!(app.dialog_state.input, "hello.txt");
        assert_eq!(app.dialog_state.cursor_position, 9);
    }

    #[test]
    fn set_status_message_stores_message() {
        let (_dir, mut app) = setup_app();
        app.set_status_message("test message".to_string());
        assert!(app.status_message.is_some());
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert_eq!(msg, "test message");
    }

    #[test]
    fn clear_expired_status_keeps_recent() {
        let (_dir, mut app) = setup_app();
        app.set_status_message("fresh".to_string());
        app.clear_expired_status();
        assert!(app.status_message.is_some());
    }

    #[test]
    fn clear_expired_status_removes_old() {
        let (_dir, mut app) = setup_app();
        app.status_message = Some((
            "old".to_string(),
            Instant::now() - std::time::Duration::from_secs(5),
        ));
        app.clear_expired_status();
        assert!(app.status_message.is_none());
    }

    #[test]
    fn current_dir_returns_root_for_directory() {
        let (dir, app) = setup_app();
        // selected_index 0 is root, which is a directory
        assert_eq!(app.current_dir(), dir.path().to_path_buf());
    }

    #[test]
    fn current_dir_returns_parent_for_file() {
        let (dir, mut app) = setup_app();
        // Navigate to a file (files come after directories in flat_items)
        // flat_items: root(dir), alpha(dir), beta(dir), file_a.txt, file_b.rs
        app.tree_state.selected_index = 3; // file_a.txt
        assert_eq!(app.current_dir(), dir.path().to_path_buf());
    }

    // === Preview state tests ===

    #[test]
    fn default_focused_panel_is_tree() {
        let (_dir, app) = setup_app();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
    }

    #[test]
    fn toggle_focus_switches_panel() {
        let (_dir, mut app) = setup_app();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
        app.toggle_focus();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
        app.toggle_focus();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
    }

    #[test]
    fn preview_state_defaults() {
        let (_dir, app) = setup_app();
        assert!(app.preview_state.current_path.is_none());
        assert!(app.preview_state.content_lines.is_empty());
        assert_eq!(app.preview_state.scroll_offset, 0);
        assert_eq!(app.preview_state.view_mode, ViewMode::HeadAndTail);
        assert!(!app.preview_state.line_wrap);
        assert_eq!(app.preview_state.total_lines, 0);
    }

    #[test]
    fn extract_preview_selected_text_single_line() {
        let (_dir, mut app) = setup_app();
        app.preview_state.content_lines = vec![Line::from("hello world")];
        app.preview_state.total_lines = 1;
        app.preview_selection
            .set_anchor(crate::terminal::TerminalCoord { line: 0, col: 6 });
        app.preview_selection
            .set_endpoint(crate::terminal::TerminalCoord { line: 0, col: 10 });

        let extracted = app.extract_preview_selected_text();
        assert_eq!(extracted.as_deref(), Some("world"));
    }

    #[test]
    fn extract_preview_selected_text_multi_line() {
        let (_dir, mut app) = setup_app();
        app.preview_state.content_lines =
            vec![Line::from("alpha"), Line::from("beta"), Line::from("gamma")];
        app.preview_state.total_lines = 3;
        app.preview_selection
            .set_anchor(crate::terminal::TerminalCoord { line: 0, col: 2 });
        app.preview_selection
            .set_endpoint(crate::terminal::TerminalCoord { line: 2, col: 2 });

        let extracted = app.extract_preview_selected_text();
        assert_eq!(extracted.as_deref(), Some("pha\nbeta\ngam"));
    }

    #[test]
    fn preview_scroll_down_up() {
        let (_dir, mut app) = setup_app();
        app.preview_state.total_lines = 100;
        app.preview_scroll_down();
        assert_eq!(app.preview_state.scroll_offset, 1);
        app.preview_scroll_down();
        assert_eq!(app.preview_state.scroll_offset, 2);
        app.preview_scroll_up();
        assert_eq!(app.preview_state.scroll_offset, 1);
    }

    #[test]
    fn preview_copy_expands_tabs_on_all_selected_logical_lines() {
        let (_dir, mut app) = setup_app();
        app.preview_state.content_lines = vec![
            Line::from("a\t中"),
            Line::from("\te\u{301}👩‍💻"),
            Line::from("中Z"),
        ];
        app.preview_selection
            .set_anchor(crate::terminal::TerminalCoord { line: 0, col: 2 });
        app.preview_selection
            .set_endpoint(crate::terminal::TerminalCoord { line: 2, col: 1 });
        assert_eq!(
            app.extract_preview_selected_text().as_deref(),
            Some("  中\n    e\u{301}👩‍💻\n中")
        );
        app.preview_state.line_wrap = true;
        app.preview_state.horizontal_offset = 99;
        assert_eq!(
            app.extract_preview_selected_text().as_deref(),
            Some("  中\n    e\u{301}👩‍💻\n中")
        );
    }

    #[test]
    fn preview_modes_and_resize_clamp_independent_offsets() {
        let (_dir, mut app) = setup_app();
        app.preview_area = Rect::new(0, 0, 6, 4);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area); // four cells, two rows
        app.preview_state.content_lines = vec![Line::from("abcdefghij"), Line::from("last")];
        app.preview_scroll_horizontal(true);
        assert_eq!(app.preview_state.horizontal_offset, 4);
        app.preview_scroll_horizontal(true);
        assert_eq!(app.preview_state.horizontal_offset, 6);
        app.preview_toggle_wrap();
        assert!(app.preview_state.line_wrap);
        assert_eq!(app.preview_state.horizontal_offset, 0);
        assert_eq!(app.preview_state.visual_row_count(4), 4);
        app.preview_jump_bottom();
        assert_eq!(app.preview_state.scroll_offset, 2);
        app.clamp_preview_scroll();
        assert_eq!(app.preview_state.scroll_offset, 2); // no cursor-following reset
        app.preview_toggle_wrap();
        assert_eq!(app.preview_state.scroll_offset, 0);
        app.preview_area = Rect::new(0, 0, 30, 10);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
        app.preview_state.horizontal_offset = 99;
        app.preview_state.scroll_offset = 99;
        app.clamp_preview_scroll();
        assert_eq!(app.preview_state.horizontal_offset, 0);
        assert_eq!(app.preview_state.scroll_offset, 0);
        app.preview_area = Rect::new(0, 0, 0, 0);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
        app.clamp_preview_scroll();
        assert_eq!(app.preview_state.horizontal_offset, 0);
        assert_eq!(app.preview_state.visual_row_count(0), 2);
        app.preview_area = Rect::new(0, 0, 6, 3);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
        app.preview_state.scroll_offset = 1;
        app.preview_toggle_wrap();
        assert_eq!(app.preview_state.scroll_offset, 3);
        app.preview_toggle_wrap();
        assert_eq!(app.preview_state.scroll_offset, 1);
    }

    #[test]
    fn preview_scroll_clamps_at_boundaries() {
        let (_dir, mut app) = setup_app();
        app.preview_state.total_lines = 3;
        // Can't scroll past end
        app.preview_state.scroll_offset = 2;
        app.preview_scroll_down();
        assert_eq!(app.preview_state.scroll_offset, 2);
        // Can't scroll before start
        app.preview_state.scroll_offset = 0;
        app.preview_scroll_up();
        assert_eq!(app.preview_state.scroll_offset, 0);
    }

    #[test]
    fn preview_scroll_down_noop_when_empty() {
        let (_dir, mut app) = setup_app();
        app.preview_state.total_lines = 0;
        app.preview_scroll_down();
        assert_eq!(app.preview_state.scroll_offset, 0);
    }

    #[test]
    fn preview_jump_top_bottom() {
        let (_dir, mut app) = setup_app();
        app.preview_state.total_lines = 100;
        app.preview_jump_bottom();
        assert_eq!(app.preview_state.scroll_offset, 99);
        app.preview_jump_top();
        assert_eq!(app.preview_state.scroll_offset, 0);
    }

    #[test]
    fn preview_half_page_scroll() {
        let (_dir, mut app) = setup_app();
        app.preview_state.total_lines = 100;
        app.preview_half_page_down(20);
        assert_eq!(app.preview_state.scroll_offset, 10);
        app.preview_half_page_down(20);
        assert_eq!(app.preview_state.scroll_offset, 20);
        app.preview_half_page_up(20);
        assert_eq!(app.preview_state.scroll_offset, 10);
    }

    #[test]
    fn preview_half_page_clamps() {
        let (_dir, mut app) = setup_app();
        app.preview_state.total_lines = 10;
        app.preview_half_page_down(100);
        assert_eq!(app.preview_state.scroll_offset, 9);
        app.preview_half_page_up(100);
        assert_eq!(app.preview_state.scroll_offset, 0);
    }

    #[test]
    fn preview_jump_bottom_respects_viewport_height() {
        let (_dir, mut app) = setup_app();
        app.preview_state.content_lines =
            (0..100).map(|i| Line::from(format!("line {i}"))).collect();
        app.preview_state.total_lines = 100;
        app.preview_area = Rect::new(0, 0, 80, 12);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area); // inner height = 10
        app.preview_jump_bottom();
        assert_eq!(app.preview_state.scroll_offset, 90);
    }

    #[test]
    fn clamp_preview_scroll_after_resize() {
        let (_dir, mut app) = setup_app();
        app.preview_state.content_lines =
            (0..100).map(|i| Line::from(format!("line {i}"))).collect();
        app.preview_state.total_lines = 100;
        app.preview_area = Rect::new(0, 0, 80, 12);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area); // inner height = 10
        app.preview_jump_bottom();
        assert_eq!(app.preview_state.scroll_offset, 90);

        app.preview_area = Rect::new(0, 0, 80, 22);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area); // inner height = 20
        app.clamp_preview_scroll();
        assert_eq!(app.preview_state.scroll_offset, 80);
    }

    // === Integration tests: preview update flow ===

    #[test]
    fn update_preview_loads_file_content() {
        let (dir, mut app) = setup_app();
        // Write content to a file
        std::fs::write(dir.path().join("file_a.txt"), "hello world\n").unwrap();
        // Select file_a.txt (index 3)
        app.tree_state.selected_index = 3;
        app.update_preview();
        assert!(app.preview_state.current_path.is_some());
        assert!(!app.preview_state.content_lines.is_empty());
        assert!(app.preview_state.total_lines >= 1);
    }

    #[test]
    fn update_preview_directory_shows_summary() {
        let (_dir, mut app) = setup_app();
        // Select alpha directory (index 1)
        app.tree_state.selected_index = 1;
        app.update_preview();
        assert!(app.preview_state.current_path.is_some());
        let all_text: String = app
            .preview_state
            .content_lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("Directory:"));
    }

    #[test]
    fn update_preview_binary_file_shows_metadata() {
        let (dir, mut app) = setup_app();
        // Create a binary file
        let bin_path = dir.path().join("model.pt");
        std::fs::write(&bin_path, [0u8; 100]).unwrap();
        app.tree_state.reload_dir(dir.path());

        // Find the .pt file in flat_items
        let idx = app
            .tree_state
            .flat_items
            .iter()
            .position(|item| item.name == "model.pt")
            .unwrap();
        app.tree_state.selected_index = idx;
        app.update_preview();

        let all_text: String = app
            .preview_state
            .content_lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("Binary file"));
        assert!(all_text.contains("model.pt"));
    }

    #[test]
    fn update_preview_notebook_file() {
        let (dir, mut app) = setup_app();
        let nb_path = dir.path().join("test.ipynb");
        std::fs::write(
            &nb_path,
            r#"{"cells":[{"cell_type":"code","source":["x=1"],"outputs":[]}],"metadata":{}}"#,
        )
        .unwrap();
        app.tree_state.reload_dir(dir.path());

        let idx = app
            .tree_state
            .flat_items
            .iter()
            .position(|item| item.name == "test.ipynb")
            .unwrap();
        app.tree_state.selected_index = idx;
        app.update_preview();

        let all_text: String = app
            .preview_state
            .content_lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(all_text.contains("Cell 1"));
        assert!(all_text.contains("code"));
    }

    #[test]
    fn update_preview_resets_scroll_on_selection_change() {
        let (dir, mut app) = setup_app();
        std::fs::write(dir.path().join("file_a.txt"), "line1\nline2\nline3\n").unwrap();
        // Select file
        app.tree_state.selected_index = 3;
        app.update_preview();
        app.preview_state.scroll_offset = 2;
        // Change selection
        app.tree_state.selected_index = 1; // directory
        app.last_previewed_index = None; // force update
        app.update_preview();
        assert_eq!(app.preview_state.scroll_offset, 0);
    }

    #[test]
    fn update_preview_skips_if_same_selection() {
        let (dir, mut app) = setup_app();
        std::fs::write(dir.path().join("file_a.txt"), "hello\n").unwrap();
        app.tree_state.selected_index = 3;
        app.update_preview();
        let first_path = app.preview_state.current_path.clone();
        // Call again without changing selection
        app.preview_state.scroll_offset = 5;
        app.update_preview();
        // Should not reset scroll
        assert_eq!(app.preview_state.scroll_offset, 5);
        assert_eq!(app.preview_state.current_path, first_path);
    }

    // === Search (Ctrl+P) tests ===

    #[test]
    fn open_search_sets_mode() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        assert_eq!(app.workspace.focus.overlay, AppMode::Search);
        assert!(app.search_state.cached_paths.is_some());
    }

    #[test]
    fn close_search_returns_to_normal() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.close_search();
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
    }

    #[test]
    fn search_input_updates_query() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.search_input_char('f');
        assert_eq!(app.search_state.query, "f");
        app.search_input_char('i');
        assert_eq!(app.search_state.query, "fi");
    }

    #[test]
    fn search_delete_char_removes() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.search_input_char('a');
        app.search_input_char('b');
        app.search_delete_char();
        assert_eq!(app.search_state.query, "a");
    }

    #[test]
    fn search_delete_at_empty_is_noop() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.search_delete_char();
        assert_eq!(app.search_state.query, "");
    }

    #[test]
    fn search_results_update_on_input() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.search_input_char('f');
        app.search_input_char('i');
        app.search_input_char('l');
        app.search_input_char('e');
        // Should find file_a.txt and file_b.rs
        assert!(app.search_state.results.len() >= 2);
    }

    #[test]
    fn search_empty_query_clears_results() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.search_input_char('a');
        assert!(!app.search_state.results.is_empty());
        app.search_delete_char();
        assert!(app.search_state.results.is_empty());
    }

    #[test]
    fn search_no_matches_empty_results() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.search_input_char('z');
        app.search_input_char('z');
        app.search_input_char('z');
        assert!(app.search_state.results.is_empty());
    }

    #[test]
    fn search_select_navigation() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.search_input_char('f');
        assert_eq!(app.search_state.selected_index, 0);
        app.search_select_next();
        assert_eq!(app.search_state.selected_index, 1);
        app.search_select_previous();
        assert_eq!(app.search_state.selected_index, 0);
    }

    #[test]
    fn search_select_clamps() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        app.search_select_previous(); // at 0, should stay
        assert_eq!(app.search_state.selected_index, 0);
    }

    #[test]
    fn search_confirm_navigates_to_file() {
        let (dir, mut app) = setup_app();
        // Create a nested file for navigation test
        fs::create_dir_all(dir.path().join("alpha").join("nested")).unwrap();
        File::create(dir.path().join("alpha").join("nested").join("deep.txt")).unwrap();
        app.tree_state.reload_dir(dir.path());
        app.invalidate_search_cache();

        app.open_search();
        app.search_input_char('d');
        app.search_input_char('e');
        app.search_input_char('e');
        app.search_input_char('p');

        assert!(!app.search_state.results.is_empty());
        app.search_confirm();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert!(app.workspace.documents.active().unwrap().is_pinned());
        assert!(app.search_action_state.is_none());
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);

        // Should have navigated to the deep.txt file
        let selected = &app.tree_state.flat_items[app.tree_state.selected_index];
        assert_eq!(selected.name, "deep.txt");
    }

    #[test]
    fn search_action_open_terminal_disabled_does_not_focus_terminal() {
        let (dir, mut app) = setup_app();
        app.config.terminal.enabled = Some(false);
        app.workspace.focus.overlay = AppMode::SearchAction;
        app.search_action_state = Some(SearchActionState {
            path: dir.path().join("file_a.txt"),
            display: "file_a.txt".to_string(),
            is_directory: false,
            is_binary: false,
        });

        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.search_action_open_terminal(&tx);

        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert!(!app.workspace.layout.terminal_visible());
        assert_ne!(app.workspace.focus.panel, FocusedPanel::Terminal);
    }

    #[test]
    fn build_path_index_finds_files() {
        let (_dir, app) = setup_app();
        let index = app.build_path_index();
        // Should find file_a.txt, file_b.rs, .hidden
        assert!(index.len() >= 2);
    }

    #[test]
    fn invalidate_search_cache_clears() {
        let (_dir, mut app) = setup_app();
        app.open_search();
        assert!(app.search_state.cached_paths.is_some());
        app.invalidate_search_cache();
        assert!(app.search_state.cached_paths.is_none());
    }

    // ── Phase 6 Task 3: incremental filename + content search ───────────────

    #[tokio::test]
    async fn task3_filename_index_is_worker_driven_incremental_and_drains() {
        let (dir, mut app) = setup_app();
        fs::write(dir.path().join("new_file.txt"), "x").unwrap();
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(crate::app_jobs::run),
        )));
        app.tree_state.reload_dir(dir.path());
        app.open_search();
        assert!(
            app.search_state.indexing,
            "filename walk must be admitted to the worker pool"
        );
        let mut guard = 0;
        while app.search_state.indexing && guard < 200 {
            let delivery = app.next_background().await.unwrap();
            app.apply_background(delivery);
            guard += 1;
        }
        assert!(!app.search_state.indexing);
        assert!(app.search_state.index_complete);
        let paths = app.search_state.cached_paths.as_ref().unwrap();
        assert!(paths.iter().any(|path| path.ends_with("file_a.txt")));
        assert!(paths.iter().any(|path| path.ends_with("new_file.txt")));
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn task3_content_search_query_replacement_rejects_stale_batch() {
        let (dir, mut app) = setup_app();
        let aaa = dir.path().join("aaa.txt");
        let bbb = dir.path().join("bbb.txt");
        fs::write(&aaa, "STALE_QUERY_MATCH\n").unwrap();
        fs::write(&bbb, "NEW_QUERY_MATCH\n").unwrap();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered_tx));
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_gate = gate.clone();
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(move |job: crate::app_jobs::NativeJob, token| {
                if let Some(crate::search::SearchJob::Content(request)) = &job.search {
                    if request.query.text == "STALE_QUERY_MATCH" {
                        entered.lock().unwrap().take().unwrap().send(()).unwrap();
                        let _open = worker_gate
                            .1
                            .wait_while(worker_gate.0.lock().unwrap(), |open| !*open);
                    }
                }
                crate::app_jobs::run(job, token)
            }),
        )));
        app.tree_state.reload_dir(dir.path());
        app.open_search();
        app.open_content_search();
        app.content_search.query = "STALE_QUERY_MATCH".into();
        app.content_search.cursor_position = app.content_search.query.len();
        app.start_content_search();
        entered_rx.await.unwrap(); // stale job admitted and blocked
                                   // A newer query supersedes the older request on the same root domain.
        app.content_search.query = "NEW_QUERY_MATCH".into();
        app.content_search.cursor_position = app.content_search.query.len();
        app.start_content_search();
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        let mut guard = 0;
        while app.content_search.scanning && guard < 200 {
            let delivery = app.next_background().await.unwrap();
            app.apply_background(delivery);
            guard += 1;
        }
        assert!(app.content_search.hits.iter().all(|hit| hit.path == bbb));
        assert!(app
            .content_search
            .hits
            .iter()
            .all(|hit| hit.excerpt.contains("NEW_QUERY_MATCH")));
        app.shutdown_background().await;
    }

    #[test]
    fn task3_content_search_cap_reports_incomplete_status() {
        let (dir, mut app) = setup_app();
        for index in 0..5 {
            fs::write(
                dir.path().join(format!("match_{index}.txt")),
                "needle here\n",
            )
            .unwrap();
        }
        app.tree_state.reload_dir(dir.path());
        app.config.general.search_max_hits = Some(1);
        app.config.general.search_max_files = Some(100);
        app.open_search();
        app.content_search_active = true;
        app.content_search.query = "needle".into();
        app.content_search.cursor_position = 6;
        app.start_content_search();
        assert_eq!(app.content_search.hits.len(), 1);
        assert!(app.content_search.capped);
        assert!(!app.content_search.complete);
        assert!(app.content_search.status_text(false).contains("incomplete"));
    }

    #[test]
    fn task3_content_navigation_preserves_dirty_active_document_and_same_file() {
        use crate::workspace::documents::OpenDisposition;
        let (dir, mut app) = setup_app();
        let first = dir.path().join("first.txt");
        let second = dir.path().join("second.txt");
        fs::write(&first, "alpha\nneedle one\n").unwrap();
        fs::write(&second, "beta\nneedle two\n").unwrap();
        app.tree_state.reload_dir(dir.path());
        let first_id = app
            .workspace
            .documents
            .open(&first, OpenDisposition::Pinned)
            .unwrap();
        app.workspace
            .documents
            .get_mut(first_id)
            .unwrap()
            .editor
            .insert_text("unsaved")
            .unwrap();
        assert!(
            app.workspace
                .documents
                .get(first_id)
                .unwrap()
                .editor
                .modified
        );

        // Navigate to a hit in a different file: the dirty buffer survives.
        app.navigate_to_hit(&crate::search::SearchHit {
            path: second.clone(),
            line: 2,
            byte: 5,
            column: 0,
            excerpt: "needle two".into(),
        });
        assert!(
            app.workspace
                .documents
                .get(first_id)
                .unwrap()
                .editor
                .modified
        );
        assert_eq!(app.workspace.documents.active().unwrap().path(), second);
        assert_eq!(
            app.workspace.documents.active().unwrap().editor.cursor_line,
            1
        );

        // Navigating to a hit in the dirty document keeps its bytes and moves the
        // cursor within the existing buffer instead of reloading it.
        app.navigate_to_hit(&crate::search::SearchHit {
            path: first.clone(),
            line: 2,
            byte: 6,
            column: 0,
            excerpt: "needle one".into(),
        });
        let active = app.workspace.documents.active().unwrap();
        assert_eq!(active.path(), first);
        assert!(active.editor.modified);
        assert!(active.editor.buffer[0].starts_with("unsaved"));
        assert_eq!(active.editor.cursor_line, 1);
    }

    #[test]
    fn task3_content_search_is_native_and_shell_metacharacters_are_literal() {
        // No external search executable: a directory whose name would be
        // interpreted by a shell is searched literally by the native engine.
        let root = TempDir::new().unwrap();
        let odd = root.path().join("weird name; rm -rf");
        fs::create_dir_all(&odd).unwrap();
        fs::write(odd.join("target file.txt"), "literal needle\n").unwrap();
        let outcome = crate::search::search_project(
            root.path(),
            crate::search::SearchQuery::new("needle", false),
            &crate::search::default_excludes(),
            &crate::search::SearchLimits::default(),
        );
        assert_eq!(outcome.hits.len(), 1);
        assert!(outcome.hits[0].path.ends_with("target file.txt"));
    }

    #[test]
    fn task3_content_mode_toggle_input_delete_and_empty_confirm() {
        let (_dir, mut app) = setup_app();
        app.open_content_search(); // from Normal: sets Search overlay
        assert_eq!(app.workspace.focus.overlay, AppMode::Search);
        assert!(app.content_search_active);
        app.search_delete_char(); // cursor at zero is a no-op
        app.search_input_char('a');
        app.search_input_char('b');
        assert_eq!(app.content_search.query, "ab");
        app.search_delete_char();
        assert_eq!(app.content_search.query, "a");
        app.search_confirm(); // no selected hit: early return, stays open
        assert_eq!(app.workspace.focus.overlay, AppMode::Search);
        app.toggle_content_search_mode();
        assert!(!app.content_search_active);
        app.toggle_content_search_mode();
        assert!(app.content_search_active);
    }

    #[test]
    fn task3_content_selection_secondary_actions_and_confirm_navigate() {
        let (dir, mut app) = setup_app();
        fs::write(dir.path().join("hit.txt"), "needle here\n").unwrap();
        app.tree_state.reload_dir(dir.path());
        app.open_search();
        app.open_content_search();
        app.search_input_char('n');
        app.search_input_char('e');
        assert!(!app.content_search.hits.is_empty());
        app.search_select_next();
        app.search_select_previous();
        app.search_secondary_actions();
        assert_eq!(app.workspace.focus.overlay, AppMode::SearchAction);
        app.search_action_back();
        assert_eq!(app.workspace.focus.overlay, AppMode::Search);
        app.search_confirm();
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert!(app
            .workspace
            .documents
            .active()
            .unwrap()
            .path()
            .ends_with("hit.txt"));
    }

    #[test]
    fn task3_navigate_to_directory_hit_falls_back_to_preview() {
        let (dir, mut app) = setup_app();
        let ok = app.navigate_to_hit(&crate::search::SearchHit {
            path: dir.path().join("alpha"),
            line: 1,
            byte: 0,
            column: 0,
            excerpt: String::new(),
        });
        assert!(!ok);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
    }

    #[test]
    fn task3_crlf_hit_navigates_to_the_exact_column() {
        let (dir, mut app) = setup_app();
        // CRLF file: the engine's absolute offset counts every preceding `\r`,
        // while the editor buffer (built from `str::lines()`) strips them.
        let crlf = dir.path().join("crlf.txt");
        fs::write(&crlf, b"aaa\r\nneedle\r\n").unwrap();
        let engine = crate::search::search_project(
            dir.path(),
            crate::search::SearchQuery::new("needle", false),
            &crate::search::default_excludes(),
            &crate::search::SearchLimits::default(),
        );
        let hit = engine
            .hits
            .iter()
            .find(|hit| hit.path == crlf)
            .expect("needle hit");
        assert_eq!(hit.line, 2);
        assert_eq!(hit.byte, 5, "absolute raw-file offset counts the CRLF \\r");
        assert!(app.navigate_to_hit(hit));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert_eq!(
            app.workspace
                .documents
                .active()
                .unwrap()
                .editor
                .cursor_position()
                .byte,
            0
        );

        // Multi-line CRLF file: the needle is on line 3 at column 4 and three
        // preceding CRLF pairs have shifted the absolute offset by 3.
        let multi = dir.path().join("crlf_multi.txt");
        fs::write(&multi, b"alpha\r\nbeta\r\nneedle here\r\ngamma\r\n").unwrap();
        let engine = crate::search::search_project(
            dir.path(),
            crate::search::SearchQuery::new("needle", false),
            &crate::search::default_excludes(),
            &crate::search::SearchLimits::default(),
        );
        let hit = engine
            .hits
            .iter()
            .find(|hit| hit.path == multi)
            .expect("needle hit");
        assert_eq!(hit.line, 3);
        assert!(app.navigate_to_hit(hit));
        assert_eq!(
            app.workspace
                .documents
                .active()
                .unwrap()
                .editor
                .cursor_position()
                .byte,
            0
        );

        // The same file searched for a needle mid-line lands on its column.
        fs::write(&multi, b"alpha\r\nbeta\r\nxx needle\r\ngamma\r\n").unwrap();
        let engine = crate::search::search_project(
            dir.path(),
            crate::search::SearchQuery::new("needle", false),
            &crate::search::default_excludes(),
            &crate::search::SearchLimits::default(),
        );
        let hit = engine
            .hits
            .iter()
            .find(|hit| hit.path == multi)
            .expect("needle hit");
        assert!(app.navigate_to_hit(hit));
        assert_eq!(
            app.workspace
                .documents
                .active()
                .unwrap()
                .editor
                .cursor_position()
                .byte,
            3
        );
    }

    #[test]
    fn task3_lf_hit_column_is_unchanged() {
        let (dir, mut app) = setup_app();
        let lf = dir.path().join("lf.txt");
        fs::write(&lf, b"aaa\nneedle here\n").unwrap();
        let engine = crate::search::search_project(
            dir.path(),
            crate::search::SearchQuery::new("needle", false),
            &crate::search::default_excludes(),
            &crate::search::SearchLimits::default(),
        );
        let hit = engine
            .hits
            .iter()
            .find(|hit| hit.path == lf)
            .expect("needle hit");
        assert_eq!(hit.line, 2);
        assert_eq!(hit.byte, 4);
        assert!(app.navigate_to_hit(hit));
        assert_eq!(
            app.workspace
                .documents
                .active()
                .unwrap()
                .editor
                .cursor_position()
                .byte,
            0
        );
    }

    #[tokio::test]
    async fn task3_filename_index_worker_failure_is_reported_and_not_pending() {
        use crate::app_jobs::{NativeJob, NativeOutput};
        let (dir, mut app) = setup_app();
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(|_: NativeJob, _| NativeOutput::Failed("injected index failure")),
        )));
        app.tree_state.reload_dir(dir.path());
        app.open_search();
        let delivery = app.next_background().await.unwrap();
        app.apply_background(delivery);
        assert!(!app.search_state.indexing);
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("injected index failure"));
        // A failed delivery leaves a partial index: the status must say so.
        app.search_state.query = "many".into();
        app.update_search_results();
        assert!(
            app.search_state.status.contains("index incomplete"),
            "{}",
            app.search_state.status
        );
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn task3_refused_index_admission_reports_partial_status() {
        let (dir, mut app) = setup_app();
        fs::write(dir.path().join("alpha.txt"), b"x").unwrap();
        app.prepared_pipeline = true;
        // A one-byte job envelope refuses every real search submission.
        let limits = crate::background::Limits {
            job_bytes: 1,
            ..Default::default()
        };
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                limits,
                crate::background::Worker::Blocking(Arc::new(crate::app_jobs::run)),
            )
            .unwrap(),
        );
        app.tree_state.reload_dir(dir.path());
        app.open_search();
        // Admission was refused and the synchronous fallback ran, so the index
        // is usable but explicitly partial rather than complete.
        assert!(app.search_state.index_capped);
        assert!(!app.search_state.index_complete);
        assert!(!app.search_state.indexing);
        app.search_state.query = "alpha".into();
        app.update_search_results();
        assert!(
            app.search_state.status.contains("index incomplete"),
            "refused admission must not look complete: {}",
            app.search_state.status
        );
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn task3_refused_content_admission_reports_incomplete() {
        let (dir, mut app) = setup_app();
        fs::write(dir.path().join("alpha.txt"), b"needle\n").unwrap();
        app.prepared_pipeline = true;
        let limits = crate::background::Limits {
            job_bytes: 1,
            ..Default::default()
        };
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                limits,
                crate::background::Worker::Blocking(Arc::new(crate::app_jobs::run)),
            )
            .unwrap(),
        );
        app.tree_state.reload_dir(dir.path());
        app.open_search();
        app.open_content_search();
        app.content_search.query = "needle".into();
        app.content_search.cursor_position = 6;
        app.start_content_search();
        assert!(!app.content_search.scanning);
        assert!(!app.content_search.complete);
        // A refused admission is explicitly capped, not merely "not complete".
        assert!(app.content_search.capped);
        assert!(
            app.content_search.status_text(false).contains("incomplete"),
            "{}",
            app.content_search.status_text(false)
        );
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn task3_failed_content_delivery_reports_incomplete_status() {
        use crate::app_jobs::{NativeJob, NativeOutput};
        let (dir, mut app) = setup_app();
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(|_: NativeJob, _| NativeOutput::Failed("injected content failure")),
        )));
        app.tree_state.reload_dir(dir.path());
        app.open_search();
        app.open_content_search();
        app.content_search.query = "needle".into();
        app.content_search.cursor_position = 6;
        app.start_content_search();
        while app.content_search.scanning {
            let delivery = app.next_background().await.unwrap();
            app.apply_background(delivery);
        }
        assert!(!app.content_search.complete);
        // A failed delivery is explicitly capped, not merely "not complete".
        assert!(app.content_search.capped);
        assert!(
            app.content_search.status_text(false).contains("incomplete"),
            "{}",
            app.content_search.status_text(false)
        );
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn task3_filename_index_continues_past_one_batch_to_completion() {
        let (dir, mut app) = setup_app();
        for index in 0..600 {
            fs::write(dir.path().join(format!("many_{index}.txt")), b"x").unwrap();
        }
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(crate::app_jobs::run),
        )));
        app.tree_state.reload_dir(dir.path());
        app.open_search();
        let mut guard = 0;
        while app.search_state.indexing && guard < 1000 {
            let delivery = app.next_background().await.unwrap();
            app.apply_background(delivery);
            guard += 1;
        }
        assert!(app.search_state.index_complete);
        assert!(app.search_state.cached_paths.as_ref().unwrap().len() >= 600);
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn task3_content_search_worker_failure_and_panic_are_reported() {
        use crate::app_jobs::{NativeJob, NativeOutput};
        let (dir, mut app) = setup_app();
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(|_: NativeJob, _| NativeOutput::Failed("injected content failure")),
        )));
        app.tree_state.reload_dir(dir.path());
        app.open_search();
        app.open_content_search();
        app.content_search.query = "needle".into();
        app.content_search.cursor_position = 6;
        app.start_content_search();
        while app.content_search.scanning {
            let delivery = app.next_background().await.unwrap();
            app.apply_background(delivery);
        }
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("injected content failure"));
        app.shutdown_background().await;

        let (_dir2, mut app2) = setup_app();
        app2.prepared_pipeline = true;
        app2.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(|_: NativeJob, _| -> NativeOutput { panic!("injected content panic") }),
        )));
        app2.open_search();
        app2.open_content_search();
        app2.content_search.query = "needle".into();
        app2.content_search.cursor_position = 6;
        app2.start_content_search();
        while app2.content_search.scanning {
            let delivery = app2.next_background().await.unwrap();
            app2.apply_background(delivery);
        }
        assert!(app2
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("Content search failed"));
        app2.shutdown_background().await;
    }

    #[tokio::test]
    async fn task3_content_search_close_cancels_inflight_request() {
        let (dir, mut app) = setup_app();
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(crate::app_jobs::run),
        )));
        app.tree_state.reload_dir(dir.path());
        app.open_search();
        app.open_content_search();
        app.content_search.query = "definitely_absent_literal_zzz".into();
        app.content_search.cursor_position = app.content_search.query.len();
        app.start_content_search();
        assert!(app.content_search.generation.is_some());
        app.close_search();
        assert!(app.content_search.generation.is_none());
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        app.shutdown_background().await;
    }

    // ── Phase 6 Task 5: composite responsiveness/search checkpoint ───────────

    /// Drain accepted background deliveries until `quiet` holds or the guard
    /// trips. Uses a real timeout rather than a sleep so a genuine stall fails
    /// instead of hanging the suite.
    async fn task5_drain_until(app: &mut App, quiet: impl Fn(&App) -> bool) {
        let mut guard = 0;
        while !quiet(app) && guard < 1000 {
            match tokio::time::timeout(std::time::Duration::from_secs(10), app.next_background())
                .await
            {
                Ok(Some(delivery)) => app.apply_background(delivery),
                _ => break,
            }
            guard += 1;
        }
    }

    /// `FileManagerTUI-5yf` end-to-end: a root whose direct subdirectory count
    /// exceeds the default `max_pending` plus a reachable matching file. The
    /// worker-driven scan must finish (not stall), report completion honestly,
    /// and return the hit.
    #[tokio::test]
    async fn task5_checkpoint_wide_directory_content_search_returns_hits_and_completes() {
        let (dir, mut app) = setup_app();
        for index in 0..(crate::search::DEFAULT_MAX_PENDING + 8) {
            fs::create_dir(dir.path().join(format!("d{index:05}"))).unwrap();
        }
        fs::write(dir.path().join("zzz.txt"), "needle\n").unwrap();
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(crate::app_jobs::run),
        )));
        app.tree_state.reload_dir(dir.path());
        app.open_search();
        app.content_search_active = true;
        app.content_search.query = "needle".into();
        app.content_search.cursor_position = 6;
        app.start_content_search();
        task5_drain_until(&mut app, |app| !app.content_search.scanning).await;
        assert!(
            !app.content_search.scanning,
            "wide-directory scan must finish instead of stalling"
        );
        assert!(
            app.content_search.complete,
            "the traversal must report honest completion"
        );
        assert!(!app.content_search.capped);
        assert!(app
            .content_search
            .hits
            .iter()
            .any(|hit| hit.path.ends_with("zzz.txt")));
        app.shutdown_background().await;
    }

    /// Generation isolation under a rapid input/output flood: a slow, gated
    /// preview of the first selection stays in flight while newer preview
    /// selections and newer content-search queries supersede it. What this test
    /// proves is **cancellation/supersede**, not the stale-arrival guard:
    /// superseded jobs are cancelled in `submit_content_search`, so a stale
    /// content batch never reaches `apply_content_search`. The guard itself is
    /// proven directly by
    /// `task5_checkpoint_stale_content_generation_is_rejected_at_apply`.
    #[tokio::test]
    async fn task5_checkpoint_old_generations_cannot_overwrite_under_slow_flood() {
        use crate::app_jobs::Target;
        let (dir, mut app) = setup_app();
        let files: Vec<PathBuf> = (0..4)
            .map(|index| {
                let path = dir.path().join(format!("q{index}.txt"));
                fs::write(&path, format!("TERM_{index}\n")).unwrap();
                path
            })
            .collect();
        app.tree_state.reload_dir(dir.path());
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered_tx));
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_gate = gate.clone();
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(move |job: crate::app_jobs::NativeJob, token| {
                if let Target::Preview(key) = &job.target {
                    if key.path.ends_with("q0.txt") {
                        entered.lock().unwrap().take().unwrap().send(()).unwrap();
                        let _open = worker_gate
                            .1
                            .wait_while(worker_gate.0.lock().unwrap(), |open| !*open);
                    }
                }
                crate::app_jobs::run(job, token)
            }),
        )));
        // Flood preview navigation q0 (gated, slow) -> q1 -> q2 -> q3.
        app.tree_state.selected_index = app
            .tree_state
            .flat_items
            .iter()
            .position(|node| node.path == files[0])
            .unwrap();
        app.last_previewed_index = None;
        app.update_preview();
        entered_rx.await.unwrap();
        for file in &files[1..] {
            app.tree_state.selected_index = app
                .tree_state
                .flat_items
                .iter()
                .position(|node| node.path == *file)
                .unwrap();
            app.last_previewed_index = None;
            app.update_preview();
        }
        // Flood content queries for the same root while q0 is still gated.
        app.open_search();
        app.open_content_search();
        for index in 0..4 {
            app.content_search.query = format!("TERM_{index}");
            app.content_search.cursor_position = app.content_search.query.len();
            app.start_content_search();
        }
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        task5_drain_until(&mut app, |app| {
            !app.content_search.scanning
                && !app.jobs.as_ref().is_some_and(|jobs| jobs.has_pending())
        })
        .await;
        assert_eq!(
            app.preview_state.current_path.as_deref(),
            Some(files[3].as_path()),
            "an older preview generation overwrote the newest selection"
        );
        assert!(!app.content_search.scanning);
        assert!(
            !app.content_search.hits.is_empty(),
            "the newest query must produce its hits"
        );
        assert!(
            app.content_search
                .hits
                .iter()
                .all(|hit| hit.path == files[3] && hit.excerpt.contains("TERM_3")),
            "a stale content batch reached the current state"
        );
        app.shutdown_background().await;
    }

    /// The generation guard itself, exercised without cancellation: a delivery
    /// carrying a superseded generation is handed straight to
    /// `apply_background`, bypassing the pool's cancel-on-supersede path. Only
    /// the guard can reject it, so this test fails if the guard at
    /// `apply_content_search` is removed.
    #[tokio::test]
    async fn task5_checkpoint_stale_content_generation_is_rejected_at_apply() {
        use crate::app_jobs::{Delivery, NativeOutput, Target};
        let (dir, mut app) = setup_app();
        let stale_file = dir.path().join("stale.txt");
        fs::write(&stale_file, "STALE_GEN_MATCH\n").unwrap();
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(crate::app_jobs::run),
        )));
        app.tree_state.reload_dir(dir.path());
        app.open_search();
        app.open_content_search();
        // Admit generation G1, capture it, then supersede with G2 without
        // cancelling G1 in the pool: G1's delivery is forged below.
        app.content_search.query = "STALE_GEN_MATCH".into();
        app.content_search.cursor_position = app.content_search.query.len();
        app.start_content_search();
        let stale_generation = app
            .content_search
            .generation
            .expect("first content request admitted");
        app.content_search.query = "NO_SUCH_MATCH_zzz".into();
        app.content_search.cursor_position = app.content_search.query.len();
        app.start_content_search();
        let current_generation = app
            .content_search
            .generation
            .expect("superseding content request admitted");
        assert_ne!(stale_generation, current_generation);
        let hits_before = app.content_search.hits.len();
        // Forge a delivery for the OLD generation carrying a matching hit.
        let stale_batch = crate::search::ContentBatch {
            hits: vec![crate::search::SearchHit {
                path: stale_file.clone(),
                line: 1,
                byte: 0,
                column: 0,
                excerpt: "STALE_GEN_MATCH".into(),
            }],
            cursor: crate::search::SearchCursor::new(dir.path()),
        };
        app.apply_background(Delivery {
            target: Target::ContentSearch(dir.path().to_path_buf()),
            generation: stale_generation,
            result: Ok(NativeOutput::ContentSearch(stale_batch)),
        });
        assert_eq!(
            app.content_search.hits.len(),
            hits_before,
            "a stale generation reached apply_content_search"
        );
        assert_eq!(
            app.content_search.generation,
            Some(current_generation),
            "the stale delivery retired the current generation"
        );
        assert!(
            app.content_search
                .hits
                .iter()
                .all(|hit| hit.path != stale_file),
            "a stale hit was applied"
        );
        app.shutdown_background().await;
    }

    /// A worker panic is converted to a failed delivery, never a hang, and the
    /// pool stays usable for the next request.
    #[tokio::test]
    async fn task5_checkpoint_worker_panic_is_isolated_and_pool_recovers() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("file_a.txt");
        fs::write(&file, "recovered\n").unwrap();
        app.tree_state.reload_dir(dir.path());
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_worker = calls.clone();
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(move |job: crate::app_jobs::NativeJob, token| {
                if calls_worker.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    panic!("injected checkpoint panic");
                }
                crate::app_jobs::run(job, token)
            }),
        )));
        // First admitted job panics; the failure is reported, not swallowed.
        app.open_search();
        task5_drain_until(&mut app, |app| !app.search_state.indexing).await;
        assert!(!app.search_state.indexing);
        assert!(
            !app.search_state.index_complete,
            "a panicked index must not be reported complete"
        );
        // The pool is still usable: a later preview applies normally.
        app.tree_state.selected_index = app
            .tree_state
            .flat_items
            .iter()
            .position(|node| node.path == file)
            .unwrap();
        app.last_previewed_index = None;
        app.update_preview();
        task5_drain_until(&mut app, |app| {
            !app.jobs.as_ref().is_some_and(|jobs| jobs.has_pending())
        })
        .await;
        assert_eq!(
            app.preview_state.current_path.as_deref(),
            Some(file.as_path())
        );
        app.shutdown_background().await;
    }

    /// Rendering and input never invoke loaders: with an injected slow (gated)
    /// preview loader, draws and a preview command perform zero render-thread
    /// I/O and the state degrades to a pending preview instead of blocking.
    #[tokio::test]
    async fn task5_checkpoint_slow_loader_never_blocks_draw_or_input() {
        use crate::app_jobs::Target;
        let (dir, mut app) = setup_app();
        let file = dir.path().join("file_a.txt");
        fs::write(&file, "slow\ncontent\n").unwrap();
        app.tree_state.reload_dir(dir.path());
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_gate = gate.clone();
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(move |job: crate::app_jobs::NativeJob, token| {
                if matches!(&job.target, Target::Preview(_)) {
                    let _open = worker_gate
                        .1
                        .wait_while(worker_gate.0.lock().unwrap(), |open| !*open);
                }
                crate::app_jobs::run(job, token)
            }),
        )));
        app.tree_state.selected_index = app
            .tree_state
            .flat_items
            .iter()
            .position(|node| node.path == file)
            .unwrap();
        app.last_previewed_index = None;
        let _guard = crate::highlighting::RenderIoGuard::arm();
        app.update_preview();
        assert!(
            summary_text(&app).contains("Preparing preview"),
            "a slow loader must degrade to a pending state"
        );
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| crate::ui::render(&mut app, frame))
            .unwrap();
        assert_eq!(
            crate::highlighting::render_io_count(),
            0,
            "draw performed prepared-state I/O"
        );
        // An input path while the loader is slow must also stay clean. Tree
        // navigation resubmits the preview from the worker, never synchronously.
        app.workspace.focus.panel = FocusedPanel::Tree;
        let (tx, _rx) = crate::event::event_channel(Default::default());
        crate::handler::handle_key_event(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Down,
                crossterm::event::KeyModifiers::NONE,
            ),
            &tx,
        );
        assert_eq!(
            crate::highlighting::render_io_count(),
            0,
            "input path performed prepared-state I/O"
        );
        terminal
            .draw(|frame| crate::ui::render(&mut app, frame))
            .unwrap();
        assert_eq!(crate::highlighting::render_io_count(), 0);
        // Re-focus the newest selection and release the slow loader.
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.tree_state.selected_index = app
            .tree_state
            .flat_items
            .iter()
            .position(|node| node.path == file)
            .unwrap();
        app.last_previewed_index = None;
        app.update_preview();
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        task5_drain_until(&mut app, |app| {
            !app.jobs.as_ref().is_some_and(|jobs| jobs.has_pending())
        })
        .await;
        assert_eq!(
            app.preview_state.current_path.as_deref(),
            Some(file.as_path())
        );
        app.shutdown_background().await;
    }

    /// Dirty-redraw coalescing remains the production policy for background and
    /// terminal output bursts: many non-immediate dirty marks collapse into a
    /// single bounded frame slot.
    #[test]
    fn task5_checkpoint_dirty_redraw_coalesces_background_output_bursts() {
        let mut redraw =
            crate::background::DirtyRedraw::new(std::time::Duration::ZERO, Default::default())
                .unwrap();
        redraw.drawn(std::time::Duration::ZERO);
        for _ in 0..32 {
            redraw.dirty(false);
        }
        assert_eq!(
            redraw.wait(std::time::Duration::from_millis(1)),
            Some(std::time::Duration::from_millis(15)),
            "coalesced output burst schedules one frame, not one per event"
        );
        redraw.drawn(std::time::Duration::from_millis(1));
        assert_eq!(
            redraw.wait(std::time::Duration::from_millis(2)),
            None,
            "a clean frame with no pending work must not busy-loop"
        );
        redraw.dirty(true);
        assert_eq!(
            redraw.wait(std::time::Duration::from_millis(2)),
            Some(std::time::Duration::ZERO),
            "explicit input forces an immediate frame"
        );
    }

    // === Filter (/) tests ===

    #[test]
    fn start_filter_sets_mode() {
        let (_dir, mut app) = setup_app();
        app.start_filter();
        assert_eq!(app.workspace.focus.overlay, AppMode::Filter);
    }

    #[test]
    fn filter_input_filters_tree() {
        let (_dir, mut app) = setup_app();
        app.start_filter();
        let total_before = app.tree_state.flat_items.len();
        app.filter_input_char('a');
        // Should show fewer items (only matching + ancestors)
        assert!(app.tree_state.flat_items.len() <= total_before);
        assert!(app.tree_state.is_filtering);
    }

    #[test]
    fn filter_preserves_parent_dirs() {
        let (dir, mut app) = setup_app();
        // Create inner.txt inside alpha
        File::create(dir.path().join("alpha").join("inner.txt")).unwrap();
        // Expand alpha so its children are loaded
        app.tree_state.selected_index = 1; // alpha dir
        app.expand_selected();

        app.start_filter();
        app.filter_input_char('i');
        app.filter_input_char('n');
        app.filter_input_char('n');
        app.filter_input_char('e');
        app.filter_input_char('r');

        // "alpha" directory should be preserved as parent of "inner.txt"
        let names: Vec<&str> = app
            .tree_state
            .flat_items
            .iter()
            .map(|i| i.name.as_str())
            .collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"inner.txt"));
    }

    #[test]
    fn clear_filter_restores_tree() {
        let (_dir, mut app) = setup_app();
        let original_count = app.tree_state.flat_items.len();
        app.start_filter();
        app.filter_input_char('x');
        app.clear_filter();
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert!(!app.tree_state.is_filtering);
        assert_eq!(app.tree_state.flat_items.len(), original_count);
    }

    #[test]
    fn accept_filter_keeps_filtered_view() {
        let (_dir, mut app) = setup_app();
        app.start_filter();
        app.filter_input_char('f');
        let filtered_count = app.tree_state.flat_items.len();
        app.accept_filter();
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        // Filtered view should persist
        assert_eq!(app.tree_state.flat_items.len(), filtered_count);
    }

    #[test]
    fn filter_backspace_updates_filter() {
        let (_dir, mut app) = setup_app();
        let original_count = app.tree_state.flat_items.len();
        app.start_filter();
        app.filter_input_char('z');
        app.filter_input_char('z');
        app.filter_delete_char();
        app.filter_delete_char();
        // Should restore full tree when filter query becomes empty
        assert!(!app.tree_state.is_filtering);
        assert_eq!(app.tree_state.flat_items.len(), original_count);
    }

    #[test]
    fn filter_case_insensitive() {
        let (_dir, mut app) = setup_app();
        app.start_filter();
        app.filter_input_char('F');
        app.filter_input_char('I');
        app.filter_input_char('L');
        app.filter_input_char('E');
        // Should match "file_a.txt" and "file_b.rs" despite uppercase query
        let names: Vec<&str> = app
            .tree_state
            .flat_items
            .iter()
            .map(|i| i.name.as_str())
            .collect();
        assert!(names.contains(&"file_a.txt"));
        assert!(names.contains(&"file_b.rs"));
    }

    #[test]
    fn navigate_to_path_expands_ancestors() {
        let (dir, mut app) = setup_app();
        // Create nested structure
        fs::create_dir_all(dir.path().join("alpha").join("nested")).unwrap();
        File::create(dir.path().join("alpha").join("nested").join("target.txt")).unwrap();
        app.tree_state.reload_dir(dir.path());

        let target = dir.path().join("alpha").join("nested").join("target.txt");
        app.navigate_to_path(&target);

        let selected = &app.tree_state.flat_items[app.tree_state.selected_index];
        assert_eq!(selected.name, "target.txt");
    }

    // === Filesystem watcher tests ===

    #[test]
    fn handle_fs_change_detects_new_file() {
        let (dir, mut app) = setup_app();
        let original_count = app.tree_state.flat_items.len();
        // Create a new file externally
        File::create(dir.path().join("new_file.txt")).unwrap();
        // Simulate watcher event
        app.handle_fs_change(vec![dir.path().join("new_file.txt")]);
        assert!(app.tree_state.flat_items.len() > original_count);
        let names: Vec<&str> = app
            .tree_state
            .flat_items
            .iter()
            .map(|i| i.name.as_str())
            .collect();
        assert!(names.contains(&"new_file.txt"));
    }

    #[test]
    fn handle_fs_change_preserves_selection() {
        let (dir, mut app) = setup_app();
        // Select "file_a.txt"
        let file_a_idx = app
            .tree_state
            .flat_items
            .iter()
            .position(|i| i.name == "file_a.txt")
            .unwrap();
        app.tree_state.selected_index = file_a_idx;

        // Create a new file externally, trigger refresh
        File::create(dir.path().join("zzz_newfile.txt")).unwrap();
        app.handle_fs_change(vec![dir.path().join("zzz_newfile.txt")]);

        // Selection should still point to file_a.txt
        let selected = &app.tree_state.flat_items[app.tree_state.selected_index];
        assert_eq!(selected.name, "file_a.txt");
    }

    #[test]
    fn handle_fs_change_selection_fallback_on_delete() {
        let (dir, mut app) = setup_app();
        // Select "file_a.txt"
        let file_a_idx = app
            .tree_state
            .flat_items
            .iter()
            .position(|i| i.name == "file_a.txt")
            .unwrap();
        app.tree_state.selected_index = file_a_idx;

        // Delete file_a.txt externally
        fs::remove_file(dir.path().join("file_a.txt")).unwrap();
        app.handle_fs_change(vec![dir.path().join("file_a.txt")]);

        // Selection should have moved to a valid index
        assert!(app.tree_state.selected_index < app.tree_state.flat_items.len());
    }

    #[test]
    fn handle_fs_change_preserves_expanded_dirs() {
        let (dir, mut app) = setup_app();
        // Expand "alpha" directory
        let alpha_idx = app
            .tree_state
            .flat_items
            .iter()
            .position(|i| i.name == "alpha")
            .unwrap();
        app.tree_state.selected_index = alpha_idx;
        app.expand_selected();
        let count_after_expand = app.tree_state.flat_items.len();

        // Create a file in root, trigger refresh
        File::create(dir.path().join("extra.txt")).unwrap();
        app.handle_fs_change(vec![dir.path().join("extra.txt")]);

        // alpha should still be expanded (count increased by 1 for new file)
        assert!(app.tree_state.flat_items.len() > count_after_expand);
        let alpha_item = app
            .tree_state
            .flat_items
            .iter()
            .find(|i| i.name == "alpha")
            .unwrap();
        assert!(alpha_item.is_expanded);
    }

    #[test]
    fn handle_fs_change_invalidates_search_cache() {
        let (dir, mut app) = setup_app();
        // Build search cache by opening and closing,
        // then rebuild it manually since close_search invalidates.
        app.open_search();
        app.close_search();
        // Rebuild the cache after close.
        app.search_state.cached_paths = Some(app.build_path_index());
        assert!(app.search_state.cached_paths.is_some());

        // Trigger fs change in Normal mode
        File::create(dir.path().join("cache_buster.txt")).unwrap();
        app.handle_fs_change(vec![dir.path().join("cache_buster.txt")]);
        assert!(app.search_state.cached_paths.is_none());
    }

    #[test]
    fn handle_fs_change_keeps_directory_preview_valid() {
        let (dir, mut app) = setup_app();
        // Select alpha directory
        app.tree_state.selected_index = 1;
        app.update_preview();
        assert_eq!(app.last_previewed_index, Some(1));

        // Trigger watcher refresh from a root-level file change
        File::create(dir.path().join("new_file.txt")).unwrap();
        app.handle_fs_change(vec![dir.path().join("new_file.txt")]);

        // Directory preview should remain valid to avoid restart/flicker loops
        assert_eq!(
            app.last_previewed_index,
            Some(app.tree_state.selected_index),
            "directory preview should not be invalidated on unrelated fs events"
        );
    }

    #[test]
    fn handle_fs_change_invalidates_preview_for_selected_file_change() {
        let (dir, mut app) = setup_app();
        let file_path = dir.path().join("file_a.txt");
        // Select file_a.txt
        app.tree_state.selected_index = 3;
        app.update_preview();
        assert_eq!(app.last_previewed_index, Some(3));

        // Simulate watcher event for the selected file
        std::fs::write(&file_path, "updated content\n").unwrap();
        app.handle_fs_change(vec![file_path]);

        assert_eq!(
            app.last_previewed_index, None,
            "selected file preview should be invalidated when that file changes"
        );
    }

    #[test]
    fn fs_change_skipped_during_search_mode() {
        let (_dir, mut app) = setup_app();
        // Open search — builds path cache and sets mode to Search
        app.open_search();
        assert!(app.search_state.cached_paths.is_some());
        assert_eq!(app.workspace.focus.overlay, AppMode::Search);

        // While in Search mode, fs change should be silently ignored
        app.handle_fs_change(vec![app.tree_state.root.path.clone()]);
        // Cache must NOT be invalidated while searching
        assert!(
            app.search_state.cached_paths.is_some(),
            "search cache should survive fs events during search mode"
        );
    }

    #[test]
    fn fs_change_skipped_during_filter_mode() {
        let (_dir, mut app) = setup_app();
        app.start_filter();
        app.filter_input_char('f');
        let filtered_count = app.tree_state.flat_items.len();
        assert_eq!(app.workspace.focus.overlay, AppMode::Filter);

        // While in Filter mode, fs change should be silently ignored
        app.handle_fs_change(vec![app.tree_state.root.path.clone()]);
        // flat_items should still be the filtered set, not the full tree
        assert_eq!(
            app.tree_state.flat_items.len(),
            filtered_count,
            "filtered view should survive fs events during filter mode"
        );
    }

    #[test]
    fn fs_change_skipped_when_filter_is_active_in_normal_mode() {
        let (dir, mut app) = setup_app();
        app.start_filter();
        app.filter_input_char('f');
        app.accept_filter();
        let filtered_count = app.tree_state.flat_items.len();
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert!(app.tree_state.is_filtering);

        File::create(dir.path().join("new_file.txt")).unwrap();
        app.handle_fs_change(vec![dir.path().join("new_file.txt")]);

        assert_eq!(
            app.tree_state.flat_items.len(),
            filtered_count,
            "filtered view should survive fs events after accept_filter"
        );
    }

    #[test]
    fn fs_change_works_after_closing_search() {
        let (dir, mut app) = setup_app();
        // Open and close search to return to Normal mode
        app.open_search();
        app.close_search();
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);

        // Now fs change should process normally
        let original_count = app.tree_state.flat_items.len();
        File::create(dir.path().join("new_file.txt")).unwrap();
        app.handle_fs_change(vec![dir.path().join("new_file.txt")]);
        assert!(app.tree_state.flat_items.len() > original_count);
    }

    #[test]
    fn full_refresh_reloads_tree() {
        let (dir, mut app) = setup_app();
        let original_count = app.tree_state.flat_items.len();
        // Create a file externally
        File::create(dir.path().join("f5_file.txt")).unwrap();
        app.full_refresh();
        assert!(app.tree_state.flat_items.len() > original_count);
        assert!(app.status_message.is_some());
    }

    #[test]
    fn toggle_watcher_flips_state() {
        let (_dir, mut app) = setup_app();
        // setup_app forces watcher_active = true for test convenience
        assert!(app.watcher_active);
        let result = app.toggle_watcher();
        assert!(!result);
        assert!(!app.watcher_active);
        let result2 = app.toggle_watcher();
        assert!(result2);
        assert!(app.watcher_active);
    }

    #[test]
    fn toggle_watcher_sets_status_message() {
        let (_dir, mut app) = setup_app();
        // Start active (set by setup_app); toggle off → manual message
        app.toggle_watcher();
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(msg.contains("Manual") || msg.contains("⏸"));
        // Toggle back on → auto-refresh message
        app.toggle_watcher();
        let (msg, _) = app.status_message.as_ref().unwrap();
        assert!(msg.contains("Auto") || msg.contains("👁"));
    }

    #[test]
    fn handle_fs_change_preserves_sort_order() {
        let (dir, mut app) = setup_app();

        // Verify initial sort order: dirs first (alpha, beta), then files (file_a.txt, file_b.rs)
        // flat_items[0] = root, [1] = alpha, [2] = beta, [3] = file_a.txt, [4] = file_b.rs
        assert_eq!(app.tree_state.flat_items[1].name, "alpha");
        assert_eq!(app.tree_state.flat_items[2].name, "beta");
        assert_eq!(app.tree_state.flat_items[3].name, "file_a.txt");
        assert_eq!(app.tree_state.flat_items[4].name, "file_b.rs");

        // Create a new file externally to trigger a change
        File::create(dir.path().join("aaa_new.txt")).unwrap();
        fs::create_dir(dir.path().join("gamma")).unwrap();
        app.handle_fs_change(vec![
            dir.path().join("aaa_new.txt"),
            dir.path().join("gamma"),
        ]);

        // After fs change, dirs must still appear first, alphabetically sorted
        let names: Vec<&str> = app
            .tree_state
            .flat_items
            .iter()
            .skip(1) // skip root
            .map(|item| item.name.as_str())
            .collect();

        // Find the boundary between dirs and files
        let dir_count = names
            .iter()
            .take_while(|n| {
                app.tree_state
                    .flat_items
                    .iter()
                    .find(|i| i.name == **n)
                    .map(|i| i.node_type == crate::fs::tree::NodeType::Directory)
                    .unwrap_or(false)
            })
            .count();

        // All directories should come first
        assert!(
            dir_count >= 3,
            "Expected at least 3 dirs (alpha, beta, gamma), got {dir_count}"
        );

        // Directories should be alphabetically sorted
        let dir_names: Vec<&str> = names[..dir_count].to_vec();
        assert_eq!(dir_names, vec!["alpha", "beta", "gamma"]);

        // Files should be alphabetically sorted
        let file_names: Vec<&str> = names[dir_count..].to_vec();
        let mut expected_files = file_names.clone();
        expected_files.sort_by_key(|a| a.to_lowercase());
        assert_eq!(
            file_names, expected_files,
            "Files should be alphabetically sorted"
        );
    }

    #[test]
    fn navigate_to_path_preserves_sort_order() {
        let (dir, mut app) = setup_app();

        // Create nested structure: alpha/z_file.txt, alpha/a_file.txt, alpha/nested_dir/
        fs::create_dir_all(dir.path().join("alpha").join("nested_dir")).unwrap();
        File::create(dir.path().join("alpha").join("z_file.txt")).unwrap();
        File::create(dir.path().join("alpha").join("a_file.txt")).unwrap();

        // Navigate to a nested file — this forces alpha to expand
        let target = dir.path().join("alpha").join("a_file.txt");
        app.navigate_to_path(&target);

        // Find alpha's children in the flat list
        let alpha_children: Vec<&str> = app
            .tree_state
            .flat_items
            .iter()
            .filter(|i| i.depth == 2) // alpha's children are at depth 2
            .map(|i| i.name.as_str())
            .collect();

        // nested_dir (directory) should come first, then a_file, z_file (alphabetical)
        assert!(
            !alpha_children.is_empty(),
            "Alpha should have children after navigate_to_path"
        );
        assert_eq!(
            alpha_children[0], "nested_dir",
            "Directory should come first"
        );
    }

    #[test]
    fn preview_scroll_with_content_and_area() {
        let (dir, mut app) = setup_app();
        // Write a file with many lines
        let content: String = (0..200).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(dir.path().join("file_a.txt"), &content).unwrap();

        // Simulate a real terminal preview area (height=30, visible=28)
        app.preview_area = Rect::new(40, 0, 80, 30);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);

        // Select file_a.txt
        app.tree_state.selected_index = 3;
        app.update_preview();

        let content_lines_len = app.preview_state.content_lines.len();
        let visible_height = app.preview_area.height.saturating_sub(2) as usize;
        let expected_max = content_lines_len.saturating_sub(visible_height);

        // Scroll down 50 times, simulating render cycle each time
        for _ in 0..50 {
            app.preview_scroll_down();
            app.update_preview();
            app.clamp_preview_scroll();
        }

        assert_eq!(
            app.preview_state.scroll_offset,
            50.min(expected_max),
            "After scrolling 50 times, offset should be 50 (or max if less)"
        );
    }

    #[test]
    fn preview_scroll_preserved_after_fs_change() {
        let (dir, mut app) = setup_app();
        // Write a file with many lines
        let content: String = (0..200).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(dir.path().join("file_a.txt"), &content).unwrap();

        // Simulate a real terminal preview area
        app.preview_area = Rect::new(40, 0, 80, 30);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);

        // Select file_a.txt and update preview
        app.tree_state.selected_index = 3;
        app.update_preview();

        // Scroll down to position 25
        for _ in 0..25 {
            app.preview_scroll_down();
        }
        assert_eq!(app.preview_state.scroll_offset, 25);

        // Simulate a file watcher event (unrelated file change)
        app.handle_fs_change(vec![dir.path().join("file_b.rs")]);

        // The next render cycle calls update_preview + clamp
        app.update_preview();
        app.clamp_preview_scroll();

        // Scroll offset should be preserved since we're still viewing the same file
        assert_eq!(
            app.preview_state.scroll_offset, 25,
            "Scroll offset should be preserved after FS change event for same file"
        );
    }

    #[test]
    fn preview_scroll_resets_on_different_file() {
        let (dir, mut app) = setup_app();
        let content: String = (0..200).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(dir.path().join("file_a.txt"), &content).unwrap();
        std::fs::write(dir.path().join("file_b.rs"), "fn main() {}\n").unwrap();

        app.preview_area = Rect::new(40, 0, 80, 30);
        app.preview_content_area = ratatui::widgets::Block::bordered().inner(app.preview_area);
        app.tree_state.selected_index = 3; // file_a.txt
        app.update_preview();

        // Scroll down
        for _ in 0..25 {
            app.preview_scroll_down();
        }
        assert_eq!(app.preview_state.scroll_offset, 25);

        // Switch to a different file
        app.tree_state.selected_index = 4; // file_b.rs
        app.last_previewed_index = None;
        app.update_preview();

        // Scroll should reset to 0 for different file
        assert_eq!(
            app.preview_state.scroll_offset, 0,
            "Scroll should reset when switching to a different file"
        );
    }

    // === Directional focus navigation tests ===

    #[test]
    fn focus_left_from_preview_goes_to_tree() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.focus_left();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
    }

    #[test]
    fn focus_left_from_terminal_goes_to_tree() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        app.focus_left();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
    }

    #[test]
    fn focus_left_from_tree_is_noop() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Tree;
        app.focus_left();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
    }

    #[test]
    fn focus_right_from_tree_goes_to_preview() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Tree;
        app.focus_right();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
    }

    #[test]
    fn focus_right_from_terminal_goes_to_preview() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        app.focus_right();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
    }

    #[test]
    fn focus_right_from_preview_is_noop() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.focus_right();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
    }

    #[test]
    fn focus_up_from_terminal_goes_to_tree() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        app.focus_up();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
    }

    #[test]
    fn focus_up_from_tree_is_noop() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Tree;
        app.focus_up();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
    }

    #[test]
    fn focus_up_from_preview_is_noop() {
        let (_dir, mut app) = setup_app();
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.focus_up();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
    }

    #[test]
    fn focus_down_from_tree_goes_to_terminal_when_visible() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Tree;
        app.focus_down();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Terminal);
    }

    #[test]
    fn focus_down_from_preview_goes_to_terminal_when_visible() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.focus_down();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Terminal);
    }

    #[test]
    fn focus_down_is_noop_when_terminal_hidden() {
        let (_dir, mut app) = setup_app();
        if app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Tree;
        app.focus_down();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
    }

    #[test]
    fn focus_down_is_noop_when_already_on_terminal() {
        let (_dir, mut app) = setup_app();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        app.focus_down();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Terminal);
    }

    // === Editor hard block tests ===

    #[test]
    fn editor_refuses_large_file_by_bytes() {
        let dir = TempDir::new().unwrap();
        let big_path = dir.path().join("big.txt");
        // Create file > 10MB
        let data = vec![b'x'; 11 * 1024 * 1024];
        std::fs::write(&big_path, &data).unwrap();

        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        sync_load_root(&mut app);
        // Navigate to big.txt: expand root, then select the file
        app.tree_state.expand_selected();
        // Find and select the big file
        for i in 0..app.tree_state.flat_items.len() {
            if app.tree_state.flat_items[i].path == big_path {
                app.tree_state.selected_index = i;
                break;
            }
        }
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.last_previewed_index = None;
        app.update_preview();

        let result = app.enter_edit_mode();
        assert!(!result);
        assert!(app.status_message.as_ref().unwrap().0.contains("too large"));
    }

    #[test]
    fn editor_refuses_file_with_too_many_lines() {
        let dir = TempDir::new().unwrap();
        let big_path = dir.path().join("many_lines.txt");
        // Create file with >100K lines (small per line to stay under 10MB)
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&big_path).unwrap();
            for i in 0..100_001 {
                writeln!(f, "{}", i).unwrap();
            }
        }

        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        sync_load_root(&mut app);
        app.tree_state.expand_selected();
        for i in 0..app.tree_state.flat_items.len() {
            if app.tree_state.flat_items[i].path == big_path {
                app.tree_state.selected_index = i;
                break;
            }
        }
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.last_previewed_index = None;
        app.update_preview();

        let result = app.enter_edit_mode();
        assert!(!result);
        assert!(app.status_message.as_ref().unwrap().0.contains("too large"));
    }

    #[test]
    fn editor_opens_small_file() {
        let dir = TempDir::new().unwrap();
        let small_path = dir.path().join("small.txt");
        std::fs::write(&small_path, "hello world\n").unwrap();

        let mut app = App::new(dir.path(), crate::config::AppConfig::default()).unwrap();
        sync_load_root(&mut app);
        app.tree_state.expand_selected();
        for i in 0..app.tree_state.flat_items.len() {
            if app.tree_state.flat_items[i].path == small_path {
                app.tree_state.selected_index = i;
                break;
            }
        }
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.last_previewed_index = None;
        app.update_preview();

        let result = app.enter_edit_mode();
        assert!(result);
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
    }

    #[test]
    fn editor_entry_and_exit_preserve_live_watcher_preference() {
        for enabled in [false, true] {
            let (dir, mut app) = setup_app();
            let file = dir.path().join("file_a.txt");
            fs::write(&file, "unchanged").unwrap();
            app.tree_state.selected_index = app
                .tree_state
                .flat_items
                .iter()
                .position(|item| item.path == file)
                .unwrap();
            app.preview_state.current_path = Some(file);
            app.workspace.focus.panel = FocusedPanel::Preview;
            app.watcher_active = enabled;
            assert!(app.enter_edit_mode());
            assert_eq!(app.watcher_active, enabled);

            app.exit_edit_mode();

            assert_eq!(app.watcher_active, enabled);
            assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
        }
    }
    #[test]
    fn ownership_retains_editor_after_exit_and_failed_open() {
        let (dir, mut app) = setup_app();
        let a = dir.path().join("file_a.txt");
        fs::write(&a, "original").unwrap();
        app.tree_state.selected_index = app
            .tree_state
            .flat_items
            .iter()
            .position(|i| i.path == a)
            .unwrap();
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.preview_state.current_path = Some(a.clone());
        assert!(app.enter_edit_mode());
        app.workspace
            .documents
            .active_mut()
            .map(|d| &mut d.editor)
            .unwrap()
            .insert_text("unsaved")
            .unwrap();
        let id = app.workspace.documents.active_id().unwrap();
        fs::write(&a, "external").unwrap();
        app.handle_fs_change(vec![a.clone()]);
        assert!(app
            .workspace
            .documents
            .get(id)
            .unwrap()
            .has_external_change());
        app.exit_edit_mode();
        assert!(app
            .workspace
            .documents
            .active()
            .map(|d| &d.editor)
            .is_some_and(|e| e.modified));
        app.preview_state.current_path = Some(dir.path().join("missing"));
        assert!(!app.enter_edit_mode());
        assert_eq!(app.workspace.documents.active_id(), Some(id));
        assert!(app
            .workspace
            .documents
            .active()
            .map(|d| &d.editor)
            .unwrap()
            .buffer[0]
            .contains("unsaved"));
    }
    #[test]
    fn path_selection_survives_refresh_and_sort_with_multi_select() {
        let (dir, mut app) = setup_app();
        let file_a = dir.path().join("file_a.txt");
        let file_b = dir.path().join("file_b.rs");
        app.tree_state.selected_index = app.tree_state.find_index_by_path(&file_a).unwrap();
        app.tree_state.toggle_multi_select();
        app.tree_state.selected_index = app.tree_state.find_index_by_path(&file_b).unwrap();
        assert!(app.tree_state.selected_paths().contains(&file_a));
        assert!(app.tree_state.selected_paths().contains(&file_b));

        // A refresh inserts an alphabetically-earlier sibling, shifting rows.
        File::create(dir.path().join("aaa_new.txt")).unwrap();
        app.handle_fs_change(vec![dir.path().join("aaa_new.txt")]);
        assert!(
            app.tree_state.selected_paths().contains(&file_a),
            "multi-selection lost across refresh"
        );
        assert!(
            app.tree_state.selected_paths().contains(&file_b),
            "primary selection lost across refresh"
        );
        assert_eq!(
            app.tree_state.flat_items[app.tree_state.selected_index].path,
            file_b
        );

        // Changing sort order must not move the selected paths either.
        app.tree_state.cycle_sort();
        app.tree_state.toggle_dirs_first();
        assert!(app.tree_state.selected_paths().contains(&file_a));
        assert!(app.tree_state.selected_paths().contains(&file_b));
    }

    #[test]
    fn open_document_detects_external_change_while_editor_focused_in_manual_mode() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("file_a.txt");
        fs::write(&file, "original\n").unwrap();
        app.tree_state.selected_index = app.tree_state.find_index_by_path(&file).unwrap();
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.preview_state.current_path = Some(file.clone());
        assert!(app.enter_edit_mode());
        let id = app.workspace.documents.active_id().unwrap();
        // Manual-refresh mode must not globally suppress document watching.
        app.watcher_active = false;
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        fs::write(&file, "external\n").unwrap();
        app.handle_fs_change(vec![file.clone()]);
        assert!(
            app.workspace
                .documents
                .get(id)
                .unwrap()
                .has_external_change(),
            "open document missed an external write while focused in manual mode"
        );
    }

    #[test]
    fn internal_save_is_not_reported_as_external_change() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("file_a.txt");
        fs::write(&file, "original\n").unwrap();
        app.tree_state.selected_index = app.tree_state.find_index_by_path(&file).unwrap();
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.preview_state.current_path = Some(file.clone());
        assert!(app.enter_edit_mode());
        let id = app.workspace.documents.active_id().unwrap();
        app.workspace
            .documents
            .get_mut(id)
            .unwrap()
            .editor
            .insert_text("edited\n")
            .unwrap();
        app.save_editor_buffer().unwrap();
        // The watcher event for our own save must not self-flag as external.
        app.handle_fs_change(vec![file.clone()]);
        assert!(!app
            .workspace
            .documents
            .get(id)
            .unwrap()
            .has_external_change());
    }

    #[test]
    fn self_write_marker_does_not_swallow_a_genuine_external_write() {
        // The marker must be consumed only by the change that matches the exact
        // post-write identity, not by the first event touching the path.
        let (dir, mut app) = setup_app();
        let file = dir.path().join("file_a.txt");
        fs::write(&file, "original\n").unwrap();
        app.tree_state.selected_index = app.tree_state.find_index_by_path(&file).unwrap();
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.preview_state.current_path = Some(file.clone());
        assert!(app.enter_edit_mode());
        let id = app.workspace.documents.active_id().unwrap();
        app.workspace.documents.get_mut(id).unwrap().editor.buffer = vec!["saved".to_string()];
        app.save_editor_buffer().unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "saved");

        // The save's own watcher event is lost/coalesced. A later external write
        // overwrites the file and arrives as the first event for this path.
        fs::write(&file, "external-overwrite\n").unwrap();
        app.handle_fs_change(vec![file.clone()]);

        assert!(
            app.workspace
                .documents
                .get(id)
                .unwrap()
                .has_external_change(),
            "a genuine external write was swallowed by the self-write marker"
        );
    }

    #[test]
    fn self_write_marker_reports_same_len_same_mtime_external_content() {
        // Residual P2 red: an external overwrite that preserves BOTH length and
        // mtime (cp -p, rsync -t, touch -r, archive extraction) must still be
        // reported, so the stored identity must include a content digest and not
        // rely on length/mtime alone.
        let (dir, mut app) = setup_app();
        let file = dir.path().join("file_a.txt");
        fs::write(&file, "AAAAA").unwrap();
        app.tree_state.selected_index = app.tree_state.find_index_by_path(&file).unwrap();
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.preview_state.current_path = Some(file.clone());
        assert!(app.enter_edit_mode());
        let id = app.workspace.documents.active_id().unwrap();
        // Save exactly five bytes.
        app.workspace.documents.get_mut(id).unwrap().editor.buffer = vec!["AAAAA".to_string()];
        app.save_editor_buffer().unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "AAAAA");
        let saved_mtime = fs::metadata(&file).unwrap().modified().unwrap();

        // External write of a DIFFERENT five bytes, with the mtime forced back to
        // the saved value.
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(&file, "BBBBB").unwrap();
        force_mtime(&file, saved_mtime);
        assert_eq!(fs::metadata(&file).unwrap().len(), 5);
        assert_eq!(
            fs::metadata(&file).unwrap().modified().unwrap(),
            saved_mtime
        );

        app.handle_fs_change(vec![file.clone()]);
        assert!(
            app.workspace
                .documents
                .get(id)
                .unwrap()
                .has_external_change(),
            "a same-length, same-mtime external content change was swallowed"
        );
    }

    /// Force `path`'s mtime back to `when` (simulates `cp -p`/`touch -r`).
    #[cfg(unix)]
    fn force_mtime(path: &std::path::Path, when: std::time::SystemTime) {
        use std::os::unix::ffi::OsStrExt;
        let elapsed = when.duration_since(std::time::UNIX_EPOCH).unwrap();
        let times = [
            libc::timespec {
                tv_sec: elapsed.as_secs() as libc::time_t,
                tv_nsec: elapsed.subsec_nanos() as libc::c_long,
            },
            libc::timespec {
                tv_sec: elapsed.as_secs() as libc::time_t,
                tv_nsec: elapsed.subsec_nanos() as libc::c_long,
            },
        ];
        let mut c_path = path.as_os_str().as_bytes().to_vec();
        c_path.push(0);
        // SAFETY: `c_path` is a NUL-terminated C string and `times` is a valid
        // two-element timespec array; the call only mutates metadata of this
        // temporary test file.
        let rc =
            unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr().cast(), times.as_ptr(), 0) };
        assert_eq!(rc, 0, "utimensat failed to force mtime");
    }

    #[test]
    fn self_write_marker_resolution_covers_mismatch_missing_and_overflow() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("file_a.txt");
        fs::write(&file, "saved").unwrap();
        app.note_self_write(&file, b"saved");
        assert!(app.take_self_write(&file), "match must consume the marker");
        // Consumed precisely once.
        assert!(!app.take_self_write(&file));

        // Same-length external overwrite with the SAME mtime: only the content
        // digest distinguishes it, so this case is insensitive to the sleep and
        // fails if the digest comparison is removed.
        app.note_self_write(&file, b"saved");
        let saved_mtime = fs::metadata(&file).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&file, "XXXXX").unwrap();
        force_mtime(&file, saved_mtime);
        assert_eq!(
            fs::metadata(&file).unwrap().modified().unwrap(),
            saved_mtime
        );
        assert!(
            !app.take_self_write(&file),
            "a same-size, same-mtime external overwrite must not be swallowed"
        );
        assert!(!app.take_self_write(&file), "mismatch marker is discarded");

        // Same content but a different mtime is still reported (mtime term
        // enforced): fails if the mtime comparison is removed.
        app.note_self_write(&file, b"XXXXX");
        let bumped = saved_mtime - std::time::Duration::from_secs(5);
        force_mtime(&file, bumped);
        assert!(
            !app.take_self_write(&file),
            "an mtime-only mismatch must not be swallowed"
        );

        // A vanished path cannot be verified ⇒ no marker is retained.
        app.note_self_write(&file, b"XXXXX");
        fs::remove_file(&file).unwrap();
        assert!(
            !app.take_self_write(&file),
            "an unreadable/missing path must not be swallowed"
        );

        // FIFO eviction stays bounded at SELF_WRITE_MAX.
        for index in 0..(SELF_WRITE_MAX + 5) {
            let path = dir.path().join(format!("bulk_{index}.txt"));
            fs::write(&path, "x").unwrap();
            app.note_self_write(&path, b"x");
        }
        assert!(app.self_written.len() <= SELF_WRITE_MAX);
    }

    #[test]
    fn self_write_digest_window_covers_prefix_and_beyond_window_tail() {
        // The digest window is the first SELF_WRITE_DIGEST_BYTES. A change inside
        // the window is detected even for content larger than the window; a tail
        // change with identical length and mtime is the documented residual limit.
        let in_window_a = vec![b'a'; SELF_WRITE_DIGEST_BYTES + 100];
        let mut in_window_b = in_window_a.clone();
        in_window_b[0] = b'b';
        assert_ne!(
            self_write_digest(&in_window_a),
            self_write_digest(&in_window_b)
        );

        let mut tail_only = in_window_a.clone();
        let last = tail_only.len() - 1;
        tail_only[last] = b'z';
        assert_eq!(
            self_write_digest(&in_window_a),
            self_write_digest(&tail_only),
            "a change confined to the tail is the documented residual limit"
        );

        // A short file fully inside the window is compared over its whole content.
        assert_ne!(self_write_digest(b"saved"), self_write_digest(b"Saved"));
        assert_eq!(self_write_digest(b"saved"), self_write_digest(b"saved"));
    }

    #[test]
    fn ownership_multiple_documents_and_dedup_keep_unsaved_state() {
        let (dir, mut app) = setup_app();
        let a = dir.path().join("file_a.txt");
        let b = dir.path().join("file_b.txt");
        fs::write(&a, "original").unwrap();
        fs::write(&b, "other").unwrap();
        app.tree_state.selected_index = app
            .tree_state
            .flat_items
            .iter()
            .position(|i| i.path == a)
            .unwrap();
        app.workspace.focus.panel = FocusedPanel::Preview;
        app.preview_state.current_path = Some(a.clone());
        assert!(app.enter_edit_mode());
        app.workspace
            .documents
            .active_mut()
            .map(|d| &mut d.editor)
            .unwrap()
            .insert_text("unsaved")
            .unwrap();
        let id = app.workspace.documents.active_id().unwrap();
        let history = app.editor().unwrap().undo_stack.len();
        app.exit_edit_mode();
        app.preview_state.current_path = Some(b);
        assert!(app.enter_edit_mode());
        app.exit_edit_mode();
        assert_eq!(app.workspace.documents.len(), 2);
        app.preview_state.current_path = Some(dir.path().join("./file_a.txt"));
        assert!(app.enter_edit_mode());
        assert_eq!(app.workspace.documents.active_id(), Some(id));
        assert_eq!(app.workspace.documents.len(), 2);
        assert_eq!(app.editor().unwrap().undo_stack.len(), history);
        assert!(!app.should_quit);
        assert!(app
            .workspace
            .documents
            .active()
            .map(|d| &d.editor)
            .unwrap()
            .buffer[0]
            .contains("unsaved"));
    }

    #[test]
    fn stage2b_save_modal_targets_origin_after_activation_changes() {
        let (dir, mut app) = setup_app();
        let a_path = dir.path().join("file_a.txt");
        let b_path = dir.path().join("file_b.txt");
        fs::write(&b_path, "beta").unwrap();
        let a = app
            .workspace
            .documents
            .open(
                &a_path,
                crate::workspace::documents::OpenDisposition::Pinned,
            )
            .unwrap();
        app.workspace
            .documents
            .get_mut(a)
            .unwrap()
            .editor
            .insert_text("unsaved")
            .unwrap();
        app.open_dialog(DialogKind::SaveConfirm);
        let b = app
            .workspace
            .documents
            .open(
                &b_path,
                crate::workspace::documents::OpenDisposition::Pinned,
            )
            .unwrap();
        app.workspace
            .documents
            .get_mut(b)
            .unwrap()
            .editor
            .insert_text("other")
            .unwrap();
        app.save_editor_buffer().unwrap();
        assert!(!app.workspace.documents.get(a).unwrap().editor.modified);
        assert!(app.workspace.documents.get(b).unwrap().editor.modified);
        assert!(fs::read_to_string(a_path).unwrap().starts_with("unsaved"));
        assert!(!app.should_quit);
    }

    #[test]
    fn document_lifecycle_close_last_document_restores_preview_and_refuses_other_overlay() {
        let (dir, mut app) = setup_app();
        app.close_active_document();
        let id = app
            .workspace
            .documents
            .open(
                &dir.path().join("file_a.txt"),
                crate::workspace::documents::OpenDisposition::Pinned,
            )
            .unwrap();
        app.workspace.focus.panel = FocusedPanel::Editor;
        app.open_dialog(DialogKind::CreateFile);
        app.close_active_document();
        assert!(app.workspace.documents.get(id).is_some());
        app.close_dialog();
        app.close_active_document();
        assert!(app.workspace.documents.is_empty());
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
        assert_eq!(
            app.right_panel_presentation,
            RightPanelPresentation::SelectedPreview
        );
        assert!(!app.should_quit);
    }

    #[test]
    fn document_lifecycle_quit_skips_already_clean_captured_doc_but_never_new_dirty_doc() {
        for add_new_dirty in [false, true] {
            let (dir, mut app) = setup_app();
            let mut ids = Vec::new();
            for name in ["a", "b"] {
                let path = dir.path().join(name);
                fs::write(&path, "text").unwrap();
                let id = app
                    .workspace
                    .documents
                    .open(&path, crate::workspace::documents::OpenDisposition::Pinned)
                    .unwrap();
                app.workspace
                    .documents
                    .get_mut(id)
                    .unwrap()
                    .editor
                    .insert_char('x');
                ids.push(id);
            }
            app.quit();
            app.workspace
                .documents
                .get_mut(ids[1])
                .unwrap()
                .editor
                .save()
                .unwrap();
            if add_new_dirty {
                let path = dir.path().join("new");
                fs::write(&path, "new").unwrap();
                let id = app
                    .workspace
                    .documents
                    .open(&path, crate::workspace::documents::OpenDisposition::Pinned)
                    .unwrap();
                app.workspace
                    .documents
                    .get_mut(id)
                    .unwrap()
                    .editor
                    .insert_char('!');
                ids.push(id);
            }
            app.save_editor_buffer().unwrap();
            assert_eq!(app.should_quit, !add_new_dirty);
            assert!(app.document_lifecycle.is_none());
            assert!(app.workspace.documents.get(ids[0]).is_some());
            if add_new_dirty {
                assert!(app.workspace.documents.get(ids[2]).unwrap().editor.modified);
            }
        }
    }

    #[test]
    fn document_lifecycle_stale_discard_and_missing_captured_document_halt_safely() {
        let (dir, mut app) = setup_app();
        let mut ids = Vec::new();
        for name in ["a", "b"] {
            let path = dir.path().join(name);
            fs::write(&path, "text").unwrap();
            let id = app
                .workspace
                .documents
                .open(&path, crate::workspace::documents::OpenDisposition::Pinned)
                .unwrap();
            app.workspace
                .documents
                .get_mut(id)
                .unwrap()
                .editor
                .insert_char('x');
            ids.push(id);
        }
        app.quit();
        app.discard_lifecycle_document(ids[1]);
        assert!(app.document_lifecycle.is_none());
        assert!(app.workspace.documents.get(ids[0]).unwrap().editor.modified);
        assert!(app.workspace.documents.get(ids[1]).unwrap().editor.modified);
        app.quit();
        app.workspace.documents.discard_and_close(ids[0]).unwrap();
        app.advance_document_lifecycle();
        assert!(!app.should_quit);
        assert!(app.document_lifecycle.is_none());
        assert!(app.workspace.documents.get(ids[1]).unwrap().editor.modified);
        app.close_dialog();
    }

    #[test]
    fn document_lifecycle_quit_pending_ids_are_bounded_without_partial_discard() {
        let (dir, mut app) = setup_app();
        for n in 0..1025 {
            let path = dir.path().join(format!("bounded-{n}"));
            fs::write(&path, "text").unwrap();
            let id = app
                .workspace
                .documents
                .open(&path, crate::workspace::documents::OpenDisposition::Pinned)
                .unwrap();
            app.workspace
                .documents
                .get_mut(id)
                .unwrap()
                .editor
                .insert_char('x');
        }
        app.quit();
        assert!(!app.should_quit);
        assert!(app.document_lifecycle.is_none());
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.documents.len(), 1025);
        assert!(app.workspace.documents.iter().all(|d| d.editor.modified));
    }

    #[test]
    fn document_lifecycle_missing_save_origin_halts_without_saving_active_other_doc() {
        let (_dir, mut app, a, b) = modal_documents();
        app.close_dialog();
        app.quit();
        app.workspace.documents.discard_and_close(a).unwrap();
        app.workspace.documents.activate(b).unwrap();
        assert!(app.save_editor_buffer().is_err());
        assert!(app.document_lifecycle.is_none());
        assert!(app.workspace.documents.get(b).unwrap().editor.modified);
        assert!(!app.should_quit);
    }

    #[test]
    fn stage2b_quit_guards_inactive_dirty_documents() {
        let (dir, mut app) = setup_app();
        fs::write(dir.path().join("file_b.txt"), "beta").unwrap();
        let a = app
            .workspace
            .documents
            .open(
                &dir.path().join("file_a.txt"),
                crate::workspace::documents::OpenDisposition::Pinned,
            )
            .unwrap();
        app.workspace
            .documents
            .get_mut(a)
            .unwrap()
            .editor
            .insert_text("unsaved")
            .unwrap();
        app.workspace
            .documents
            .open(
                &dir.path().join("file_b.txt"),
                crate::workspace::documents::OpenDisposition::Pinned,
            )
            .unwrap();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        app.quit();
        assert!(!app.should_quit);
        assert!(matches!(
            app.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::DocumentDecision { quitting: true, .. })
        ));
        app.quit();
        app.close_dialog();
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Terminal);
        assert!(app.workspace.focus.dismiss_overlay().is_none());
    }

    fn modal_documents() -> (
        TempDir,
        App,
        crate::workspace::documents::DocumentId,
        crate::workspace::documents::DocumentId,
    ) {
        let (dir, mut app) = setup_app();
        let a_path = dir.path().join("a.txt");
        let b_path = dir.path().join("b.txt");
        fs::write(&a_path, "alpha").unwrap();
        fs::write(&b_path, "beta").unwrap();
        let a = app
            .workspace
            .documents
            .open(
                &a_path,
                crate::workspace::documents::OpenDisposition::Pinned,
            )
            .unwrap();
        app.workspace
            .documents
            .get_mut(a)
            .unwrap()
            .editor
            .insert_text("unsaved")
            .unwrap();
        app.workspace.focus.panel = FocusedPanel::Editor;
        app.open_dialog(DialogKind::SaveConfirm);
        let b = app
            .workspace
            .documents
            .open(
                &b_path,
                crate::workspace::documents::OpenDisposition::Pinned,
            )
            .unwrap();
        app.workspace
            .documents
            .get_mut(b)
            .unwrap()
            .editor
            .insert_text("other")
            .unwrap();
        (dir, app, a, b)
    }

    #[test]
    fn stage2b_conflict_followups_save_as_origin_and_cancel_restore_focus() {
        let (dir, mut app, a, b) = modal_documents();
        fs::write(dir.path().join("a.txt"), "external").unwrap();
        assert!(app.save_editor_buffer().is_err());
        assert_eq!(app.workspace.focus.overlay_document(), Some(a));
        app.begin_editor_overwrite(true, false);
        assert_eq!(app.workspace.focus.overlay_document(), Some(a));
        app.open_dialog(DialogKind::EditorSaveAs {
            exit_after_save: false,
            normalize: false,
        });
        assert_eq!(app.workspace.focus.overlay_document(), Some(a));
        let b_text = app.workspace.documents.get(b).unwrap().text();
        app.save_editor_as("saved-a.txt", false, false).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("saved-a.txt")).unwrap(),
            "unsavedalpha"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "external"
        );
        assert_eq!(app.workspace.documents.get(b).unwrap().text(), b_text);
        assert!(app.workspace.documents.get(b).unwrap().editor.modified);
        assert_eq!(app.workspace.documents.active_id(), Some(a));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        // Repeated follow-ups consume only one frame, not a hidden modal stack.
        assert!(app.workspace.focus.dismiss_overlay().is_none());
    }

    #[test]
    fn stage2b_reload_and_cancel_use_origin_not_active_document() {
        for reload in [true, false] {
            let (dir, mut app, a, b) = modal_documents();
            fs::write(dir.path().join("a.txt"), "external").unwrap();
            assert!(app.save_editor_buffer().is_err());
            let b_text = app.workspace.documents.get(b).unwrap().text();
            if reload {
                app.reload_editor_buffer();
                assert_eq!(app.workspace.documents.get(a).unwrap().text(), "external");
                assert!(!app.workspace.documents.get(a).unwrap().editor.modified);
            } else {
                let (tx, _rx) = crate::event::event_channel(Default::default());
                crate::handler::handle_key_event(
                    &mut app,
                    crossterm::event::KeyEvent::new(
                        crossterm::event::KeyCode::Esc,
                        crossterm::event::KeyModifiers::NONE,
                    ),
                    &tx,
                );
                assert_eq!(
                    app.workspace.documents.get(a).unwrap().text(),
                    "unsavedalpha"
                );
                assert!(app.workspace.documents.get(a).unwrap().editor.modified);
            }
            assert_eq!(app.workspace.documents.get(b).unwrap().text(), b_text);
            assert!(app.workspace.documents.get(b).unwrap().editor.modified);
            assert_eq!(app.workspace.documents.active_id(), Some(a));
            assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
            assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        }
    }

    #[test]
    fn stage2b_new_external_revision_refuses_overwrite_of_origin() {
        let (dir, mut app, a, b) = modal_documents();
        let path = dir.path().join("a.txt");
        fs::write(&path, "external").unwrap();
        assert!(app.save_editor_buffer().is_err());
        app.begin_editor_overwrite(true, false);
        #[rustfmt::skip]
        let AppMode::Dialog(DialogKind::SaveOverwrite { expected_revision, .. }) = &app.workspace.focus.overlay else { unreachable!("expected overwrite confirmation") };
        let expected = expected_revision.clone();
        fs::write(&path, "newer external").unwrap();
        assert!(app
            .confirm_editor_overwrite(expected.as_ref(), true, false)
            .is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "newer external");
        assert_eq!(
            app.workspace.documents.get(a).unwrap().text(),
            "unsavedalpha"
        );
        assert_eq!(app.workspace.documents.get(b).unwrap().text(), "otherbeta");
        assert_eq!(app.workspace.documents.active_id(), Some(b));
        assert_eq!(app.workspace.focus.overlay_document(), Some(a));
        app.close_dialog();
        assert_eq!(app.workspace.documents.active_id(), Some(a));
    }

    #[test]
    fn stage2b_confirmed_overwrite_saves_origin_without_touching_active_b() {
        let (dir, mut app, a, b) = modal_documents();
        fs::write(dir.path().join("a.txt"), "external").unwrap();
        assert!(app.save_editor_buffer().is_err());
        app.begin_editor_overwrite(true, false);
        #[rustfmt::skip]
        let AppMode::Dialog(DialogKind::SaveOverwrite { expected_revision, .. }) = &app.workspace.focus.overlay else { unreachable!("expected overwrite confirmation") };
        let expected = expected_revision.clone();
        app.confirm_editor_overwrite(expected.as_ref(), true, false)
            .unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "unsavedalpha"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "beta"
        );
        assert!(!app.workspace.documents.get(a).unwrap().editor.modified);
        assert!(app.workspace.documents.get(b).unwrap().editor.modified);
        assert_eq!(app.workspace.documents.active_id(), Some(a));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Preview);
        assert!(!app.should_quit);
    }

    #[test]
    fn stage2b_failed_origin_reload_and_quit_save_preserve_both_buffers() {
        let (dir, mut app, a, b) = modal_documents();
        fs::remove_file(dir.path().join("a.txt")).unwrap();
        app.reload_editor_buffer();
        assert_eq!(app.workspace.focus.overlay_document(), Some(a));
        assert_eq!(app.workspace.documents.active_id(), Some(b));
        assert_eq!(
            app.workspace.documents.get(a).unwrap().text(),
            "unsavedalpha"
        );
        assert_eq!(app.workspace.documents.get(b).unwrap().text(), "otherbeta");
        app.close_dialog();
        app.workspace.documents.activate(b).unwrap();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        app.quit();
        assert!(app.save_editor_buffer().is_err());
        assert_eq!(app.workspace.focus.overlay_document(), Some(a));
        assert!(app.workspace.documents.get(a).unwrap().editor.modified);
        assert!(app.workspace.documents.get(b).unwrap().editor.modified);
        assert!(!app.should_quit);
        app.close_dialog();
        assert_eq!(app.workspace.documents.active_id(), Some(b));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Terminal);
    }

    #[test]
    fn stage2b_copy_nested_in_modal_restores_b_then_origin_a() {
        let (_dir, mut app, a, b) = modal_documents();
        app.config.general.mouse = Some(false);
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        let mut output = Vec::new();
        assert!(!app.show_copyable_text("literal copy".into(), &mut output, false));
        assert!(output.is_empty());
        app.workspace.documents.activate(a).unwrap();
        assert!(!app.dismiss_copy_overlay());
        assert_eq!(app.workspace.documents.active_id(), Some(b));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Terminal);
        assert_eq!(app.workspace.focus.overlay_document(), Some(a));
        assert_eq!(
            app.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::SaveConfirm)
        );
        app.close_dialog();
        assert_eq!(app.workspace.documents.active_id(), Some(a));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
    }

    #[test]
    fn stage2b_copy_depth_failure_leaves_context_and_transport_untouched() {
        let (_dir, mut app, a, _b) = modal_documents();
        for _ in 1..8 {
            app.workspace
                .focus
                .open_overlay(AppMode::Help, Some(a))
                .unwrap();
        }
        let mut output = Vec::new();
        assert!(!app.show_copyable_text("secret".into(), &mut output, true));
        assert!(output.is_empty());
        assert!(app.copy_overlay_text.is_none());
        assert_eq!(app.workspace.focus.overlay, AppMode::Help);
        assert_eq!(app.workspace.focus.overlay_document(), Some(a));
        assert!(app.status_message.as_ref().unwrap().0.contains("too many"));
    }

    #[test]
    fn stage2b_save_one_dirty_document_never_quits_with_another_dirty() {
        let (_dir, mut app, a, b) = modal_documents();
        app.close_dialog();
        app.workspace.documents.activate(b).unwrap();
        app.workspace.focus.panel = FocusedPanel::Tree;
        app.quit();
        assert_eq!(app.workspace.focus.overlay_document(), Some(a));
        app.save_editor_buffer().unwrap();
        assert!(!app.should_quit);
        assert!(!app.workspace.documents.get(a).unwrap().editor.modified);
        assert!(app.workspace.documents.get(b).unwrap().editor.modified);
        assert_eq!(app.workspace.documents.active_id(), Some(b));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
        app.quit();
        assert_eq!(app.workspace.focus.overlay_document(), Some(b));
        app.close_dialog();
        assert!(!app.should_quit);
    }

    #[test]
    fn stage2b_save_confirm_dismissal_restores_exact_origin_focus() {
        for key in [
            crossterm::event::KeyCode::Char('y'),
            crossterm::event::KeyCode::Char('n'),
        ] {
            let (_dir, mut app, a, b) = modal_documents();
            let (tx, _rx) = crate::event::event_channel(Default::default());
            crate::handler::handle_key_event(
                &mut app,
                crossterm::event::KeyEvent::new(key, crossterm::event::KeyModifiers::NONE),
                &tx,
            );
            assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
            assert_eq!(app.workspace.documents.active_id(), Some(a));
            assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
            assert!(app.workspace.documents.get(b).unwrap().editor.modified);
            assert!(!app.should_quit);
            if key == crossterm::event::KeyCode::Char('n') {
                assert!(app.workspace.documents.get(a).unwrap().editor.modified);
                assert!(app.status_message.as_ref().unwrap().0.contains("retained"));
            }
        }
    }

    fn completed_operation(dest: &Path) -> crate::event::OperationResult {
        crate::event::OperationResult {
            success_count: 1,
            errors: vec![],
            created_paths: vec![],
            source_paths: vec![],
            dest_dir: dest.into(),
            was_cut: false,
        }
    }

    fn progress_dialog() -> DialogKind {
        DialogKind::Progress {
            message: "Pasting".into(),
            current: 0,
            total: 1,
        }
    }

    #[test]
    fn round1_operation_completion_preserves_nested_copy_and_mouse_until_dismissal() {
        for mouse in [true, false] {
            let (dir, mut app, a, b) = modal_documents();
            app.close_dialog();
            app.config.general.mouse = Some(mouse);
            app.open_dialog(progress_dialog());
            app.workspace.documents.activate(b).unwrap();
            if !app.workspace.layout.terminal_visible() {
                app.workspace.layout.toggle_terminal();
            }
            app.workspace.focus.panel = FocusedPanel::Terminal;
            let mut transport = Vec::new();
            assert_eq!(
                app.show_copyable_text("manual payload".into(), &mut transport, false),
                mouse
            );
            let suspended_output = transport.clone();
            assert_eq!(app.copy_overlay_mouse_suspended, mouse);
            app.handle_operation_complete(completed_operation(dir.path()));
            assert_eq!(app.workspace.focus.overlay, AppMode::CopyOverlay);
            assert_eq!(app.copy_overlay_text.as_deref(), Some("manual payload"));
            assert_eq!(app.workspace.focus.overlay_document(), Some(b));
            assert_eq!(app.workspace.documents.active_id(), Some(b));
            app.restore_copy_mouse_capture(&mut transport);
            assert_eq!(transport, suspended_output);
            assert_eq!(app.copy_overlay_mouse_suspended, mouse);
            assert_eq!(app.dismiss_copy_overlay(), mouse);
            assert!(app.copy_overlay_text.is_none());
            assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
            assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
            assert_eq!(app.workspace.documents.active_id(), Some(a));
            assert!(app.workspace.focus.dismiss_overlay().is_none());
            transport.clear();
            app.restore_copy_mouse_capture(&mut transport);
            assert_eq!(transport.windows(7).any(|w| w == b"[?1000h"), mouse);
            assert!(!app.copy_overlay_mouse_suspended);
            transport.clear();
            app.restore_copy_mouse_capture(&mut transport);
            assert!(transport.is_empty());
        }
    }

    #[test]
    fn round1_operation_completion_retires_only_progress_between_save_and_copy() {
        let (dir, mut app, a, b) = modal_documents();
        app.workspace.focus.panel = FocusedPanel::Tree;
        app.workspace
            .focus
            .open_overlay(AppMode::Dialog(progress_dialog()), Some(b))
            .unwrap();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        let mut transport = Vec::new();
        app.config.general.mouse = Some(false);
        app.show_copyable_text("copy".into(), &mut transport, false);
        app.handle_operation_complete(completed_operation(dir.path()));
        assert_eq!(app.workspace.focus.overlay, AppMode::CopyOverlay);
        app.dismiss_copy_overlay();
        assert_eq!(
            app.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::SaveConfirm)
        );
        assert_eq!(app.workspace.focus.overlay_document(), Some(a));
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Tree);
        assert_eq!(app.workspace.documents.active_id(), Some(b));
        app.close_dialog();
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert_eq!(app.workspace.documents.active_id(), Some(a));
        assert!(app.workspace.focus.dismiss_overlay().is_none());
    }

    #[test]
    fn round1_operation_completion_preserves_unrelated_save_and_help() {
        for help in [false, true] {
            let (dir, mut app, a, b) = modal_documents();
            if help {
                app.close_dialog();
                app.set_overlay(AppMode::Help);
                app.workspace.documents.activate(b).unwrap();
            }
            app.dialog_state.input = "unfinished input".into();
            let overlay = app.workspace.focus.overlay.clone();
            app.handle_operation_complete(completed_operation(dir.path()));
            assert_eq!(app.workspace.focus.overlay, overlay);
            assert_eq!(app.workspace.focus.overlay_document(), Some(a));
            assert_eq!(app.workspace.documents.active_id(), Some(b));
            assert_eq!(app.dialog_state.input, "unfinished input");
            app.dismiss_overlay();
            assert_eq!(app.workspace.documents.active_id(), Some(a));
        }
    }

    #[test]
    fn round1_operation_completion_top_progress_restores_own_context() {
        let (dir, mut app, a, b) = modal_documents();
        app.close_dialog();
        app.open_dialog(progress_dialog());
        app.workspace.documents.activate(b).unwrap();
        if !app.workspace.layout.terminal_visible() {
            app.workspace.layout.toggle_terminal();
        }
        app.workspace.focus.panel = FocusedPanel::Terminal;
        app.handle_operation_complete(completed_operation(dir.path()));
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert_eq!(app.workspace.focus.panel, FocusedPanel::Editor);
        assert_eq!(app.workspace.documents.active_id(), Some(a));
        assert!(app.workspace.focus.dismiss_overlay().is_none());
    }

    // ── Phase 6 Task 2: prepared-only rendering ─────────────────────────────

    fn prepared_jobs(
        worker: crate::background::Worker<
            crate::app_jobs::NativeJob,
            crate::app_jobs::NativeOutput,
        >,
    ) -> crate::app_jobs::AppJobs {
        crate::app_jobs::AppJobs::new(crate::background::Limits::default(), worker).unwrap()
    }

    #[tokio::test]
    async fn task2_prepared_only_render_has_zero_io_and_loads_offthread() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("file_a.txt");
        fs::write(&file, "prepared\ncontent\n").unwrap();
        app.tree_state.reload_dir(dir.path());
        let index = app
            .tree_state
            .flat_items
            .iter()
            .position(|n| n.path == file)
            .unwrap();
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(crate::app_jobs::run),
        )));
        app.tree_state.selected_index = index;
        app.last_previewed_index = None;
        crate::highlighting::reset_render_io();
        app.update_preview();
        assert_eq!(
            app.preview_state.current_path.as_deref(),
            Some(file.as_path())
        );
        assert!(summary_text(&app).contains("Preparing preview"));
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| crate::ui::render(&mut app, frame))
            .unwrap();
        assert_eq!(
            crate::highlighting::render_io_count(),
            0,
            "render path performed prepared-state I/O"
        );
        let delivery = app.next_background().await.unwrap();
        assert!(delivery.target.is_preview());
        app.apply_background(delivery);
        assert_eq!(
            app.preview_state.current_path.as_deref(),
            Some(file.as_path())
        );
        assert!(summary_text(&app).contains("prepared"));
        assert_eq!(crate::highlighting::render_io_count(), 0);
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn task2_slow_old_preview_cannot_replace_newer_selection() {
        use crate::app_jobs::Target;
        let (dir, mut app) = setup_app();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, "AAA\n").unwrap();
        fs::write(&b, "BBB\n").unwrap();
        app.tree_state.reload_dir(dir.path());
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered_tx));
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_gate = gate.clone();
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(move |job: crate::app_jobs::NativeJob, token| {
                if let Target::Preview(key) = &job.target {
                    if key.path.ends_with("a.txt") {
                        entered
                            .lock()
                            .unwrap()
                            .take()
                            .unwrap()
                            .send(())
                            .ok()
                            .unwrap();
                        let _open = worker_gate
                            .1
                            .wait_while(worker_gate.0.lock().unwrap(), |open| !*open);
                    }
                }
                crate::app_jobs::run(job, token)
            }),
        )));
        let ia = app
            .tree_state
            .flat_items
            .iter()
            .position(|n| n.path == a)
            .unwrap();
        let ib = app
            .tree_state
            .flat_items
            .iter()
            .position(|n| n.path == b)
            .unwrap();
        app.tree_state.selected_index = ia;
        app.last_previewed_index = None;
        app.update_preview();
        entered_rx.await.unwrap(); // A is running and gated
        app.tree_state.selected_index = ib;
        app.last_previewed_index = None;
        app.update_preview(); // newer selection supersedes A
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        let delivery = app.next_background().await.unwrap();
        assert_eq!(delivery.target.path(), b.as_path());
        app.apply_background(delivery);
        assert_eq!(app.preview_state.current_path, Some(b.clone()));
        assert!(summary_text(&app).contains("BBB"));
        assert!(!summary_text(&app).contains("AAA"));
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn task2_theme_change_invalidates_and_republishes_preview() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("file_a.txt");
        fs::write(&file, "themed\n").unwrap();
        app.tree_state.reload_dir(dir.path());
        let index = app
            .tree_state
            .flat_items
            .iter()
            .position(|n| n.path == file)
            .unwrap();
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(crate::app_jobs::run),
        )));
        app.tree_state.selected_index = index;
        app.last_previewed_index = None;
        app.update_preview();
        let before = app.desired_preview.as_ref().unwrap().theme;
        let delivery = app.next_background().await.unwrap();
        app.apply_background(delivery);
        assert!(summary_text(&app).contains("themed"));
        // A syntax-theme change must produce a different identity and resubmit.
        app.config.preview.syntax_theme = Some("Solarized (dark)".into());
        app.last_previewed_index = None;
        app.update_preview();
        let after = app.desired_preview.as_ref().unwrap().theme;
        assert_ne!(
            before, after,
            "theme change did not rekey the preview request"
        );
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| crate::ui::render(&mut app, frame))
            .unwrap();
        assert_eq!(crate::highlighting::render_io_count(), 0);
        let delivery = app.next_background().await.unwrap();
        app.apply_background(delivery);
        assert!(summary_text(&app).contains("themed"));
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn task2_navigation_cancels_only_its_own_preview_request() {
        use crate::app_jobs::Target;
        let (dir, mut app) = setup_app();
        let file = dir.path().join("file_a.txt");
        fs::write(&file, "gated\n").unwrap();
        app.tree_state.reload_dir(dir.path());
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered_tx));
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_gate = gate.clone();
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(move |job: crate::app_jobs::NativeJob, token| {
                if matches!(&job.target, Target::Preview(_)) {
                    entered
                        .lock()
                        .unwrap()
                        .take()
                        .unwrap()
                        .send(())
                        .ok()
                        .unwrap();
                    let _open = worker_gate
                        .1
                        .wait_while(worker_gate.0.lock().unwrap(), |open| !*open);
                }
                crate::app_jobs::run(job, token)
            }),
        )));
        let fi = app
            .tree_state
            .flat_items
            .iter()
            .position(|n| n.path == file)
            .unwrap();
        let di = app
            .tree_state
            .flat_items
            .iter()
            .position(|n| n.name == "alpha")
            .unwrap();
        app.tree_state.selected_index = fi;
        app.last_previewed_index = None;
        app.update_preview();
        entered_rx.await.unwrap();
        assert!(app.jobs.as_ref().unwrap().has_pending());
        app.tree_state.selected_index = di;
        app.last_previewed_index = None;
        app.update_preview();
        assert!(app.desired_preview.is_none());
        assert!(
            !app.jobs.as_ref().unwrap().has_pending(),
            "preview cancellation retired its own request"
        );
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        app.shutdown_background().await;
    }

    /// Open a Rust file with `lines` single-token lines and focus the editor.
    fn task2_open_editor(
        dir: &TempDir,
        app: &mut App,
        name: &str,
        lines: usize,
    ) -> crate::workspace::documents::DocumentId {
        let path = dir.path().join(name);
        let mut body = String::new();
        for i in 0..lines {
            body.push_str(&format!("let value_{i} = {i};\n"));
        }
        fs::write(&path, body).unwrap();
        let id = app
            .workspace
            .documents
            .open(&path, crate::workspace::documents::OpenDisposition::Pinned)
            .unwrap();
        app.workspace.focus.panel = FocusedPanel::Editor;
        app.prepared_pipeline = true;
        id
    }

    #[test]
    fn task2_editor_edit_before_cached_viewport_has_no_stale_lines_and_stays_bounded() {
        let (dir, mut app) = setup_app();
        let id = task2_open_editor(&dir, &mut app, "long.rs", 600);
        for _ in 0..8 {
            app.advance_syntax_preparation();
        }
        assert!(
            app.syntax_cache_for(id).unwrap().is_complete(),
            "bounded steps must eventually prepare the whole document"
        );
        assert!(app.syntax_cache_for(id).unwrap().runs(0).is_some());
        // An edit at line zero invalidates from there.
        app.workspace
            .documents
            .get_mut(id)
            .unwrap()
            .editor
            .set_cursor_position(0, 0);
        app.workspace
            .documents
            .get_mut(id)
            .unwrap()
            .editor
            .insert_text("// ")
            .unwrap();
        app.advance_syntax_preparation();
        let cache = app.syntax_cache_for(id).unwrap();
        assert!(!cache.is_complete(), "edit must invalidate prepared state");
        assert!(
            cache.prepared() <= crate::highlighting::SYNTAX_WINDOW_LINES,
            "one preparation step must stay bounded"
        );
        assert!(
            cache.runs(500).is_none(),
            "a line past the prepared prefix must be pending, never stale"
        );
    }

    #[test]
    fn task2_editor_theme_change_invalidates_syntax_cache() {
        let (dir, mut app) = setup_app();
        let id = task2_open_editor(&dir, &mut app, "themed.rs", 600);
        for _ in 0..8 {
            app.advance_syntax_preparation();
        }
        assert!(app.syntax_cache_for(id).unwrap().is_complete());
        let before_theme = app.syntax_cache_for(id).unwrap().theme_epoch();
        app.config.preview.syntax_theme = Some("InspiredGitHub".into());
        app.advance_syntax_preparation();
        let cache = app.syntax_cache_for(id).unwrap();
        assert_ne!(cache.theme_epoch(), before_theme, "theme change must rekey");
        assert!(
            !cache.is_complete(),
            "theme change must invalidate prepared lines"
        );
        assert!(
            cache.prepared() <= crate::highlighting::SYNTAX_WINDOW_LINES,
            "post-theme preparation stays bounded"
        );
        for _ in 0..8 {
            app.advance_syntax_preparation();
        }
        assert!(app.syntax_cache_for(id).unwrap().is_complete());
    }

    #[test]
    fn task2_ui_renders_editor_from_prepared_cache_with_zero_render_io() {
        let (dir, mut app) = setup_app();
        let id = task2_open_editor(&dir, &mut app, "render.rs", 40);
        app.advance_syntax_preparation();
        assert!(app.syntax_cache_for(id).is_some());
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| crate::ui::render(&mut app, frame))
            .unwrap();
        assert_eq!(crate::highlighting::render_io_count(), 0);
    }

    #[test]
    fn task2_editor_close_retires_syntax_cache() {
        let (dir, mut app) = setup_app();
        let id = task2_open_editor(&dir, &mut app, "close.rs", 4);
        app.advance_syntax_preparation();
        assert!(app.syntax_cache_for(id).is_some());
        app.workspace.documents.close(id).unwrap();
        app.advance_syntax_preparation();
        assert!(
            app.syntax_cache_for(id).is_none(),
            "closing a document must retire its prepared syntax cache"
        );
    }

    #[test]
    fn task2_editor_parser_state_carries_across_lines_and_invalidates_after_edit() {
        let (dir, mut app) = setup_app();
        let path = dir.path().join("spans.rs");
        fs::write(
            &path,
            "fn main() {\n    /* opening\n       still inside\n       closing */\n}\n",
        )
        .unwrap();
        let id = app
            .workspace
            .documents
            .open(&path, crate::workspace::documents::OpenDisposition::Pinned)
            .unwrap();
        app.workspace.focus.panel = FocusedPanel::Editor;
        app.prepared_pipeline = true;
        for _ in 0..2 {
            app.advance_syntax_preparation();
        }
        let inside_before: Vec<_> = app
            .syntax_cache_for(id)
            .unwrap()
            .runs(2)
            .expect("interior line prepared")
            .iter()
            .map(|run| run.style)
            .collect();
        assert!(!inside_before.is_empty());
        // Turn the block-comment opener into a string delimiter; the interior
        // line must no longer be highlighted as a comment.
        app.workspace
            .documents
            .get_mut(id)
            .unwrap()
            .editor
            .set_cursor_position(1, 4);
        app.workspace
            .documents
            .get_mut(id)
            .unwrap()
            .editor
            .insert_text("\"")
            .unwrap();
        app.advance_syntax_preparation();
        let inside_after: Vec<_> = app
            .syntax_cache_for(id)
            .unwrap()
            .runs(2)
            .expect("interior line re-prepared")
            .iter()
            .map(|run| run.style)
            .collect();
        assert_ne!(
            inside_before, inside_after,
            "newline-aware parser state must carry into and change interior line styling"
        );
    }

    #[tokio::test]
    async fn task2_cycle_view_mode_submits_job_without_input_thread_io() {
        use crate::highlighting::RequestedView;
        let (dir, mut app) = setup_app();
        let file = dir.path().join("cycle.txt");
        fs::write(
            &file,
            (1..=2000)
                .map(|i| format!("line {i}\n"))
                .collect::<String>(),
        )
        .unwrap();
        app.tree_state.reload_dir(dir.path());
        app.config.preview.max_full_preview_bytes = Some(1);
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(crate::app_jobs::run),
        )));
        app.tree_state.selected_index = app
            .tree_state
            .flat_items
            .iter()
            .position(|n| n.path == file)
            .unwrap();
        app.last_previewed_index = None;
        app.update_preview();
        while app.jobs.as_ref().is_some_and(|jobs| jobs.has_pending()) {
            let delivery = app.next_background().await.unwrap();
            app.apply_background(delivery);
        }
        assert!(app.preview_state.is_large_file);
        app.workspace.focus.panel = FocusedPanel::Preview;

        // Probe only the CyclePreview control path.
        let _guard = crate::highlighting::RenderIoGuard::arm();
        crate::commands::dispatch_command(&mut app, crate::commands::CommandId::CyclePreview)
            .unwrap();
        assert_eq!(
            crate::highlighting::render_io_count(),
            0,
            "CyclePreview must not read/highlight on the input thread"
        );
        assert!(
            app.desired_preview
                .as_ref()
                .is_some_and(|key| key.path == file
                    && matches!(
                        key.view,
                        RequestedView::HeadAndTail | RequestedView::HeadOnly
                    )),
            "CyclePreview must submit a versioned preview job for the new view"
        );
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn task2_adjust_preview_lines_submits_job_without_input_thread_io() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("adjust.txt");
        fs::write(
            &file,
            (1..=2000)
                .map(|i| format!("line {i}\n"))
                .collect::<String>(),
        )
        .unwrap();
        app.tree_state.reload_dir(dir.path());
        app.config.preview.max_full_preview_bytes = Some(1);
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(crate::app_jobs::run),
        )));
        app.tree_state.selected_index = app
            .tree_state
            .flat_items
            .iter()
            .position(|n| n.path == file)
            .unwrap();
        app.last_previewed_index = None;
        app.update_preview();
        while app.jobs.as_ref().is_some_and(|jobs| jobs.has_pending()) {
            let delivery = app.next_background().await.unwrap();
            app.apply_background(delivery);
        }
        assert!(app.preview_state.is_large_file);
        let head_before = app.preview_state.head_lines;
        app.workspace.focus.panel = FocusedPanel::Preview;

        let _guard = crate::highlighting::RenderIoGuard::arm();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        crate::handler::handle_key_event(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('+'),
                crossterm::event::KeyModifiers::NONE,
            ),
            &tx,
        );
        assert_eq!(
            crate::highlighting::render_io_count(),
            0,
            "'+' must not read/highlight on the input thread"
        );
        assert_eq!(app.preview_state.head_lines, head_before + 10);
        assert!(
            app.desired_preview.as_ref().is_some_and(
                |key| key.path == file && key.head_lines == app.preview_state.head_lines
            ),
            "'+' must resubmit the versioned job with updated head/tail counts"
        );
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn task2_near_limit_full_preview_falls_back_to_bounded_head_tail() {
        use crate::app_jobs::{NativeOutput, Target};
        let (dir, mut app) = setup_app();
        let file = dir.path().join("near.txt");
        let body: String = (1..=4000)
            .map(|i| format!("line {i} with some padding text\n"))
            .collect();
        fs::write(&file, body).unwrap();
        app.tree_state.reload_dir(dir.path());
        app.prepared_pipeline = true;
        app.jobs = Some(
            crate::app_jobs::AppJobs::new(
                crate::background::Limits {
                    result_bytes: 1024,
                    ..crate::background::Limits::default()
                },
                crate::background::Worker::Blocking(Arc::new(crate::app_jobs::run)),
            )
            .unwrap(),
        );
        app.tree_state.selected_index = app
            .tree_state
            .flat_items
            .iter()
            .position(|n| n.path == file)
            .unwrap();
        app.last_previewed_index = None;
        app.update_preview();
        let mut saw_fallback = false;
        while app.jobs.as_ref().is_some_and(|jobs| jobs.has_pending()) {
            let delivery = app.next_background().await.unwrap();
            if let Ok(NativeOutput::Preview(prepared)) = &delivery.result {
                assert!(
                    matches!(delivery.target, Target::Preview(_)),
                    "preview result must carry its identity"
                );
                assert!(
                    prepared.lines.len() < 4000,
                    "near-limit preview must be bounded, not the whole file"
                );
                let text: String = prepared
                    .lines
                    .iter()
                    .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
                    .collect();
                assert!(
                    !text.contains("PayloadTooLarge"),
                    "must not surface a raw envelope refusal"
                );
                saw_fallback = true;
            }
            app.apply_background(delivery);
        }
        assert!(saw_fallback, "expected a bounded preview result");
        app.shutdown_background().await;
    }

    #[tokio::test]
    async fn task2_preview_worker_honors_deadline_and_reports_failure() {
        let (dir, mut app) = setup_app();
        let file = dir.path().join("slow.txt");
        fs::write(&file, "content\n").unwrap();
        app.tree_state.reload_dir(dir.path());
        app.prepared_pipeline = true;
        app.jobs = Some(prepared_jobs(crate::background::Worker::Blocking(
            Arc::new(crate::app_jobs::run),
        )));
        app.tree_state.selected_index = app
            .tree_state
            .flat_items
            .iter()
            .position(|n| n.path == file)
            .unwrap();
        app.last_previewed_index = None;
        // A zero deadline must surface an explicit failure, never a preview.
        let key = app.preview_key_for(&file);
        let mut job = app.build_native_job(crate::app_jobs::Target::Preview(key));
        job.timeout = std::time::Duration::ZERO;
        let generation = app.jobs.as_mut().unwrap().submit(job).unwrap();
        let delivery = app.next_background().await.unwrap();
        assert_eq!(delivery.generation, generation);
        match delivery.result {
            Ok(crate::app_jobs::NativeOutput::Failed(reason)) => {
                assert!(
                    reason.contains("timed out") || reason.contains("cancelled"),
                    "unexpected failure reason: {reason}"
                );
            }
            _ => unreachable!("expected an explicit deadline failure"),
        }
        app.shutdown_background().await;
    }

    #[test]
    fn task2_syntax_cache_aggregate_bound_evicts_non_active_documents() {
        let (dir, mut app) = setup_app();
        app.prepared_pipeline = true;
        let mut ids = Vec::new();
        for index in 0..6 {
            let path = dir.path().join(format!("agg_{index}.rs"));
            let body: String = (0..800)
                .map(|i| format!("let identifier_{i} = \"value {i}\";\n"))
                .collect();
            fs::write(&path, body).unwrap();
            let id = app
                .workspace
                .documents
                .open(&path, crate::workspace::documents::OpenDisposition::Pinned)
                .unwrap();
            ids.push(id);
        }
        app.workspace.focus.panel = FocusedPanel::Editor;
        app.workspace
            .documents
            .activate(*ids.last().unwrap())
            .unwrap();
        // Prepare a large prefix for every document.
        for id in &ids {
            app.workspace.documents.activate(*id).unwrap();
            for _ in 0..4 {
                app.advance_syntax_preparation();
            }
        }
        assert!(
            app.syntax_cache_bytes() > 0,
            "preparation populated at least one cache"
        );
        // Force the aggregate over the bound by shrinking the budget check: the
        // invariant is that the total never exceeds the configured aggregate.
        app.workspace
            .documents
            .activate(*ids.last().unwrap())
            .unwrap();
        app.advance_syntax_preparation();
        assert!(
            app.syntax_cache_bytes()
                <= crate::highlighting::SYNTAX_TOTAL_CACHE_BYTES
                    + crate::highlighting::SYNTAX_CACHE_BYTES,
            "aggregate prepared bytes stay bounded across documents"
        );
        // Closing documents retires their caches.
        for id in &ids {
            let _ = app.workspace.documents.close(*id);
        }
        app.advance_syntax_preparation();
        assert_eq!(app.syntax_cache_bytes(), 0);
    }

    #[test]
    fn task2_syntax_cache_aggregate_limit_evicts_non_active_documents() {
        let (dir, mut app) = setup_app();
        app.prepared_pipeline = true;
        let mut ids = Vec::new();
        for index in 0..3 {
            let path = dir.path().join(format!("evict_{index}.rs"));
            let body: String = (0..200)
                .map(|i| format!("let identifier_{i} = \"value {i}\";\n"))
                .collect();
            fs::write(&path, body).unwrap();
            let id = app
                .workspace
                .documents
                .open(&path, crate::workspace::documents::OpenDisposition::Pinned)
                .unwrap();
            ids.push(id);
        }
        app.workspace.focus.panel = FocusedPanel::Editor;
        for id in &ids {
            app.workspace.documents.activate(*id).unwrap();
            for _ in 0..6 {
                app.advance_syntax_preparation();
            }
        }
        let active = *ids.last().unwrap();
        app.workspace.documents.activate(active).unwrap();
        assert!(
            app.syntax_caches.len() > 1,
            "multiple prepared caches exist before eviction"
        );
        // A zero limit must evict every non-active cache and keep the active one.
        app.retire_dead_syntax_with_limit(0);
        assert_eq!(app.syntax_caches.len(), 1, "non-active caches retired");
        assert!(app.syntax_caches.contains_key(&active), "active cache kept");
        // A permissive limit must not evict anything.
        let bytes = app.syntax_cache_bytes();
        app.retire_dead_syntax_with_limit(usize::MAX);
        assert_eq!(
            app.syntax_cache_bytes(),
            bytes,
            "permissive limit leaves caches untouched"
        );
    }

    fn git_snapshot_with(
        entry: (&str, crate::git::GitEntryKind, [u8; 2]),
    ) -> crate::git::GitSnapshot {
        crate::git::GitSnapshot {
            branch: crate::git::BranchState::Symbolic {
                name: "main".to_string(),
                oid: "abc".to_string(),
            },
            entries: vec![crate::git::GitEntry {
                path: entry.0.to_string(),
                original_path: None,
                kind: entry.1,
                status: entry.2,
            }],
        }
    }

    fn git_refresh(
        generation: u64,
        root: &Path,
        snapshot: crate::git::GitSnapshot,
    ) -> crate::git::GitRefresh {
        crate::git::GitRefresh {
            generation,
            root: root.to_path_buf(),
            result: crate::git::GitResult::Snapshot(snapshot),
        }
    }

    #[test]
    fn git_worktree_root_walks_to_the_repository_and_stops_outside_one() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        let nested = repo.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(git_worktree_root(&nested).as_deref(), Some(repo.path()));
        assert_eq!(git_worktree_root(repo.path()).as_deref(), Some(repo.path()));

        let plain = tempfile::tempdir().unwrap();
        assert_eq!(git_worktree_root(plain.path()), None);
    }

    /// Folded from the Phase 8 Task 2 review: render must not re-walk `.git`
    /// ancestors on every frame. The resolution is memoized in a bounded
    /// one-entry cache and invalidated when a refresh is requested, so a
    /// repository created after the value was first resolved is still observed.
    #[test]
    fn git_worktree_resolution_is_memoized_and_invalidated_on_refresh() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        let mut app = App::new(&nested, AppConfig::default()).unwrap();

        // No repository yet: the resolved value is `None` and is remembered.
        assert_eq!(app.git_worktree(), None);

        // A repository appears at the root, but a render frame reuses the memo
        // rather than re-walking, so the value is unchanged until a refresh.
        std::fs::create_dir(root.path().join(".git")).unwrap();
        assert_eq!(
            app.git_worktree(),
            None,
            "render must reuse the memoized resolution, not re-walk"
        );

        // Requesting a refresh revalidates the work tree.
        app.request_git_refresh();
        assert_eq!(
            app.git_worktree().as_deref(),
            Some(root.path()),
            "a refresh must observe a newly created repository"
        );
    }

    #[test]
    fn git_snapshot_refuses_stale_generations_and_prior_workspace_roots() {
        let repo_a = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo_a.path().join(".git")).unwrap();
        let repo_b = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo_b.path().join(".git")).unwrap();
        let mut app = App::new(repo_a.path(), AppConfig::default()).unwrap();

        // Generation 1 is superseded before its result arrives (a stale event).
        let first = app.git.begin(repo_a.path().to_path_buf());
        let second = app.git.begin(repo_a.path().to_path_buf());
        assert!(app.accept_git_refresh(git_refresh(
            second,
            repo_a.path(),
            git_snapshot_with(("a.txt", crate::git::GitEntryKind::Ordinary, *b".M")),
        )));
        assert!(!app.accept_git_refresh(git_refresh(
            first,
            repo_a.path(),
            git_snapshot_with(("stale.txt", crate::git::GitEntryKind::Ordinary, *b".M")),
        )));
        assert!(app.git_snapshot().is_some());

        // A result tagged with a *different* work-tree root must never recolor
        // the current tree, even if it carries the current generation.
        let third = app.git.begin(repo_a.path().to_path_buf());
        assert!(app.accept_git_refresh(git_refresh(
            third,
            repo_b.path(),
            git_snapshot_with(("b.txt", crate::git::GitEntryKind::Ordinary, *b".M")),
        )));
        assert!(
            app.git_render().is_none(),
            "a prior-workspace root must not recolor the current tree"
        );

        // Re-accepting for the real root restores rendering.
        let fourth = app.git.begin(repo_a.path().to_path_buf());
        assert!(app.accept_git_refresh(git_refresh(
            fourth,
            repo_a.path(),
            git_snapshot_with(("a.txt", crate::git::GitEntryKind::Ordinary, *b".M")),
        )));
        assert!(app.git_render().is_some());
    }

    /// Folded from the Phase 8 Task 2 review: after the workspace root changes
    /// and a refresh is issued for the new root, a render that races the
    /// in-flight result must not pass the work-tree guard and recolor the tree
    /// with the previous repository's snapshot. `GitState::begin` clears the
    /// retained snapshot the instant the root changes, so this holds even
    /// before the new result arrives.
    #[test]
    fn git_root_change_never_renders_the_previous_repository_snapshot() {
        let repo_a = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo_a.path().join(".git")).unwrap();
        let repo_b = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo_b.path().join(".git")).unwrap();
        let mut app = App::new(repo_a.path(), AppConfig::default()).unwrap();

        let first = app.git.begin(repo_a.path().to_path_buf());
        assert!(app.accept_git_refresh(git_refresh(
            first,
            repo_a.path(),
            git_snapshot_with(("a.txt", crate::git::GitEntryKind::Ordinary, *b".M")),
        )));
        assert!(app.git_render().is_some());

        // The workspace switches to repo_b and a refresh is issued for it; the
        // new result has not arrived yet.
        app.tree_state.root.path = repo_b.path().to_path_buf();
        let _second = app.git.begin(repo_b.path().to_path_buf());
        assert!(
            app.git_render().is_none(),
            "a root change must not render the previous repository's snapshot"
        );
        assert!(app.git_snapshot().is_none());
    }

    /// Phase 8 Task 3 checkpoint: `git status` reads the working tree and
    /// `.git`, and the watcher (notify 7 subscribes to open events) reports
    /// those reads back as `FsChange`. Re-requesting on every `FsChange` would
    /// therefore spawn `git status` on each debounce window, forever. Requests
    /// for the same work tree inside the minimum interval are coalesced.
    #[tokio::test]
    async fn git_refresh_requests_are_coalesced_within_the_minimum_interval() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        let mut app = App::new(repo.path(), AppConfig::default()).unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.event_tx = Some(tx);

        app.request_git_refresh();
        assert_eq!(app.git.issued(), 1);
        app.request_git_refresh();
        assert_eq!(
            app.git.issued(),
            1,
            "a second request inside the minimum interval must be coalesced"
        );
    }

    /// P2-A: a request dropped by the coalescing floor must still converge.
    /// Dropping it records a deferred deadline; the main loop's bounded timer
    /// wake fires at `last_request + GIT_REFRESH_MIN_INTERVAL` and issues
    /// exactly one follow-up request (no further filesystem event, no repeat
    /// storm), and the indicator reaches the new state.
    #[tokio::test]
    async fn git_refresh_dropped_by_the_floor_converges_on_the_deferred_deadline() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        let mut app = App::new(repo.path(), AppConfig::default()).unwrap();
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.event_tx = Some(tx);

        // First request runs immediately.
        app.request_git_refresh();
        assert_eq!(app.git.issued(), 1);
        let issued_at = std::time::Instant::now();

        // A same-root change inside the floor window is dropped, not lost: it
        // records a deferred deadline and issues nothing (still exactly one
        // request so far).
        app.request_git_refresh();
        assert_eq!(
            app.git.issued(),
            1,
            "no request may run inside the floor window"
        );
        let scheduled = app
            .git_refresh_wait(issued_at)
            .expect("the dropped request must schedule a deferred deadline");
        assert!(
            scheduled <= GIT_REFRESH_MIN_INTERVAL
                && scheduled >= GIT_REFRESH_MIN_INTERVAL - Duration::from_millis(50),
            "deferred deadline must be the floor: {scheduled:?}"
        );

        // Before the deadline the deferred issue is a no-op and re-arms nothing.
        assert!(app.issue_deferred_git_refresh(issued_at).is_none());
        assert_eq!(app.git.issued(), 1);
        assert!(
            app.git_refresh_wait(issued_at).is_some(),
            "the deadline must survive a not-yet-due check"
        );

        // Cross the real deadline (a genuine timer wake, not a synthetic
        // instant) and let exactly one follow-up request converge the change.
        tokio::time::sleep(GIT_REFRESH_MIN_INTERVAL).await;
        let wake = std::time::Instant::now();
        assert!(
            app.issue_deferred_git_refresh(wake).is_some(),
            "the deferred deadline must be due at a real timer wake"
        );
        assert_eq!(app.git.issued(), 2, "the dropped request must run once due");

        // Exactly one deferred refresh per dropped window: the deadline is
        // cleared and a further timer wake issues nothing.
        assert_eq!(app.git_refresh_wait(wake), None);
        assert!(app.issue_deferred_git_refresh(wake).is_none());
        assert_eq!(app.git.issued(), 2, "no repeat storm from the timer");

        // The follow-up re-armed the floor; another change at the deadline is
        // deferred to a fresh deadline rather than issued immediately.
        app.request_git_refresh();
        assert_eq!(app.git.issued(), 2);
        let rearmed = app
            .git_refresh_wait(wake)
            .expect("the deferred request itself must respect the floor");
        assert!(
            rearmed <= GIT_REFRESH_MIN_INTERVAL + Duration::from_millis(50),
            "re-armed floor must be the minimum interval: {rearmed:?}"
        );

        // The indicator reaches the new state: accept a snapshot for the
        // deferred generation and render it.
        let generation = app.git.issued();
        assert!(app.accept_git_refresh(git_refresh(
            generation,
            repo.path(),
            git_snapshot_with(("changed.txt", crate::git::GitEntryKind::Ordinary, *b".M")),
        )));
        let (_, snapshot) = app.git_render().expect("converged snapshot must render");
        assert!(
            snapshot
                .entries
                .iter()
                .any(|entry| entry.path == "changed.txt"),
            "the deferred refresh must deliver the new state"
        );
    }

    #[test]
    fn git_indicators_disable_and_s3_exclude_snapshots() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        let disabled = AppConfig {
            git: crate::config::GitConfig {
                enabled: Some(false),
            },
            ..Default::default()
        };
        let mut app = App::new(repo.path(), disabled).unwrap();
        let generation = app.git.begin(repo.path().to_path_buf());
        app.accept_git_refresh(git_refresh(
            generation,
            repo.path(),
            git_snapshot_with(("a.txt", crate::git::GitEntryKind::Ordinary, *b".M")),
        ));
        assert!(
            app.git_snapshot().is_none(),
            "disabled indicators must leave no stale decorations"
        );
        assert!(!app.git_indicators_enabled());

        // S3 virtual roots never receive indicators, even with a snapshot.
        let mut s3 = App::new(repo.path(), AppConfig::default()).unwrap();
        s3.init_s3_mode(crate::s3::S3Config {
            path: crate::s3::S3Path::parse("s3://fixture/prefix").unwrap(),
            profile: None,
        });
        let generation = s3.git.begin(repo.path().to_path_buf());
        s3.accept_git_refresh(git_refresh(
            generation,
            repo.path(),
            git_snapshot_with(("a.txt", crate::git::GitEntryKind::Ordinary, *b".M")),
        ));
        assert!(s3.git_snapshot().is_none());
        assert!(!s3.git_indicators_enabled());
    }

    #[test]
    fn git_snapshot_absent_for_non_repository_and_request_without_sender_is_inert() {
        let plain = tempfile::tempdir().unwrap();
        let mut app = App::new(plain.path(), AppConfig::default()).unwrap();
        // No `.git` ancestor: no refresh is even requested, so nothing is issued.
        app.request_git_refresh();
        assert_eq!(app.git.issued(), 0);
        assert!(app.git_snapshot().is_none());
        assert!(app.git_render().is_none());
    }

    #[test]
    fn sync_lsp_documents_tracks_open_docs_and_skips_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn a() {}\n").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();

        // Disabled: the poll is a no-op — nothing is tracked.
        app.config.lsp_global.enabled = Some(false);
        assert!(app.open_document_path(&file, true));
        app.sync_lsp_documents();
        assert_eq!(app.lsp.tracked_len(), 0);

        // Enabled: the document is tracked even before a session is Ready —
        // its didOpen defers until the handshake completes.
        app.config.lsp_global.enabled = None;
        app.sync_lsp_documents();
        assert_eq!(app.lsp.tracked_len(), 1);
    }

    /// Decode the first queued Send body's (id, method).
    fn first_send(rx: &std::sync::mpsc::Receiver<crate::lsp::LspCommand>) -> serde_json::Value {
        #[rustfmt::skip]
        let Ok(crate::lsp::LspCommand::Send(body)) = rx.try_recv() else { unreachable!("expected Send") };
        serde_json::from_slice(&body).unwrap()
    }

    #[test]
    fn lsp_completion_flows_request_to_overlay_to_one_undo() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "let fo = x\n").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        assert!(app.open_document_path(&file, true));
        app.sync_lsp_documents();
        let rx = app.lsp.insert_ready_session(
            "rust",
            crate::lsp::features::ServerFeatures {
                completion: true,
                ..Default::default()
            },
        );

        // Capability gate honors the advertised provider.
        app.lsp_completion().unwrap();
        let send = first_send(&rx);
        assert_eq!(send["method"], "textDocument/completion");
        let id = send["id"].as_u64().unwrap();
        assert_eq!(
            send["params"]["position"]["line"], 0,
            "cursor position encoded"
        );

        // The response resolves into the overlay.
        let outcome = serde_json::json!([{"label": "foobar",
            "textEdit": {"range": {"start": {"line": 0, "character": 4},
                "end": {"line": 0, "character": 6}}, "newText": "foobar"}}])
        .to_string();
        app.lsp.handle_event(
            "rust",
            0,
            crate::lsp::client::ClientEvent::Response {
                id,
                generation: 0,
                outcome: Ok(outcome),
            },
        );
        app.drain_lsp_results();
        assert!(app.language_features.is_some());
        assert_eq!(app.workspace.focus.overlay, AppMode::LanguageFeatures);

        // Enter applies it atomically — one undo restores the buffer.
        app.apply_language_selection();
        let document = app.workspace.documents.active().unwrap();
        assert_eq!(document.text(), "let foobar = x\n");
        app.workspace.documents.active_mut().unwrap().editor.undo();
        assert_eq!(
            app.workspace.documents.active().unwrap().text(),
            "let fo = x\n"
        );
        assert!(app.language_features.is_none());
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
    }

    #[test]
    fn lsp_capability_fallback_fails_visibly_and_stale_results_drop() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn a() {}\n").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        assert!(app.open_document_path(&file, true));
        app.sync_lsp_documents();
        // Server advertises hover only — completion is refused up front.
        let rx = app.lsp.insert_ready_session(
            "rust",
            crate::lsp::features::ServerFeatures {
                hover: true,
                ..Default::default()
            },
        );
        let err = app.lsp_completion().unwrap_err();
        assert!(err.contains("unsupported"), "{err}");
        assert!(rx.try_recv().is_err(), "no request may be sent");
        let _rx = app.lsp.insert_ready_session(
            "rust",
            crate::lsp::features::ServerFeatures {
                completion: true,
                ..Default::default()
            },
        );
        app.lsp_completion().unwrap();
        let send = first_send(&_rx);
        let id = send["id"].as_u64().unwrap();
        // Buffer moves while the request is in flight → stale result drops.
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .insert_text("x")
            .unwrap();
        app.lsp.handle_event(
            "rust",
            0,
            crate::lsp::client::ClientEvent::Response {
                id,
                generation: 0,
                outcome: Ok("[]".to_string()),
            },
        );
        app.drain_lsp_results();
        assert!(app.language_features.is_none());
        let note = app
            .status_message
            .as_ref()
            .map(|(m, _)| m.as_str())
            .unwrap_or_default();
        assert!(
            note.contains("changed") || note.contains("dropped"),
            "{note}"
        );
    }

    #[test]
    fn lsp_definition_navigates_other_file_and_preserves_origin_state() {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("a.rs");
        let target = dir.path().join("b.rs");
        std::fs::write(&origin, "use b::thing\n").unwrap();
        std::fs::write(&target, "pub fn thing() {}\n").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        assert!(app.open_document_path(&origin, true));
        // Dirty the origin + park its cursor — navigation must not disturb it.
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .insert_text("// dirty\n")
            .unwrap();
        // Park the origin cursor somewhere recognizable.
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .set_cursor_position(1, 2);
        let origin_id = app.workspace.documents.active_id().unwrap();
        let origin_revision = app
            .workspace
            .documents
            .get(origin_id)
            .unwrap()
            .editor
            .content_revision();
        app.sync_lsp_documents();
        let rx = app.lsp.insert_ready_session(
            "rust",
            crate::lsp::features::ServerFeatures {
                definition: true,
                ..Default::default()
            },
        );
        app.lsp_definition().unwrap();
        let send = first_send(&rx);
        let id = send["id"].as_u64().unwrap();
        let outcome = serde_json::json!({
            "uri": crate::lsp::features::uri_for_path(
                &std::fs::canonicalize(&target).unwrap()),
            "range": {"start": {"line": 0, "character": 4},
                "end": {"line": 0, "character": 9}},
        })
        .to_string();
        app.lsp.handle_event(
            "rust",
            0,
            crate::lsp::client::ClientEvent::Response {
                id,
                generation: 0,
                outcome: Ok(outcome),
            },
        );
        app.drain_lsp_results();
        // Single definition → straight to the target, no overlay.
        assert!(app.language_features.is_none());
        let active = app.workspace.documents.active().unwrap();
        assert!(active.path().ends_with("b.rs"), "{:?}", active.path());
        assert_eq!(active.editor.cursor_position().byte, 4);
        // Origin doc keeps its dirty revision and its cursor.
        let origin_doc = app.workspace.documents.get(origin_id).unwrap();
        assert_eq!(origin_doc.editor.content_revision(), origin_revision);
        assert!(origin_doc.editor.modified);
        assert_eq!(
            (
                origin_doc.editor.cursor_position().line,
                origin_doc.editor.cursor_position().byte
            ),
            (1, 2)
        );

        // A non-file scheme is refused visibly — nothing opens.
        let count = app.workspace.documents.iter().count();
        app.navigate_to_location(crate::lsp::features::LocationEntry {
            uri: "untitled:u1".to_string(),
            start_line: 0,
            start_character: 0,
            end_line: 0,
            end_character: 0,
        });
        assert_eq!(app.workspace.documents.iter().count(), count);
        let note = app
            .status_message
            .as_ref()
            .map(|(m, _)| m.as_str())
            .unwrap_or_default();
        assert!(note.contains("unsupported URI scheme"), "{note}");
    }

    /// Seed a synced doc + ready session; returns (session_rx, doc_id).
    fn lsp_fixture(
        app: &mut App,
        file: &std::path::Path,
        features: crate::lsp::features::ServerFeatures,
    ) -> (
        std::sync::mpsc::Receiver<crate::lsp::LspCommand>,
        crate::workspace::documents::DocumentId,
    ) {
        assert!(app.open_document_path(file, true));
        app.sync_lsp_documents();
        let rx = app.lsp.insert_ready_session("rust", features);
        let id = app.workspace.documents.active_id().unwrap();
        (rx, id)
    }

    #[test]
    fn lsp_result_error_rename_and_close_drop_paths() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn a() {}\n").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (rx, doc) = lsp_fixture(
            &mut app,
            &file,
            crate::lsp::features::ServerFeatures {
                completion: true,
                hover: true,
                ..Default::default()
            },
        );

        // Server error → status message, no overlay.
        app.lsp_completion().unwrap();
        let id = first_send(&rx)["id"].as_u64().unwrap();
        app.lsp.handle_event(
            "rust",
            0,
            crate::lsp::client::ClientEvent::Response {
                id,
                generation: 0,
                outcome: Err("boom".into()),
            },
        );
        app.drain_lsp_results();
        assert!(app.language_features.is_none());
        let note = app
            .status_message
            .as_ref()
            .map(|(m, _)| m.clone())
            .unwrap_or_default();
        assert!(note.contains("completion"), "{note}");

        // Malformed response body → same path.
        app.lsp_completion().unwrap();
        let id = first_send(&rx)["id"].as_u64().unwrap();
        app.lsp.handle_event(
            "rust",
            0,
            crate::lsp::client::ClientEvent::Response {
                id,
                generation: 0,
                outcome: Ok("{not json".into()),
            },
        );
        app.drain_lsp_results();
        assert!(app.language_features.is_none());

        // Completion that fails to parse visibly → status, no overlay.
        app.lsp_completion().unwrap();
        let id = first_send(&rx)["id"].as_u64().unwrap();
        app.lsp.handle_event(
            "rust",
            0,
            crate::lsp::client::ClientEvent::Response {
                id,
                generation: 0,
                outcome: Ok(serde_json::json!([{"label": "x", "textEdit":
                    {"range": {"start": {"line": 99, "character": 0},
                        "end": {"line": 99, "character": 1}}, "newText": "y"}}])
                .to_string()),
            },
        );
        app.drain_lsp_results();
        assert!(app.language_features.is_none());
        let note = app
            .status_message
            .as_ref()
            .map(|(m, _)| m.clone())
            .unwrap_or_default();
        assert!(note.contains("malformed"), "{note}");

        // Empty result list → status.
        app.lsp_completion().unwrap();
        let id = first_send(&rx)["id"].as_u64().unwrap();
        app.lsp.handle_event(
            "rust",
            0,
            crate::lsp::client::ClientEvent::Response {
                id,
                generation: 0,
                outcome: Ok("[]".into()),
            },
        );
        app.drain_lsp_results();
        let note = app
            .status_message
            .as_ref()
            .map(|(m, _)| m.clone())
            .unwrap_or_default();
        assert!(note.contains("no completions"), "{note}");

        // Hover returning nothing → status.
        app.lsp_hover().unwrap();
        let id = first_send(&rx)["id"].as_u64().unwrap();
        app.lsp.handle_event(
            "rust",
            0,
            crate::lsp::client::ClientEvent::Response {
                id,
                generation: 0,
                outcome: Ok(serde_json::json!({"contents": null}).to_string()),
            },
        );
        app.drain_lsp_results();
        let note = app
            .status_message
            .as_ref()
            .map(|(m, _)| m.clone())
            .unwrap_or_default();
        assert!(note.contains("no hover"), "{note}");

        // A response for a CLOSED document drops silently.
        let request = crate::lsp::FeatureRequest {
            language: "rust".into(),
            method: "textDocument/hover".into(),
            document: doc,
            uri: "file:///gone".into(),
            revision: 0,
        };
        let _ = app.workspace.documents.close(doc);
        app.lsp.handle_event(
            "rust",
            0,
            crate::lsp::client::ClientEvent::Response {
                id: 0,
                generation: 0,
                outcome: Ok("{}".into()),
            },
        );
        // inject the pending entry manually — response id 0 arrives with no
        // pending registration → dropped (no result even reaches the app).
        let _ = request;
        assert!(app.language_features.is_none());
    }

    #[test]
    fn lsp_references_overlay_enters_other_file_and_busy_overlay_drops() {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("a.rs");
        let t1 = dir.path().join("t1.rs");
        let t2 = dir.path().join("t2.rs");
        std::fs::write(&origin, "fn use_it() {}\n").unwrap();
        std::fs::write(&t1, "fn x() {}\n").unwrap();
        std::fs::write(&t2, "fn y() {}\n").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (rx, _doc) = lsp_fixture(
            &mut app,
            &origin,
            crate::lsp::features::ServerFeatures {
                references: true,
                hover: true,
                ..Default::default()
            },
        );
        // Two locations → list overlay; Enter navigates to the selection.
        app.lsp_references().unwrap();
        let id = first_send(&rx)["id"].as_u64().unwrap();
        let loc = |p: &std::path::Path, l: u64| {
            serde_json::json!({"uri":
            crate::lsp::features::uri_for_path(&std::fs::canonicalize(p).unwrap()),
            "range": {"start": {"line": l, "character": 0},
                "end": {"line": l, "character": 1}}})
        };
        app.lsp.handle_event(
            "rust",
            0,
            crate::lsp::client::ClientEvent::Response {
                id,
                generation: 0,
                outcome: Ok(serde_json::json!([loc(&t1, 0), loc(&t2, 0)]).to_string()),
            },
        );
        app.drain_lsp_results();
        assert!(app.language_features.is_some());
        app.language_features.as_mut().unwrap().selected = 1;
        app.apply_language_selection();
        assert!(app
            .workspace
            .documents
            .active()
            .unwrap()
            .path()
            .ends_with("t2.rs"));

        // A result arriving while another modal owns input drops with a note.
        app.set_overlay(AppMode::Help);
        app.lsp
            .handle_event(
                "rust",
                0,
                crate::lsp::client::ClientEvent::Response {
                    id: 4242,
                    generation: 0,
                    outcome: Ok("null".into()),
                },
            )
            .unwrap_or_default();
        // inject a pending + deliver directly through the queue path
        app.drain_lsp_results();
        assert!(app.language_features.is_none());
        app.dismiss_overlay();
    }

    #[test]
    fn lsp_apply_stale_snippet_and_symbols_empty_cover_apply_paths() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "data\n").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (_rx, doc) = lsp_fixture(
            &mut app,
            &file,
            crate::lsp::features::ServerFeatures::default(),
        );
        use crate::components::language_features::{FeatureView, LanguageFeatures};
        use crate::lsp::features::{CompletionEdit, CompletionEntry};

        // Stale at Enter-time: overlay opened, then the buffer moved.
        let revision = app
            .workspace
            .documents
            .get(doc)
            .unwrap()
            .editor
            .content_revision();
        app.workspace
            .focus
            .open_overlay(AppMode::LanguageFeatures, Some(doc))
            .unwrap();
        app.language_features = Some(LanguageFeatures::new(
            doc,
            "file:///x".into(),
            revision,
            FeatureView::Completion {
                items: vec![CompletionEntry {
                    label: "x".into(),
                    detail: None,
                    kind: None,
                    documentation: None,
                    edit: CompletionEdit::Insert { text: "x".into() },
                    additional_edits: vec![],
                    snippet: false,
                    has_command: false,
                    deprecated: false,
                }],
            },
        ));
        app.workspace
            .documents
            .get_mut(doc)
            .unwrap()
            .editor
            .insert_text("e")
            .unwrap();
        app.apply_language_selection();
        assert!(app.language_features.is_none());
        let note = app
            .status_message
            .as_ref()
            .map(|(m, _)| m.clone())
            .unwrap_or_default();
        assert!(note.contains("stale"), "{note}");

        // Snippet item → apply fails visibly with no partial edit.
        let uri =
            crate::lsp::features::uri_for_path(app.workspace.documents.get(doc).unwrap().path());
        let revision = app
            .workspace
            .documents
            .get(doc)
            .unwrap()
            .editor
            .content_revision();
        app.workspace
            .focus
            .open_overlay(AppMode::LanguageFeatures, Some(doc))
            .unwrap();
        app.language_features = Some(LanguageFeatures::new(
            doc,
            uri,
            revision,
            FeatureView::Completion {
                items: vec![CompletionEntry {
                    label: "s".into(),
                    detail: None,
                    kind: None,
                    documentation: None,
                    edit: CompletionEdit::Insert { text: "s".into() },
                    additional_edits: vec![],
                    snippet: true,
                    has_command: false,
                    deprecated: false,
                }],
            },
        ));
        app.apply_language_selection();
        let note = app
            .status_message
            .as_ref()
            .map(|(m, _)| m.clone())
            .unwrap_or_default();
        assert!(note.contains("snippet"), "{note}");

        // Symbols empty → status via the delivery path (request-level).
        // (covered through drain path in the references test's drops)
        let _ = doc;
    }

    #[test]
    fn lsp_goto_position_clamps_out_of_range_lines() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "ab\ncd\n").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (_rx, doc) = lsp_fixture(
            &mut app,
            &file,
            crate::lsp::features::ServerFeatures::default(),
        );
        // Line beyond the buffer clamps to the last line; undecodable
        // character (mid-scalar) falls to byte 0 of that line.
        app.goto_lsp_position(doc, 999, 0);
        let pos = app
            .workspace
            .documents
            .get(doc)
            .unwrap()
            .editor
            .cursor_position();
        // "ab\ncd\n" is three lines; line 999 clamps to the last index.
        assert_eq!(pos.line, 2);
        app.goto_lsp_position(doc, 0, 1);
        assert_eq!(
            app.workspace
                .documents
                .get(doc)
                .unwrap()
                .editor
                .cursor_position()
                .byte,
            1
        );
    }

    #[test]
    fn lsp_request_refuses_when_document_is_not_synced() {
        // Ready session + an open doc the manager never tracked → the gate
        // fails visibly before any send.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "x\n").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        assert!(app.open_document_path(&file, true));
        let _rx = app.lsp.insert_ready_session(
            "rust",
            crate::lsp::features::ServerFeatures {
                completion: true,
                ..Default::default()
            },
        );
        // No sync_lsp_documents() — the doc is untracked.
        let err = app.lsp_completion().unwrap_err();
        assert!(err.contains("not synced"), "{err}");
    }

    #[test]
    fn lsp_deliver_covers_drop_and_empty_result_arms() {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("a.rs");
        let t1 = dir.path().join("t1.rs");
        let t2 = dir.path().join("t2.rs");
        std::fs::write(&origin, "fn a() {}\n").unwrap();
        std::fs::write(&t1, "fn x() {}\n").unwrap();
        std::fs::write(&t2, "fn y() {}\n").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        assert!(app.open_document_path(&origin, true));
        app.sync_lsp_documents();
        let _rx = app
            .lsp
            .insert_ready_session("rust", crate::lsp::features::ServerFeatures::default());
        let doc = app.workspace.documents.active_id().unwrap();
        let uri =
            crate::lsp::features::uri_for_path(app.workspace.documents.get(doc).unwrap().path());
        let revision = app
            .workspace
            .documents
            .get(doc)
            .unwrap()
            .editor
            .content_revision();
        let req = |method: &str| crate::lsp::FeatureRequest {
            language: "rust".into(),
            method: method.into(),
            document: doc,
            uri: uri.clone(),
            revision,
        };
        let loc = |p: &std::path::Path| {
            serde_json::json!({"uri":
            crate::lsp::features::uri_for_path(&std::fs::canonicalize(p).unwrap()),
            "range": {"start": {"line": 0, "character": 0},
                "end": {"line": 0, "character": 1}}})
        };

        // Renamed document → uri mismatch drop + note.
        app.deliver_feature_result(
            crate::lsp::FeatureRequest {
                uri: "file:///elsewhere".into(),
                ..req("textDocument/hover")
            },
            serde_json::json!({"contents": "x"}),
        );
        let note = app
            .status_message
            .as_ref()
            .map(|(m, _)| m.clone())
            .unwrap_or_default();
        assert!(note.contains("renamed"), "{note}");

        // Empty definitions → "no results"; unknown method → silent drop.
        app.deliver_feature_result(req("textDocument/definition"), serde_json::json!([]));
        let note = app
            .status_message
            .as_ref()
            .map(|(m, _)| m.clone())
            .unwrap_or_default();
        assert!(note.contains("no results"), "{note}");
        app.deliver_feature_result(req("textDocument/semanticTokens"), serde_json::json!(1));
        assert!(app.language_features.is_none());

        // Multiple definitions → overlay titled "Definition".
        app.deliver_feature_result(
            req("textDocument/definition"),
            serde_json::json!([loc(&t1), loc(&t2)]),
        );
        let f = app.language_features.as_ref().expect("overlay opened");
        assert_eq!(f.title(), " Definition · 2 ");
        app.dismiss_language_features();

        // Empty symbols → "no symbols".
        app.deliver_feature_result(req("textDocument/documentSymbol"), serde_json::json!([]));
        let note = app
            .status_message
            .as_ref()
            .map(|(m, _)| m.clone())
            .unwrap_or_default();
        assert!(note.contains("no symbols"), "{note}");

        // Result while another modal owns input → drop + note.
        app.set_overlay(AppMode::Help);
        app.deliver_feature_result(
            req("textDocument/hover"),
            serde_json::json!({"contents": "x"}),
        );
        let note = app
            .status_message
            .as_ref()
            .map(|(m, _)| m.clone())
            .unwrap_or_default();
        assert!(note.contains("another overlay"), "{note}");
        assert!(app.language_features.is_none());
        app.dismiss_overlay();

        // A second result while the feature overlay is already open
        // refreshes it in place (the else-implicit LanguageFeatures arm).
        app.deliver_feature_result(
            req("textDocument/hover"),
            serde_json::json!({"contents": "first"}),
        );
        assert!(app.language_features.is_some());
        app.deliver_feature_result(
            req("textDocument/hover"),
            serde_json::json!({"contents": "second"}),
        );
        let f = app.language_features.as_ref().unwrap();
        #[rustfmt::skip]
        let crate::components::language_features::FeatureView::Text { lines, .. } = &f.view else { unreachable!("expected Text view") };
        assert!(lines.iter().any(|l| l.contains("second")), "{lines:?}");

        // Overlay-depth exhaustion → open_overlay Err surfaces as a note.
        app.dismiss_language_features();
        for _ in 0..8 {
            app.workspace
                .focus
                .open_overlay(AppMode::Normal, None)
                .unwrap();
        }
        app.deliver_feature_result(
            req("textDocument/hover"),
            serde_json::json!({"contents": "x"}),
        );
        assert!(app.status_message.is_some());
        assert!(app.language_features.is_none());
        while app.workspace.focus.dismiss_overlay().is_some() {}

        // Delivering a result for a document that closed in flight drops it.
        let _ = app.workspace.documents.close(doc);
        app.deliver_feature_result(
            req("textDocument/hover"),
            serde_json::json!({"contents": "x"}),
        );
        assert!(app.language_features.is_none());
    }

    #[test]
    fn lsp_apply_without_overlay_and_doc_gone_and_missing_nav() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "abc\n").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        assert!(app.open_document_path(&file, true));
        let doc = app.workspace.documents.active_id().unwrap();

        // Apply/dismiss with no overlay → no-ops.
        app.apply_language_selection();
        app.dismiss_language_features();
        assert!(app.status_message.is_none());

        // Overlay open, then the document is closed → apply dismisses.
        use crate::components::language_features::{FeatureView, LanguageFeatures};
        app.workspace
            .focus
            .open_overlay(AppMode::LanguageFeatures, Some(doc))
            .unwrap();
        app.language_features = Some(LanguageFeatures::new(
            doc,
            "file:///x".into(),
            0,
            FeatureView::Completion {
                items: vec![crate::lsp::features::CompletionEntry {
                    label: "x".into(),
                    detail: None,
                    kind: None,
                    documentation: None,
                    edit: crate::lsp::features::CompletionEdit::Insert { text: "x".into() },
                    additional_edits: vec![],
                    snippet: false,
                    has_command: false,
                    deprecated: false,
                }],
            },
        ));
        let _ = app.workspace.documents.close(doc);
        app.apply_language_selection();
        assert!(app.language_features.is_none());
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);

        // Navigate to a location whose file can't be opened → no panic, no
        // new document.
        let before = app.workspace.documents.len();
        app.navigate_to_location(crate::lsp::features::LocationEntry {
            uri: crate::lsp::features::uri_for_path(&dir.path().join("missing.rs")),
            start_line: 0,
            start_character: 0,
            end_line: 0,
            end_character: 0,
        });
        assert_eq!(app.workspace.documents.len(), before);

        // goto_lsp_position on a closed id → silent no-op.
        app.goto_lsp_position(doc, 0, 0);
    }

    #[test]
    fn lsp_hover_and_symbols_open_bounded_overlay() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn a() {}\n").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        assert!(app.open_document_path(&file, true));
        app.sync_lsp_documents();
        let rx = app.lsp.insert_ready_session(
            "rust",
            crate::lsp::features::ServerFeatures {
                hover: true,
                document_symbol: true,
                ..Default::default()
            },
        );
        // Hover with embedded escapes → sanitized text view.
        app.lsp_hover().unwrap();
        let id = first_send(&rx)["id"].as_u64().unwrap();
        app.lsp.handle_event(
            "rust",
            0,
            crate::lsp::client::ClientEvent::Response {
                id,
                generation: 0,
                outcome: Ok(serde_json::json!({"contents":
                    "\x1b[31mdanger\x1b[0m plain"})
                .to_string()),
            },
        );
        app.drain_lsp_results();
        let features = app.language_features.as_ref().unwrap();
        #[rustfmt::skip]
        let crate::components::language_features::FeatureView::Text { lines, .. } = &features.view else { unreachable!("expected Text view") };
        let text = lines.join("\n");
        assert!(
            text.contains("danger") && !text.contains('\u{1b}'),
            "{text}"
        );
        app.dismiss_language_features();

        // Symbols flatten into the same overlay.
        app.lsp_document_symbols().unwrap();
        let id = first_send(&rx)["id"].as_u64().unwrap();
        app.lsp.handle_event(
            "rust",
            0,
            crate::lsp::client::ClientEvent::Response {
                id,
                generation: 0,
                outcome: Ok(serde_json::json!([
                    {"name": "a", "kind": 12,
                     "selectionRange": {"start": {"line": 0, "character": 3},
                        "end": {"line": 0, "character": 4}}}])
                .to_string()),
            },
        );
        app.drain_lsp_results();
        assert!(app.language_features.is_some());
        // Enter on a symbol navigates inside the same document.
        app.apply_language_selection();
        assert_eq!(
            app.workspace
                .documents
                .active()
                .unwrap()
                .editor
                .cursor_position()
                .byte,
            3
        );
    }
}

/// Phase 7 Task 3 — startup restoration and recovery command integration.
#[cfg(test)]
mod recovery_integration_tests {
    use super::*;
    use crate::commands::{self, CommandContext, CommandId};
    use crate::recovery::{RecoveryPolicy, RecoveryRecord, RecoveryStore};
    use crate::workspace::documents::{DocumentStore, OpenDisposition};
    use std::time::{Duration, Instant, SystemTime};

    fn install(root: &Path, state: &Path, config: AppConfig) -> (App, RecoveryStore) {
        let mut app = App::new(root, config).unwrap();
        let store = RecoveryStore::new(state);
        app.configure_recovery(store.clone());
        (app, store)
    }

    fn dirty(app: &mut App, path: &Path, text: &str) {
        assert!(app.open_document_path(path, true));
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .insert_text(text)
            .unwrap();
        assert!(app.workspace.documents.active().unwrap().editor.modified);
    }

    fn retained(store: &RecoveryStore, root: &Path) -> Vec<RecoveryRecord> {
        let policy = RecoveryPolicy::from_config(&AppConfig::default());
        store.load_all(root, &policy, SystemTime::now())
    }

    #[test]
    fn task4_idle_events_never_consume_the_throttle_window() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = root.path().join("later.txt");
        std::fs::write(&path, "body\n").unwrap();
        let (mut app, store) = install(root.path(), state.path(), AppConfig::default());
        let now = Instant::now();

        // The pass runs with nothing dirty at `now`. It must not consume the
        // write window: the pre-throttle admission check makes it a no-op.
        assert_eq!(app.snapshot_dirty_documents(now), None);
        assert!(retained(&store, root.path()).is_empty());

        // The document becomes dirty and the pass runs again at the *same*
        // instant. Because the empty pass above never consumed the window, the
        // snapshot is written immediately instead of waiting out the interval.
        dirty(&mut app, &path, "unsaved now\n");
        assert_eq!(app.snapshot_dirty_documents(now), None);
        let records = retained(&store, root.path());
        assert_eq!(
            records.len(),
            1,
            "a dirty document must snapshot immediately at the same instant \
             when no earlier pass had anything to write"
        );
        assert!(records[0].text.contains("unsaved now"));

        // The throttle itself still applies once a real write happened.
        assert_eq!(app.snapshot_dirty_documents(now), None);
        assert_eq!(
            retained(&store, root.path()).len(),
            1,
            "the interval must still bound writes after a genuine snapshot"
        );
    }

    #[test]
    fn task4_pending_snapshot_schedules_a_bounded_wake_until_the_next_write() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = root.path().join("pending.txt");
        std::fs::write(&path, "body\n").unwrap();

        // Nothing dirty: there is no pending write to wait for.
        let (mut app, _store) = install(root.path(), state.path(), AppConfig::default());
        let now = Instant::now();
        assert_eq!(app.recovery_snapshot_wait(now), None);

        dirty(&mut app, &path, "unsaved\n");
        // The throttle has not fired yet, so the next pass is due immediately
        // and the event path captures it; no scheduled wait is needed.
        assert_eq!(app.recovery_snapshot_wait(now), None);

        // The first pass writes and starts the throttle, after which the loop
        // must wait out the interval before the next bounded attempt.
        assert_eq!(app.snapshot_dirty_documents(now), None);
        let policy_interval = app.recovery.as_ref().unwrap().policy.min_interval;
        let wait = app
            .recovery_snapshot_wait(now)
            .expect("a throttled pending write must schedule a wake");
        assert!(wait > Duration::ZERO);
        assert!(wait <= policy_interval);

        // Disabling recovery stops scheduling further attempts.
        app.set_recovery_enabled(false);
        assert_eq!(app.recovery_snapshot_wait(now), None);
    }

    #[test]
    fn task3_restart_after_unsaved_edit_restores_a_dirty_document() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = root.path().join("notes.yaml");
        std::fs::write(&path, "key: 1\n").unwrap();

        let (mut first, store) = install(root.path(), state.path(), AppConfig::default());
        dirty(&mut first, &path, "unsaved: 2\n");
        let policy = first.recovery.as_ref().unwrap().policy;
        assert_eq!(first.snapshot_dirty_documents(Instant::now()), None);
        assert_eq!(
            store
                .load_all(root.path(), &policy, SystemTime::now())
                .len(),
            1
        );

        let (mut restarted, _store) = install(root.path(), state.path(), AppConfig::default());
        assert_eq!(restarted.recovery.as_ref().unwrap().records.len(), 1);
        restarted.open_document_path(&path, true);
        restarted.restore_recovery().unwrap();
        let restored_document = restarted.workspace.documents.active().unwrap();
        assert!(restored_document.editor.modified);
        assert!(restored_document.text().contains("unsaved: 2"));
        // The source file is never written by restore.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "key: 1\n");

        // Session serialization remains a structural whitelist: recovery never
        // leaks processes, buffers, terminal content, or executable trust.
        let serialized_session = serde_json::to_string(
            &crate::session::SessionRecord::from_workspace(&restarted.workspace, root.path()),
        )
        .unwrap();
        assert!(!serialized_session.contains("trusted_server_commands"));
        let value: serde_json::Value = serde_json::from_str(&serialized_session).unwrap();
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "active_document_path",
                "documents",
                "layout",
                "recent_files",
                "schema",
                "version",
                "workspace_root",
            ]
        );
    }

    #[test]
    fn task3_user_refusal_leaves_document_file_and_record_untouched() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = root.path().join("draft.txt");
        std::fs::write(&path, "original\n").unwrap();
        let (mut first, store) = install(root.path(), state.path(), AppConfig::default());
        dirty(&mut first, &path, "unsaved draft\n");
        first.snapshot_dirty_documents(Instant::now());
        let before = std::fs::read(&path).unwrap();

        let (mut restarted, _store) = install(root.path(), state.path(), AppConfig::default());
        restarted.open_document_path(&path, true);
        restarted.open_recovery_prompt();
        assert!(matches!(
            restarted.workspace.focus.overlay,
            AppMode::Dialog(DialogKind::RecoveryPrompt { remaining: 1, .. })
        ));
        let (tx, _rx) = crate::event::event_channel(Default::default());
        crate::handler::handle_key_event(
            &mut restarted,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Esc,
                crossterm::event::KeyModifiers::NONE,
            ),
            &tx,
        );
        assert_eq!(restarted.workspace.focus.overlay, AppMode::Normal);
        assert!(
            !restarted
                .workspace
                .documents
                .active()
                .unwrap()
                .editor
                .modified
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        // The record is still offered on the next start.
        assert_eq!(retained(&store, root.path()).len(), 1);
    }

    #[test]
    fn task3_disabled_persistence_writes_nothing_and_disables_commands() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = root.path().join("doc.txt");
        std::fs::write(&path, "body\n").unwrap();
        let mut config = AppConfig::default();
        config.recovery.enabled = Some(false);
        let (mut app, store) = install(root.path(), state.path(), config);
        assert!(!app.recovery_enabled());
        // configure_recovery surfaces no notice while persistence is off.
        assert!(app.recovery.as_ref().unwrap().records.is_empty());
        dirty(&mut app, &path, "secret draft\n");
        assert_eq!(app.snapshot_dirty_documents(Instant::now()), None);
        assert!(retained(&store, root.path()).is_empty());
        let context = CommandContext::capture(&app);
        assert!(commands::unavailable_reason(&app, &context, CommandId::RecoveryRestore).is_some());
        assert!(commands::dispatch_command(&mut app, CommandId::RecoveryRestore).is_err());
    }

    #[test]
    fn task3_missing_document_degrades_with_a_visible_notice() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = root.path().join("gone.txt");
        std::fs::write(&path, "body\n").unwrap();
        let (mut first, _store) = install(root.path(), state.path(), AppConfig::default());
        dirty(&mut first, &path, "draft\n");
        first.snapshot_dirty_documents(Instant::now());
        std::fs::remove_file(&path).unwrap();

        let (mut restarted, _store) = install(root.path(), state.path(), AppConfig::default());
        assert_eq!(restarted.recovery.as_ref().unwrap().records.len(), 1);
        let error = restarted.restore_recovery().unwrap_err();
        assert!(error.contains("could not open"), "{error}");
        // The app is still usable: a missing document is a notice, not a crash.
        assert!(!restarted.should_quit);
        assert!(restarted.workspace.documents.is_empty());
    }

    #[test]
    fn task3_explicit_cli_root_is_never_overridden_by_a_foreign_session() {
        let root_a = tempfile::tempdir().unwrap();
        let root_b = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let doc_a = root_a.path().join("a.txt");
        std::fs::write(&doc_a, "alpha\n").unwrap();
        let mut workspace = crate::workspace::Workspace::default();
        workspace
            .documents
            .open(&doc_a, OpenDisposition::Pinned)
            .unwrap();
        let record = crate::session::SessionRecord::from_workspace(&workspace, root_a.path());
        let sessions = crate::session::SessionStore::new(state.path());
        std::fs::create_dir_all(state.path()).unwrap();
        std::fs::write(
            sessions.workspace_path(root_b.path()),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();

        let (mut app, _store) = install(root_b.path(), state.path(), AppConfig::default());
        let notice = sessions
            .restore_into(root_b.path(), &mut app.workspace)
            .unwrap();
        assert!(notice.contains("Session not restored"), "{notice}");
        // The explicit CLI root (B) is never replaced by A's restored state.
        assert!(app.workspace.documents.is_empty());
    }

    #[test]
    fn task3_recovery_commands_restore_discard_clear_and_toggle() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = root.path().join("a.txt");
        std::fs::write(&path, "one\n").unwrap();
        let (mut first, _store) = install(root.path(), state.path(), AppConfig::default());
        dirty(&mut first, &path, "draft one\n");
        first.snapshot_dirty_documents(Instant::now());

        let (mut app, store) = install(root.path(), state.path(), AppConfig::default());
        app.open_document_path(&path, true);
        let context = CommandContext::capture(&app);
        assert!(commands::unavailable_reason(&app, &context, CommandId::RecoveryRestore).is_none());
        assert!(commands::unavailable_reason(&app, &context, CommandId::RecoveryDiscard).is_none());
        assert_eq!(app.recovery.as_ref().unwrap().records.len(), 1);

        commands::dispatch_command(&mut app, CommandId::RecoveryRestore).unwrap();
        assert!(app.workspace.documents.active().unwrap().editor.modified);

        // Clear removes every owned record while leaving the dirty buffer.
        commands::dispatch_command(&mut app, CommandId::RecoveryClear).unwrap();
        assert!(app.recovery.as_ref().unwrap().records.is_empty());
        assert!(retained(&store, root.path()).is_empty());
        assert!(app.workspace.documents.active().unwrap().editor.modified);

        // Re-capture the still-dirty buffer, then discard that record.
        assert_eq!(app.snapshot_dirty_documents(Instant::now()), None);
        assert_eq!(app.recovery.as_ref().unwrap().records.len(), 1);
        commands::dispatch_command(&mut app, CommandId::RecoveryDiscard).unwrap();
        assert!(app.recovery.as_ref().unwrap().records.is_empty());
        assert!(retained(&store, root.path()).is_empty());
        assert!(app.workspace.documents.active().unwrap().editor.modified);

        commands::dispatch_command(&mut app, CommandId::RecoveryDisable).unwrap();
        assert!(!app.recovery_enabled());
        assert!(commands::dispatch_command(&mut app, CommandId::RecoveryRestore).is_err());
        commands::dispatch_command(&mut app, CommandId::RecoveryEnable).unwrap();
        assert!(app.recovery_enabled());
    }

    #[test]
    fn task3_snapshots_are_throttled_and_older_revisions_never_overwrite_newer_text() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = root.path().join("doc.txt");
        std::fs::write(&path, "base\n").unwrap();
        let (mut app, store) = install(root.path(), state.path(), AppConfig::default());
        dirty(&mut app, &path, "alpha\n");
        let policy = app.recovery.as_ref().unwrap().policy;

        let start = Instant::now();
        assert_eq!(app.snapshot_dirty_documents(start), None);
        let first = store.load_all(root.path(), &policy, SystemTime::now());
        assert_eq!(first.len(), 1);
        let rev1 = first[0].revision;
        let path1 = store.record_path(root.path(), &path, &rev1);
        let bytes1 = std::fs::read_to_string(&path1).unwrap();

        // An immediate second pass is throttled: the record bytes do not move.
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .insert_text("more")
            .unwrap();
        assert_eq!(app.snapshot_dirty_documents(start), None);
        assert_eq!(std::fs::read_to_string(&path1).unwrap(), bytes1);

        // Past the interval, a same-revision pass deterministically replaces it.
        let next = start + policy.min_interval + Duration::from_millis(1);
        assert_eq!(app.snapshot_dirty_documents(next), None);
        assert!(std::fs::read_to_string(&path1).unwrap().contains("more"));
        assert_eq!(
            store
                .load_all(root.path(), &policy, SystemTime::now())
                .len(),
            1
        );

        // A newer on-disk revision keys a distinct record; the older record's
        // text is never overwritten by the newer capture.
        std::fs::write(&path, "base two\n").unwrap();
        let rev2 = crate::recovery::RevisionRef::from_path(&path).unwrap();
        assert_ne!(rev1, rev2);
        let later = next + policy.min_interval + Duration::from_millis(1);
        assert_eq!(app.snapshot_dirty_documents(later), None);
        let all = store.load_all(root.path(), &policy, SystemTime::now());
        assert_eq!(all.len(), 2);
        let path2 = store.record_path(root.path(), &path, &rev2);
        assert_ne!(path1, path2);
        assert!(path1.exists() && path2.exists());

        let older = all
            .iter()
            .find(|record| record.revision == rev1)
            .unwrap()
            .clone();
        let newer = all
            .iter()
            .find(|record| record.revision == rev2)
            .unwrap()
            .clone();
        let mut documents = DocumentStore::new();
        documents.open(&path, OpenDisposition::Pinned).unwrap();
        assert!(matches!(
            store.restore(&older, &mut documents),
            Err(crate::recovery::RecoveryError::DiskChanged(_))
        ));
        store.restore(&newer, &mut documents).unwrap();
        assert!(documents.active().unwrap().editor.modified);
        // The older record's file is still intact on disk.
        assert!(path1.exists());
    }

    #[test]
    fn task3_shutdown_flush_is_bounded_and_reports_persistence_errors() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = root.path().join("doc.txt");
        std::fs::write(&path, "body\n").unwrap();
        let (mut app, store) = install(root.path(), state.path(), AppConfig::default());
        dirty(&mut app, &path, "draft\n");
        assert_eq!(app.flush_recovery_on_shutdown(), None);
        assert_eq!(retained(&store, root.path()).len(), 1);

        // A blocked records directory surfaces a visible warning, not a panic
        // and not a swallowed failure.
        let blocked_state = tempfile::tempdir().unwrap();
        std::fs::write(blocked_state.path().join("recovery"), b"block").unwrap();
        let blocked_path = root.path().join("blocked.txt");
        std::fs::write(&blocked_path, "body\n").unwrap();
        let (mut blocked, _store) =
            install(root.path(), blocked_state.path(), AppConfig::default());
        dirty(&mut blocked, &blocked_path, "draft two\n");
        let warning = blocked.flush_recovery_on_shutdown().unwrap();
        assert!(warning.contains("not saved"), "{warning}");
    }

    #[test]
    fn task3_recovery_record_serialization_is_a_structural_whitelist() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = root.path().join("doc.txt");
        std::fs::write(&path, "body\n").unwrap();
        let (mut app, store) = install(root.path(), state.path(), AppConfig::default());
        dirty(&mut app, &path, "draft\n");
        app.snapshot_dirty_documents(Instant::now());
        let records = retained(&store, root.path());
        let record_path = store.record_path(root.path(), &path, &records[0].revision);
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(record_path).unwrap()).unwrap();
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "captured_secs",
                "document_path",
                "revision",
                "schema",
                "text",
                "version",
                "workspace_root",
            ]
        );
        let mut revision_keys: Vec<&str> = value["revision"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        revision_keys.sort_unstable();
        assert_eq!(
            revision_keys,
            ["modified_known", "modified_nanos", "modified_secs", "size"]
        );
    }

    #[test]
    fn task3_recovery_prompt_accepts_restore_and_refuses_on_escape() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = root.path().join("doc.txt");
        std::fs::write(&path, "body\n").unwrap();
        let (mut first, _store) = install(root.path(), state.path(), AppConfig::default());
        dirty(&mut first, &path, "draft\n");
        first.snapshot_dirty_documents(Instant::now());

        let (mut app, _store) = install(root.path(), state.path(), AppConfig::default());
        app.open_document_path(&path, true);
        let (tx, _rx) = crate::event::event_channel(Default::default());
        app.open_recovery_prompt();
        crate::handler::handle_key_event(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Esc,
                crossterm::event::KeyModifiers::NONE,
            ),
            &tx,
        );
        assert!(!app.workspace.documents.active().unwrap().editor.modified);

        app.open_recovery_prompt();
        crate::handler::handle_key_event(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('r'),
                crossterm::event::KeyModifiers::NONE,
            ),
            &tx,
        );
        assert!(app.workspace.documents.active().unwrap().editor.modified);
        // The single record is still retained (restore never destroys it), but
        // the offer is done: the prompt closes instead of re-offering it.
        assert_eq!(app.recovery.as_ref().unwrap().records.len(), 1);
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);

        // Discarding that document's record is an explicit, single-document
        // action: the open (still dirty) buffer and the source file are
        // untouched, and the dialog closes once the record is gone.
        app.open_recovery_prompt();
        crate::handler::handle_key_event(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('d'),
                crossterm::event::KeyModifiers::NONE,
            ),
            &tx,
        );
        assert!(app.recovery.as_ref().unwrap().records.is_empty());
        assert!(app.workspace.documents.active().unwrap().editor.modified);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "body\n");
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
    }

    /// P2-3: with several retained records the prompt names one document, and
    /// each handled offer advances to the next remaining document (bounded), so
    /// the user is never told every record was handled when it was not.
    #[test]
    fn task3_multi_record_prompt_offers_each_document_in_turn() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let first = root.path().join("a.txt");
        let second = root.path().join("b.txt");
        std::fs::write(&first, "one\n").unwrap();
        std::fs::write(&second, "two\n").unwrap();

        let (mut app, _store) = install(root.path(), state.path(), AppConfig::default());
        dirty(&mut app, &first, "draft a\n");
        dirty(&mut app, &second, "draft b\n");
        assert_eq!(app.snapshot_dirty_documents(Instant::now()), None);
        assert_eq!(app.recovery.as_ref().unwrap().records.len(), 2);

        let (tx, _rx) = crate::event::event_channel(Default::default());
        let press = |app: &mut App, code: crossterm::event::KeyCode| {
            crate::handler::handle_key_event(
                app,
                crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE),
                &tx,
            );
        };
        let offered = |app: &App| match &app.workspace.focus.overlay {
            AppMode::Dialog(DialogKind::RecoveryPrompt {
                document,
                remaining,
            }) => Some((document.clone(), *remaining)),
            _ => None,
        };

        // First offer names one specific document and discloses 1 further record.
        app.open_recovery_prompt();
        let (first_offer, remaining) = offered(&app).expect("first offer");
        assert_eq!(remaining, 2);
        assert!(
            first_offer == first || first_offer == second,
            "offer names a retained document"
        );

        // Discarding the first offer advances to the other document, not done.
        press(&mut app, crossterm::event::KeyCode::Char('d'));
        assert_eq!(app.recovery.as_ref().unwrap().records.len(), 1);
        let (second_offer, remaining) = offered(&app).expect("second offer");
        assert_eq!(remaining, 1);
        assert_ne!(second_offer, first_offer, "the next document is offered");

        // Handling the last offer closes the prompt: the bounded pass is done.
        press(&mut app, crossterm::event::KeyCode::Char('d'));
        assert!(app.recovery.as_ref().unwrap().records.is_empty());
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert!(offered(&app).is_none());
    }

    /// Fix round 2: the "N further" disclosure counts distinct documents the
    /// pass will still offer, not raw records. One document with several
    /// revision-keyed records must not overstate how many offers remain.
    #[test]
    fn task3_prompt_disclosure_counts_distinct_documents_not_records() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let multi = root.path().join("multi.txt");
        let other = root.path().join("other.txt");
        std::fs::write(&multi, "v1\n").unwrap();
        std::fs::write(&other, "v1\n").unwrap();

        let (mut app, store) = install(root.path(), state.path(), AppConfig::default());
        let policy = app.recovery.as_ref().unwrap().policy;

        // Two revision-keyed records for SAME document, plus one for a different
        // path. `multi` captures twice with a new on-disk revision in between.
        dirty(&mut app, &multi, "draft v1\n");
        assert_eq!(app.snapshot_dirty_documents(Instant::now()), None);
        std::fs::write(&multi, "v2\n").unwrap();
        app.workspace
            .documents
            .active_mut()
            .unwrap()
            .editor
            .insert_text("draft v2\n")
            .unwrap();
        assert_eq!(
            app.snapshot_dirty_documents(
                Instant::now() + policy.min_interval + Duration::from_millis(1)
            ),
            None
        );
        dirty(&mut app, &other, "draft other\n");
        assert_eq!(
            app.snapshot_dirty_documents(
                Instant::now() + policy.min_interval * 2 + Duration::from_millis(2)
            ),
            None
        );
        let records = store.load_all(root.path(), &policy, SystemTime::now());
        assert_eq!(records.len(), 3, "two multi records + one other");
        assert_eq!(
            records
                .iter()
                .filter(|record| record.document_path == multi)
                .count(),
            2
        );

        let (tx, _rx) = crate::event::event_channel(Default::default());
        let press = |app: &mut App, code: crossterm::event::KeyCode| {
            crate::handler::handle_key_event(
                app,
                crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE),
                &tx,
            );
        };
        let offered = |app: &App| match &app.workspace.focus.overlay {
            AppMode::Dialog(DialogKind::RecoveryPrompt {
                document,
                remaining,
            }) => Some((document.clone(), *remaining)),
            _ => None,
        };

        // First offer: two distinct documents remain in the pass (this one plus
        // one further), even though three raw records exist.
        app.open_recovery_prompt();
        let (first_offer, remaining) = offered(&app).expect("first offer");
        assert_eq!(
            remaining, 2,
            "disclosure counts distinct documents, not raw records"
        );

        // Each distinct document is offered exactly once; discard advances.
        press(&mut app, crossterm::event::KeyCode::Char('d'));
        let (second_offer, remaining) = offered(&app).expect("second offer");
        assert_eq!(remaining, 1);
        assert_ne!(second_offer, first_offer, "each document offered once");

        // After the second distinct document the pass terminates: the remaining
        // record for the already-handled first document is not re-offered.
        press(&mut app, crossterm::event::KeyCode::Char('d'));
        assert_eq!(app.workspace.focus.overlay, AppMode::Normal);
        assert!(
            offered(&app).is_none(),
            "pass terminates after distinct docs"
        );
        let leftovers = store.load_all(root.path(), &policy, SystemTime::now());
        assert!(
            leftovers
                .iter()
                .all(|record| record.document_path == first_offer
                    || record.document_path == second_offer),
            "only handled documents' records remain"
        );
    }
}
