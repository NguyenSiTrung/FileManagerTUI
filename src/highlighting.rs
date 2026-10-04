//! Prepared-only preview loading and bounded line/checkpoint syntax caches.
//!
//! ## Why this module exists (Phase 6 Task 2)
//!
//! Before this slice, `preview_content::load_*` and the editor widget's
//! `HighlightLines` reconstruction ran from the render/input path: every frame
//! read the selected file (or re-parsed the whole editor prefix) before a single
//! cell was painted. That work is now expressed as *prepared state*:
//!
//! * File previews are classified and highlighted by a worker job and delivered
//!   as immutable [`PreparedPreview`] values carrying a [`PreviewKey`] identity.
//!   Rendering consumes the prepared lines verbatim.
//! * Editor highlighting is kept in a per-document [`SyntaxCache`] at
//!   line/checkpoint granularity. Edits and theme changes invalidate only the
//!   affected suffix and the parser resumes from the nearest checkpoint, not
//!   from line zero.
//! * [`render_io_count`] is a thread-local probe: any synchronous prepared-state
//!   read performed on the render thread increments it. Prepared-only rendering
//!   therefore observes `render_io_count == 0`.

use std::path::PathBuf;
use std::sync::OnceLock;

use ratatui::text::Line;
use syntect::highlighting::{
    HighlightIterator, HighlightState, Highlighter, Style as SynStyle, Theme, ThemeSet,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

use crate::theme::ThemeColors;

// ── Scheduled syntax assets ─────────────────────────────────────────────────

/// Process-wide syntax set. Loading it reads bundled data, never user files;
/// caching avoids rebuilding it for every background job.
pub fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Built-in theme by name with a deterministic fallback, cached per name.
pub fn builtin_theme(name: &str) -> Theme {
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();
    let set = THEMES.get_or_init(ThemeSet::load_defaults);
    set.themes
        .get(name)
        .or_else(|| set.themes.get("base16-ocean.dark"))
        .cloned()
        .unwrap_or_default()
}

// ── Render-thread I/O probe ─────────────────────────────────────────────────

thread_local! {
    static RENDER_IO: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static RENDER_IO_WATCH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Record one synchronous prepared-state read (file read, stat, walk or
/// highlight) on the current thread.
///
/// The probe is wired into the real preview loaders (`preview_content::*`) and
/// syntax helpers, so any code path that reads or highlights while the watch is
/// armed is counted, not just the fallback. Worker threads keep independent
/// counters and never arm the watch, so background work stays invisible here.
pub fn note_render_io() {
    RENDER_IO.with(|count| count.set(count.get().saturating_add(1)));
}

/// Arm the current thread's probe so reads/highlights are counted. Render-path
/// tests arm it; production never arms it, so the cost is one thread-local read.
#[cfg(test)]
pub fn watch_render_io() {
    RENDER_IO_WATCH.with(|armed| armed.set(true));
}

/// Disarm the current thread's probe.
#[cfg(test)]
pub fn unwatch_render_io() {
    RENDER_IO_WATCH.with(|armed| armed.set(false));
}

/// Record a read/highlight iff this thread is watching. Called by the loaders.
pub fn note_render_io_if_watching() {
    if RENDER_IO_WATCH.with(std::cell::Cell::get) {
        note_render_io();
    }
}

/// Synchronous prepared-state reads observed on this thread since the last
/// [`reset_render_io`]. Prepared-only rendering must report zero.
#[cfg(test)]
pub fn render_io_count() -> usize {
    RENDER_IO.with(std::cell::Cell::get)
}

/// Reset the counter and arm the watch, so the next operations are measured.
///
/// Callers must disarm with [`unwatch_render_io`] (tests that may panic should
/// use [`RenderIoGuard`]).
#[cfg(test)]
pub fn reset_render_io() {
    RENDER_IO.with(|count| count.set(0));
    watch_render_io();
}

/// Reset the counter without touching the watch state.
#[cfg(test)]
pub fn reset_render_io_counter() {
    RENDER_IO.with(|count| count.set(0));
}

/// Scoped watch: counts reads while alive and disarms on drop, including panics.
#[cfg(test)]
pub struct RenderIoGuard;

#[cfg(test)]
impl RenderIoGuard {
    /// Reset the counter and arm the watch for this thread.
    pub fn arm() -> Self {
        reset_render_io();
        Self
    }
}

#[cfg(test)]
impl Drop for RenderIoGuard {
    fn drop(&mut self) {
        unwatch_render_io();
    }
}

// ── Versioned preview identity ──────────────────────────────────────────────

/// Requested large-file presentation, mirroring `app::ViewMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestedView {
    Full,
    HeadAndTail,
    HeadOnly,
    TailOnly,
}

impl RequestedView {
    /// Map the configured `preview_view_mode()` string.
    pub fn from_config(value: &str) -> Self {
        match value {
            "head_only" => Self::HeadOnly,
            "tail_only" => Self::TailOnly,
            "head_and_tail" => Self::HeadAndTail,
            _ => Self::Full,
        }
    }

    /// Whether this request streams a bounded head/tail window.
    pub fn is_large(self) -> bool {
        !matches!(self, Self::Full)
    }
}

/// Immutable identity of one prepared preview request. Any field change produces
/// a different identity, so a delayed older result can never be installed over
/// newer state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewKey {
    pub path: PathBuf,
    pub revision: u64,
    pub theme: u64,
    pub view: RequestedView,
    pub head_lines: usize,
    pub tail_lines: usize,
    pub max_full_bytes: u64,
}

/// Classification chosen by the worker (which owns the required file reads).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewClass {
    Full,
    Large,
    Binary,
    Notebook,
    Error,
}

/// Immutable prepared preview lines plus the identity that produced them.
pub struct PreparedPreview {
    pub key: PreviewKey,
    pub lines: Vec<Line<'static>>,
    pub total_lines: usize,
    pub class: PreviewClass,
}

impl PreparedPreview {
    /// Effective large-file flag for `PreviewState`.
    #[allow(dead_code)]
    pub fn is_large(&self) -> bool {
        matches!(self.class, PreviewClass::Large)
    }
}

impl crate::background::Payload for PreparedPreview {
    fn payload_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.key.path.capacity())
            .saturating_add(crate::preview_content::summary_lines_bytes(&self.lines))
    }
}

/// Classify and highlight a file for the preview panel. Runs on a worker only.
///
/// The `max_full_bytes` / `view` inputs mirror `AppConfig`; the decision to read
/// in full, stream head+tail, show binary metadata or parse a notebook happens
/// here so the render path never stat or scans the file.
pub fn load_file_preview(
    key: &PreviewKey,
    syntax: &SyntaxSet,
    theme_name: &str,
    colors: &ThemeColors,
) -> PreparedPreview {
    let theme = builtin_theme(theme_name);
    let path = key.path.as_path();

    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return error_preview(key, format!("Error reading file: {error}"), colors);
        }
    };

    let is_notebook = path.extension().and_then(|e| e.to_str()) == Some("ipynb");
    if is_notebook {
        // The notebook path has no head/tail fallback, so it takes the preview
        // module's explicit hard-cap budget: an oversize notebook degrades to
        // an honest message instead of reading the whole file.
        let (lines, total) = crate::preview_content::load_notebook_content(
            path,
            syntax,
            &theme,
            colors,
            crate::preview_content::MAX_PREVIEW_SIZE,
        );
        return PreparedPreview {
            key: key.clone(),
            lines,
            total_lines: total,
            class: PreviewClass::Notebook,
        };
    }

    if crate::preview_content::is_binary_file(path) {
        let (lines, total) = crate::preview_content::load_binary_metadata(path, colors);
        return PreparedPreview {
            key: key.clone(),
            lines,
            total_lines: total,
            class: PreviewClass::Binary,
        };
    }

    let is_large = metadata.len() > key.max_full_bytes || key.view.is_large();
    if is_large {
        let view = match key.view {
            RequestedView::HeadOnly => crate::app::ViewMode::HeadOnly,
            RequestedView::TailOnly => crate::app::ViewMode::TailOnly,
            RequestedView::Full | RequestedView::HeadAndTail => crate::app::ViewMode::HeadAndTail,
        };
        let (lines, total) = crate::preview_content::load_head_tail_content(
            path,
            syntax,
            &theme,
            colors,
            key.head_lines,
            key.tail_lines,
            view,
        );
        return PreparedPreview {
            key: key.clone(),
            lines,
            total_lines: total,
            class: PreviewClass::Large,
        };
    }

    let (lines, total) =
        crate::preview_content::load_highlighted_content(path, syntax, &theme, colors);
    PreparedPreview {
        key: key.clone(),
        lines,
        total_lines: total,
        class: PreviewClass::Full,
    }
}

/// Bounded head/tail fallback for a preview whose full result cannot fit the
/// result envelope. Shrinks the requested window until it fits `budget`, and
/// finally truncates, so a raw `PayloadTooLarge` is never surfaced.
pub fn budgeted_fallback_preview(
    key: &PreviewKey,
    syntax: &SyntaxSet,
    theme_name: &str,
    colors: &ThemeColors,
    budget: usize,
) -> PreparedPreview {
    let theme = builtin_theme(theme_name);
    let view = crate::app::ViewMode::HeadAndTail;
    let mut head = key.head_lines;
    let mut tail = key.tail_lines;
    let mut lines = Vec::new();
    let mut total = 0usize;
    for _ in 0..16 {
        let (loaded, total_lines) = crate::preview_content::load_head_tail_content(
            key.path.as_path(),
            syntax,
            &theme,
            colors,
            head,
            tail,
            view,
        );
        lines = rebuild(loaded);
        total = total_lines;
        if fits(key, &lines, total, budget) {
            return PreparedPreview {
                key: key.clone(),
                lines,
                total_lines: total,
                class: PreviewClass::Large,
            };
        }
        if head <= 1 && tail <= 1 {
            break;
        }
        head = (head / 2).max(1);
        tail = (tail / 2).max(1);
    }
    // Last resort: keep only the leading lines that fit the envelope.
    while !lines.is_empty() && !fits(key, &lines, total, budget) {
        lines.pop();
        lines = rebuild(lines);
    }
    PreparedPreview {
        key: key.clone(),
        lines,
        total_lines: total,
        class: PreviewClass::Large,
    }
}

/// Rebuild lines with exact allocations so `capacity()`-based payload
/// accounting matches the retained bytes.
fn rebuild(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let mut rebuilt: Vec<Line<'static>> = lines
        .into_iter()
        .map(|line| {
            let mut row = Line::from(
                line.spans
                    .into_iter()
                    .map(|span| ratatui::text::Span::styled(span.content.into_owned(), span.style))
                    .collect::<Vec<_>>(),
            );
            for span in &mut row.spans {
                span.content.to_mut().shrink_to_fit();
            }
            row.spans.shrink_to_fit();
            row
        })
        .collect();
    rebuilt.shrink_to_fit();
    rebuilt
}

fn fits(key: &PreviewKey, lines: &[Line<'static>], total: usize, budget: usize) -> bool {
    let candidate = PreparedPreview {
        key: key.clone(),
        lines: rebuild(lines.to_vec()),
        total_lines: total,
        class: PreviewClass::Large,
    };
    crate::background::Payload::payload_bytes(&candidate) <= budget
}

fn error_preview(key: &PreviewKey, message: String, colors: &ThemeColors) -> PreparedPreview {
    PreparedPreview {
        key: key.clone(),
        lines: vec![Line::from(ratatui::text::Span::styled(
            message,
            ratatui::style::Style::default().fg(colors.error_fg),
        ))],
        total_lines: 1,
        class: PreviewClass::Error,
    }
}

/// Stable signature of a syntax-theme name, used to version prepared state
/// without hashing the whole resolved theme.
pub fn theme_signature(name: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in name.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Cheap content revision derived from in-memory file metadata. Avoids a render
/// path `stat`: callers pass the tree's already-loaded size/modification time.
pub fn revision_from_meta(size: u64, modified: Option<std::time::SystemTime>) -> u64 {
    let mut revision = size.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    if let Some(modified) = modified {
        if let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) {
            revision ^= duration.as_nanos() as u64;
        }
    }
    revision
}

// ── Bounded line/checkpoint syntax cache ────────────────────────────────────

/// One styled byte range within a logical line (excluding its newline).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StyledRun {
    pub start: usize,
    pub end: usize,
    pub style: SynStyle,
}

/// Cached highlight runs for one logical line.
#[derive(Debug, Clone)]
pub struct CachedLine {
    pub runs: Vec<StyledRun>,
    bytes: usize,
}

impl CachedLine {
    fn retained_bytes(&self) -> usize {
        self.bytes
    }
}

/// Parser/highlighter state captured before a logical line. A recompute resumes
/// from the nearest checkpoint at or before the first invalidated line.
#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub line: usize,
    pub parse: ParseState,
    pub highlight: HighlightState,
}

/// Checkpoint spacing. Larger values save memory; smaller values shorten the
/// re-highlight range after an edit.
pub const CHECKPOINT_INTERVAL: usize = 32;

/// Reserved bytes charged per checkpoint against the document's cache bound.
pub const CHECKPOINT_STATE_BYTES: usize = 512;

/// Per-document syntax/highlight cache.
///
/// `lines[i] == None` means "not prepared": rendering must show an explicit
/// pending style instead of stale runs. `prepared` is the contiguous prepared
/// prefix length; `bytes` bounds total retained memory.
#[derive(Debug, Default)]
pub struct SyntaxCache {
    revision: u64,
    theme_epoch: u64,
    syntax_name: String,
    lines: Vec<Option<CachedLine>>,
    checkpoints: Vec<Checkpoint>,
    prepared: usize,
    bytes: usize,
    complete: bool,
    last_resume: usize,
}

#[allow(dead_code)]
impl SyntaxCache {
    /// Build a fresh cache over `lines`, resuming from a fresh parser state.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        lines: &[String],
        syntax: &SyntaxReference,
        syntax_name: &str,
        ss: &SyntaxSet,
        theme: &Theme,
        revision: u64,
        theme_epoch: u64,
        limit_bytes: usize,
    ) -> Self {
        let mut cache = Self {
            revision,
            theme_epoch,
            syntax_name: syntax_name.to_string(),
            lines: vec![None; lines.len()],
            checkpoints: Vec::new(),
            prepared: 0,
            bytes: 0,
            complete: false,
            last_resume: 0,
        };
        cache.recompute(lines, syntax, ss, theme, limit_bytes);
        cache
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn theme_epoch(&self) -> u64 {
        self.theme_epoch
    }
    #[allow(dead_code)]
    pub fn syntax_name(&self) -> &str {
        &self.syntax_name
    }
    /// Contiguous prepared prefix length (lines `0..prepared`).
    pub fn prepared(&self) -> usize {
        self.prepared
    }
    pub fn is_complete(&self) -> bool {
        self.complete
    }
    pub fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
    }
    pub fn set_theme_epoch(&mut self, theme_epoch: u64) {
        self.theme_epoch = theme_epoch;
    }
    pub fn set_syntax_name(&mut self, name: &str) {
        self.syntax_name = name.to_string();
    }
    pub fn ensure_len(&mut self, lines: usize) {
        if self.lines.len() < lines {
            self.lines.resize(lines, None);
        }
    }
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }
    /// Test-only: bytes retained by cached run data alone, recomputed from the
    /// stored lines so it cannot be derived from the charged total.
    #[cfg(test)]
    pub fn run_bytes(&self) -> usize {
        self.lines
            .iter()
            .flatten()
            .map(CachedLine::retained_bytes)
            .fold(0usize, usize::saturating_add)
    }
    #[allow(dead_code)]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
    /// Most recent recompute start line (checkpoint resume), for diagnostics.
    pub fn last_resume(&self) -> usize {
        self.last_resume
    }
    pub fn checkpoint_count(&self) -> usize {
        self.checkpoints.len()
    }

    /// Whether `line` has prepared runs. Unprepared lines must render pending.
    pub fn is_prepared(&self, line: usize) -> bool {
        self.lines.get(line).is_some_and(Option::is_some)
    }

    /// Prepared runs for `line`, if any.
    pub fn runs(&self, line: usize) -> Option<&[StyledRun]> {
        self.lines
            .get(line)
            .and_then(Option::as_ref)
            .map(|l| l.runs.as_slice())
    }

    /// Invalidate from `line` to the end without discarding earlier checkpoints.
    /// The parser will resume from the nearest kept checkpoint on recompute.
    pub fn invalidate_from(&mut self, line: usize) {
        let line = line.min(self.lines.len());
        for slot in &mut self.lines[line..] {
            if let Some(old) = slot.take() {
                self.bytes = self.bytes.saturating_sub(old.retained_bytes());
            }
        }
        let before = self.checkpoints.len();
        self.checkpoints
            .retain(|checkpoint| checkpoint.line <= line);
        let dropped = before.saturating_sub(self.checkpoints.len());
        if dropped > 0 {
            self.bytes = self
                .bytes
                .saturating_sub(dropped.saturating_mul(CHECKPOINT_STATE_BYTES));
        }
        self.prepared = self.prepared.min(line);
        self.complete = false;
    }

    /// Invalidate everything (theme change, document replacement).
    pub fn invalidate_all(&mut self) {
        self.invalidate_from(0);
        self.checkpoints.clear();
        self.prepared = 0;
        self.bytes = 0;
        for slot in &mut self.lines {
            *slot = None;
        }
    }

    /// Clone the checkpoint exactly at `line`, if present.
    pub fn checkpoint_at(&self, line: usize) -> Option<Checkpoint> {
        self.checkpoints
            .iter()
            .find(|checkpoint| checkpoint.line == line)
            .cloned()
    }

    /// Resume line to re-highlight from: the nearest checkpoint at or before the
    /// first unprepared line. `None` means start from a fresh parser state.
    pub fn resume_line(&self) -> usize {
        self.checkpoints
            .iter()
            .rev()
            .find(|checkpoint| checkpoint.line <= self.prepared)
            .map(|checkpoint| checkpoint.line)
            .unwrap_or(0)
    }

    /// Highlight `lines` incrementally, resuming from the nearest checkpoint at
    /// or before the first unprepared line. Returns the resume line used.
    pub fn recompute(
        &mut self,
        lines: &[String],
        syntax: &SyntaxReference,
        ss: &SyntaxSet,
        theme: &Theme,
        limit_bytes: usize,
    ) -> usize {
        self.prepare(
            lines,
            syntax,
            ss,
            theme,
            usize::MAX,
            lines.len(),
            limit_bytes,
        )
    }

    /// Bounded incremental step: highlight at most `max_lines` *new* lines and
    /// stop at `cap`. Returns the resume line used. `complete` is only set when
    /// the whole `min(lines.len(), cap)` prefix is prepared within budget.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        lines: &[String],
        syntax: &SyntaxReference,
        ss: &SyntaxSet,
        theme: &Theme,
        max_lines: usize,
        cap: usize,
        limit_bytes: usize,
    ) -> usize {
        if self.lines.len() != lines.len() {
            self.lines.resize(lines.len(), None);
        }
        // Choose the resume checkpoint. Never restart from line zero when a
        // valid checkpoint exists at or before the first unprepared line.
        let resume = self.resume_line();
        self.last_resume = resume;
        self.checkpoints
            .retain(|checkpoint| checkpoint.line <= resume);

        let highlighter = Highlighter::new(theme);
        let (mut parse, mut highlight) = match self.checkpoint_at(resume) {
            Some(checkpoint) => (checkpoint.parse, checkpoint.highlight),
            None => (
                ParseState::new(syntax),
                HighlightState::new(&highlighter, ScopeStack::new()),
            ),
        };
        // Charged checkpoint bytes stay part of the document bound.
        let mut checkpoint_bytes = self
            .checkpoints
            .len()
            .saturating_mul(CHECKPOINT_STATE_BYTES);
        // A checkpoint exactly at `resume` is the state *before* that line.
        if self
            .checkpoints
            .iter()
            .all(|checkpoint| checkpoint.line != resume)
        {
            self.checkpoints.push(Checkpoint {
                line: resume,
                parse: parse.clone(),
                highlight: highlight.clone(),
            });
            checkpoint_bytes = checkpoint_bytes.saturating_add(CHECKPOINT_STATE_BYTES);
        }

        let already = self.prepared.max(resume);
        let end = lines.len().min(cap);
        let mut bytes = self
            .lines
            .iter()
            .flatten()
            .map(CachedLine::retained_bytes)
            .fold(0usize, usize::saturating_add);
        let mut prepared = 0usize;
        let mut new_lines = 0usize;
        let mut complete = true;
        for (index, text) in lines.iter().enumerate().take(end) {
            if index < resume {
                prepared = index + 1;
                continue;
            }
            let redoing = index < already;
            if !redoing && new_lines >= max_lines {
                complete = false;
                break;
            }
            if bytes.saturating_add(checkpoint_bytes) >= limit_bytes {
                complete = false;
                break;
            }
            let runs = highlight_runs(&mut parse, &mut highlight, &highlighter, ss, text);
            let line_bytes = std::mem::size_of::<CachedLine>()
                + runs.capacity() * std::mem::size_of::<StyledRun>();
            let cached = CachedLine {
                runs,
                bytes: line_bytes,
            };
            bytes = bytes.saturating_add(cached.retained_bytes());
            self.lines[index] = Some(cached);
            prepared = index + 1;
            if !redoing {
                new_lines += 1;
            }
            let next = index + 1;
            if next < end && next % CHECKPOINT_INTERVAL == 0 {
                self.checkpoints.push(Checkpoint {
                    line: next,
                    parse: parse.clone(),
                    highlight: highlight.clone(),
                });
                checkpoint_bytes = checkpoint_bytes.saturating_add(CHECKPOINT_STATE_BYTES);
            }
        }
        self.bytes = bytes.saturating_add(checkpoint_bytes);
        self.prepared = prepared.max(self.prepared.min(resume));
        self.complete = complete && self.prepared >= lines.len();
        resume
    }
}

/// Highlight one logical line (newline-aware) into styled byte ranges.
fn highlight_runs(
    parse: &mut ParseState,
    highlight: &mut HighlightState,
    highlighter: &Highlighter<'_>,
    ss: &SyntaxSet,
    line: &str,
) -> Vec<StyledRun> {
    let mut with_newline = String::with_capacity(line.len() + 1);
    with_newline.push_str(line);
    with_newline.push('\n');
    let ops = parse.parse_line(&with_newline, ss).unwrap_or_default();
    let mut runs = Vec::new();
    let mut byte = 0usize;
    for (style, text) in HighlightIterator::new(highlight, &ops, &with_newline, highlighter) {
        let start = byte;
        byte += text.len();
        let end = byte.min(line.len());
        if start < end {
            runs.push(StyledRun { start, end, style });
        }
    }
    runs
}

/// Window length prepared per syntax job (bounded job payload).
pub const SYNTAX_WINDOW_LINES: usize = 256;
/// Maximum prepared prefix for one editor document; remaining lines stay in an
/// explicit pending state rather than growing unbounded.
pub const SYNTAX_PREPARE_CAP_LINES: usize = 20_000;
/// Per-document prepared highlight byte budget (includes charged checkpoint
/// state, not just cached run bytes).
pub const SYNTAX_CACHE_BYTES: usize = 4 * 1024 * 1024;
/// Aggregate prepared-highlight byte budget across all open documents. Caches
/// beyond this are evicted least-recently-used (they re-prepare on demand).
pub const SYNTAX_TOTAL_CACHE_BYTES: usize = 32 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    fn colors() -> ThemeColors {
        crate::theme::dark_theme()
    }

    fn rust_syntax() -> &'static SyntaxReference {
        syntax_set().find_syntax_by_extension("rs").unwrap()
    }

    #[test]
    fn render_probe_counts_only_this_thread() {
        let _guard = RenderIoGuard::arm();
        assert_eq!(render_io_count(), 0);
        note_render_io();
        assert_eq!(render_io_count(), 1);
    }

    #[test]
    fn render_probe_detects_a_genuine_loader_read_on_the_watched_thread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("probe.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let theme = builtin_theme("base16-ocean.dark");
        let colors = colors();

        // Unwatched: a loader read is invisible (worker threads behave this way).
        unwatch_render_io();
        reset_render_io_counter();
        let _ =
            crate::preview_content::load_highlighted_content(&path, syntax_set(), &theme, &colors);
        assert_eq!(
            render_io_count(),
            0,
            "unwatched threads must not count loader reads"
        );

        // Watched: the same real loader read is detected.
        let _guard = RenderIoGuard::arm();
        let _ =
            crate::preview_content::load_highlighted_content(&path, syntax_set(), &theme, &colors);
        assert_eq!(
            render_io_count(),
            1,
            "the probe must detect a real render-time file read"
        );

        // A watched head/tail load counts as another genuine read path.
        let _ = crate::preview_content::load_head_tail_content(
            &path,
            syntax_set(),
            &theme,
            &colors,
            5,
            5,
            crate::app::ViewMode::HeadAndTail,
        );
        assert_eq!(render_io_count(), 2);
    }

    #[test]
    fn preview_key_identity_changes_with_revision_theme_and_view() {
        let base = PreviewKey {
            path: PathBuf::from("a.rs"),
            revision: 1,
            theme: 1,
            view: RequestedView::Full,
            head_lines: 100,
            tail_lines: 100,
            max_full_bytes: 5 * 1024 * 1024,
        };
        let mut other = base.clone();
        other.revision = 2;
        assert_ne!(base, other);
        let mut themed = base.clone();
        themed.theme = 2;
        assert_ne!(base, themed);
        let mut viewed = base.clone();
        viewed.view = RequestedView::HeadOnly;
        assert_ne!(base, viewed);
        let mut larger = base.clone();
        larger.max_full_bytes = 1;
        assert_ne!(base, larger);
    }

    #[test]
    fn revision_from_meta_distinguishes_size_and_time() {
        assert_eq!(revision_from_meta(10, None), revision_from_meta(10, None));
        assert_ne!(revision_from_meta(10, None), revision_from_meta(11, None));
    }

    #[test]
    fn syntax_cache_newline_aware_state_survives_incremental_recompute() {
        let before = vec!["a = 1".to_string(), "b = 2".to_string()];
        let after = vec![
            "a = 1".to_string(),
            "\"\"\"unterminated".to_string(),
            "still in string".to_string(),
            "\"\"\"".to_string(),
            "c = 3".to_string(),
        ];
        let syntax = syntax_set().find_syntax_by_extension("py").unwrap();
        let theme = builtin_theme("base16-ocean.dark");
        let mut incremental = SyntaxCache::build(
            &before,
            syntax,
            "Python",
            syntax_set(),
            &theme,
            1,
            1,
            usize::MAX,
        );
        // The edit reopens the string literal; only the affected suffix is
        // invalidated, then recomputed from the checkpoint.
        incremental.invalidate_from(1);
        incremental.recompute(&after, syntax, syntax_set(), &theme, usize::MAX);
        let fresh = SyntaxCache::build(
            &after,
            syntax,
            "Python",
            syntax_set(),
            &theme,
            1,
            1,
            usize::MAX,
        );
        assert_eq!(incremental.prepared(), after.len());
        for line in 0..after.len() {
            assert_eq!(
                incremental.runs(line),
                fresh.runs(line),
                "recomputed line {line} diverged from a fresh full highlight"
            );
        }
    }

    #[test]
    fn syntax_cache_invalidation_resumes_from_checkpoint_not_line_zero() {
        let lines: Vec<String> = (0..200).map(|i| format!("let x{i} = {i};")).collect();
        let theme = builtin_theme("base16-ocean.dark");
        let mut cache = SyntaxCache::build(
            &lines,
            rust_syntax(),
            "Rust",
            syntax_set(),
            &theme,
            1,
            1,
            usize::MAX,
        );
        assert!(cache.is_prepared(199));
        assert!(cache.checkpoint_count() >= 6);
        cache.invalidate_from(130);
        assert!(!cache.is_prepared(130));
        assert!(cache.is_prepared(129));
        let resume = cache.recompute(&lines, rust_syntax(), syntax_set(), &theme, usize::MAX);
        assert_eq!(resume, 128, "must resume at the nearest checkpoint <= 130");
        assert!(resume > 0, "must not restart from line zero");
        assert_eq!(cache.last_resume(), 128);
        assert!(cache.is_prepared(199));
    }

    #[test]
    fn syntax_cache_memory_is_bounded_and_reports_pending() {
        let lines: Vec<String> = (0..5000)
            .map(|i| format!("let identifier_{i} = \"value {i}\";"))
            .collect();
        let theme = builtin_theme("base16-ocean.dark");
        let limit = 8 * 1024;
        let cache = SyntaxCache::build(
            &lines,
            rust_syntax(),
            "Rust",
            syntax_set(),
            &theme,
            1,
            1,
            limit,
        );
        assert!(cache.retained_bytes() <= limit + 512);
        assert!(cache.prepared() < lines.len());
        assert!(!cache.is_complete());
        assert!(!cache.is_prepared(cache.prepared()));
    }

    #[test]
    fn syntax_cache_invalidate_all_clears_prefix_and_checkpoints() {
        let lines: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
        let theme = builtin_theme("base16-ocean.dark");
        let mut cache = SyntaxCache::build(
            &lines,
            syntax_set().find_syntax_plain_text(),
            "Plain Text",
            syntax_set(),
            &theme,
            1,
            1,
            usize::MAX,
        );
        assert!(cache.is_complete());
        cache.invalidate_all();
        assert_eq!(cache.prepared(), 0);
        assert_eq!(cache.checkpoint_count(), 0);
        assert!(!cache.is_prepared(0));
        assert_eq!(cache.retained_bytes(), 0);
    }

    #[test]
    fn syntax_cache_checkpoint_bytes_count_toward_the_document_bound() {
        // Wide window, small budget: the per-document bound must include
        // checkpoint state, not just cached run bytes.
        let lines: Vec<String> = (0..1000)
            .map(|i| format!("let identifier_{i} = \"value {i}\";"))
            .collect();
        let theme = builtin_theme("base16-ocean.dark");
        let cache = SyntaxCache::build(
            &lines,
            rust_syntax(),
            "Rust",
            syntax_set(),
            &theme,
            1,
            1,
            usize::MAX,
        );
        let expected: usize = cache.checkpoint_count() * CHECKPOINT_STATE_BYTES;
        assert!(expected > 0, "checkpoints exist");
        assert!(
            cache.retained_bytes() >= expected,
            "retained bytes {} must include {} checkpoint bytes",
            cache.retained_bytes(),
            expected
        );
        // Exact identity: the charged total must be run bytes plus the
        // checkpoint charge. This fails if the charge stops being added.
        assert_eq!(
            cache.retained_bytes(),
            cache.run_bytes() + cache.checkpoint_count() * CHECKPOINT_STATE_BYTES,
            "retained bytes must equal run bytes plus checkpoint state"
        );
        // A tight budget stops before the run-byte-only total would have.
        let tight = SyntaxCache::build(
            &lines,
            rust_syntax(),
            "Rust",
            syntax_set(),
            &theme,
            1,
            1,
            4096,
        );
        assert!(
            tight.retained_bytes() <= 4096 + CHECKPOINT_STATE_BYTES,
            "checkpoint bytes must respect the per-document bound (plus one \
             checkpoint-granularity overshoot)"
        );
        assert!(
            tight.checkpoint_count() * CHECKPOINT_STATE_BYTES <= tight.retained_bytes(),
            "checkpoints are charged against the bound"
        );
        // Tight-budget caches obey the same exact identity, so dropping the
        // charge cannot leave this test green.
        assert_eq!(
            tight.retained_bytes(),
            tight.run_bytes() + tight.checkpoint_count() * CHECKPOINT_STATE_BYTES,
            "tight cache retained bytes must include the checkpoint state"
        );
        assert!(!tight.is_complete());
        assert!(tight.prepared() < lines.len());
    }

    #[test]
    fn prepared_preview_loads_full_binary_and_notebook() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("a.rs");
        std::fs::write(&text, "fn main() {}\n").unwrap();
        let mut key = PreviewKey {
            path: text.clone(),
            revision: 1,
            theme: 1,
            view: RequestedView::Full,
            head_lines: 100,
            tail_lines: 100,
            max_full_bytes: 5 * 1024 * 1024,
        };
        let prepared = load_file_preview(&key, syntax_set(), "base16-ocean.dark", &colors());
        assert_eq!(prepared.class, PreviewClass::Full);
        assert_eq!(prepared.total_lines, 1);
        assert!(!prepared.lines.is_empty());

        let binary = dir.path().join("model.pt");
        std::fs::write(&binary, [0u8; 64]).unwrap();
        key.path = binary;
        let prepared = load_file_preview(&key, syntax_set(), "base16-ocean.dark", &colors());
        assert_eq!(prepared.class, PreviewClass::Binary);

        let notebook = dir.path().join("test.ipynb");
        std::fs::write(
            &notebook,
            r#"{"cells":[{"cell_type":"code","source":["x=1"],"outputs":[]}],"metadata":{}}"#,
        )
        .unwrap();
        key.path = notebook;
        let prepared = load_file_preview(&key, syntax_set(), "base16-ocean.dark", &colors());
        assert_eq!(prepared.class, PreviewClass::Notebook);
        let text: String = prepared
            .lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(text.contains("Cell 1"));
    }

    #[test]
    fn prepared_preview_large_file_streams_head_and_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        let body = (1..=1000)
            .map(|i| format!("line {i}\n"))
            .collect::<String>();
        std::fs::write(&path, body).unwrap();
        let key = PreviewKey {
            path,
            revision: 1,
            theme: 1,
            view: RequestedView::Full,
            head_lines: 10,
            tail_lines: 5,
            max_full_bytes: 16,
        };
        let prepared = load_file_preview(&key, syntax_set(), "base16-ocean.dark", &colors());
        assert_eq!(prepared.class, PreviewClass::Large);
        assert_eq!(prepared.total_lines, 1000);
        assert_eq!(prepared.lines.len(), 16);
    }
}
