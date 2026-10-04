//! Native literal project content search and incremental filename indexing.
//!
//! Both operations are pure, bounded, resumable and cancellable: a request
//! carries a cursor describing exactly where the previous batch stopped, and a
//! batch returns the cursor for the next one. No external search executable is
//! required; every result comes from native Rust filesystem traversal.
//!
//! Bounds are explicit and reported, never silently applied: [`SearchCursor`]
//! distinguishes an exhausted traversal (`done`) from one stopped by a cap or
//! deadline (`capped`) and counts unreadable entries. Result order is
//! deterministic (FIFO directory order), so stale batches are trivially
//! distinguishable from current ones by their immutable request generation.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Directory names excluded by default. These are directory components, never
/// substrings or globs, so a file literally named `target` remains searchable.
pub const DEFAULT_EXCLUDES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    ".idea",
    ".gradle",
    "dist",
    "build",
];

pub const DEFAULT_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
pub const DEFAULT_MAX_FILES: usize = 20_000;
pub const DEFAULT_MAX_HITS: usize = 2_000;
pub const DEFAULT_MAX_BYTES_SCANNED: u64 = 128 * 1024 * 1024;
pub const DEFAULT_BATCH_FILES: usize = 64;
pub const DEFAULT_BATCH_HITS: usize = 128;
pub const DEFAULT_MAX_EXCERPT_BYTES: usize = 200;
pub const DEFAULT_MAX_DEPTH: usize = 64;
pub const DEFAULT_BATCH_ENTRIES: usize = 512;
/// Maximum pending entries retained in a cursor (bounds the job envelope).
pub const DEFAULT_MAX_PENDING: usize = 4096;

/// A literal content query. Matching is byte-based so UTF-8 contents and
/// Unicode file names never produce invalid offsets; with `case_sensitive`
/// false only ASCII case is folded, preserving byte positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    pub case_sensitive: bool,
}

impl SearchQuery {
    pub fn new(text: impl Into<String>, case_sensitive: bool) -> Self {
        Self {
            text: text.into(),
            case_sensitive,
        }
    }
}

/// A single literal match.
///
/// `line` is one-based. `byte` is the match start's absolute byte offset within
/// the raw file, so for CRLF content it counts the `\r` of every preceding pair.
/// `column` is the byte offset within the line *as the editor stores it* — the
/// line text with a trailing `\r` removed, exactly what `str::lines()` yields —
/// so navigation consumers can set a cursor without reconstructing raw offsets.
/// Both fields are always self-consistent: `column` is derived from the same
/// stripped line that produced the match, never recomputed from `byte`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub path: PathBuf,
    pub line: u64,
    /// Absolute offset in the raw file, including preceding CRLF `\r` bytes.
    pub byte: u64,
    /// Byte offset of the match within the line text (trailing `\r` removed).
    pub column: u64,
    pub excerpt: String,
}

/// Bounds for a resumable traversal. `max_pending` caps the pending
/// *directory* deque (and, for a content search, the derived file queue) so the
/// cursor can never exceed the job envelope; it does not bound the batch sizes,
/// which are `batch_files`/`batch_hits`/`batch_entries`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchLimits {
    pub max_file_bytes: u64,
    pub max_files: usize,
    pub max_hits: usize,
    pub max_bytes_scanned: u64,
    pub batch_files: usize,
    pub batch_hits: usize,
    pub max_excerpt_bytes: usize,
    pub max_depth: usize,
    pub max_pending: usize,
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_files: DEFAULT_MAX_FILES,
            max_hits: DEFAULT_MAX_HITS,
            max_bytes_scanned: DEFAULT_MAX_BYTES_SCANNED,
            batch_files: DEFAULT_BATCH_FILES,
            batch_hits: DEFAULT_BATCH_HITS,
            max_excerpt_bytes: DEFAULT_MAX_EXCERPT_BYTES,
            max_depth: DEFAULT_MAX_DEPTH,
            max_pending: DEFAULT_MAX_PENDING,
        }
    }
}

impl SearchLimits {
    #[allow(dead_code)]
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.max_file_bytes == 0 {
            return Err("max_file_bytes");
        }
        if self.max_files == 0 {
            return Err("max_files");
        }
        if self.max_hits == 0 {
            return Err("max_hits");
        }
        if self.max_bytes_scanned == 0 {
            return Err("max_bytes_scanned");
        }
        if self.batch_files == 0 {
            return Err("batch_files");
        }
        if self.batch_hits == 0 {
            return Err("batch_hits");
        }
        if self.max_excerpt_bytes == 0 {
            return Err("max_excerpt_bytes");
        }
        if self.max_depth == 0 {
            return Err("max_depth");
        }
        if self.max_pending == 0 {
            return Err("max_pending");
        }
        Ok(())
    }
}

/// A directory awaiting or undergoing enumeration. `offset` is the saved
/// position of the next name-sorted entry to process, so a listing paused by the
/// pending bound resumes exactly where it stopped instead of re-processing the
/// directory from zero. `counted` records whether the directory has already been
/// charged to `SearchCursor::dirs_seen`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirListing {
    pub dir: PathBuf,
    pub depth: usize,
    pub offset: usize,
    pub counted: bool,
}

/// Resumable traversal state for a content search.
///
/// `pending` is a FIFO of directories awaiting or undergoing enumeration; a
/// listing that cannot enqueue another entry because the retained structures are
/// full is requeued with its `offset` saved, so every continuation strictly
/// advances and no directory is dropped. `queue` is a FIFO of files awaiting a
/// scan. The retained structures (`pending` + `queue`, plus `visited`) are
/// jointly bounded by `max_pending`, so the cursor never exceeds the job
/// envelope. `visited` holds directory identities already descended so symlink
/// loops terminate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchCursor {
    pub pending: VecDeque<DirListing>,
    pub queue: VecDeque<PathBuf>,
    pub visited: Vec<(u64, u64)>,
    pub files_seen: usize,
    pub dirs_seen: usize,
    pub bytes_seen: u64,
    pub hits_seen: usize,
    pub unreadable: usize,
    pub done: bool,
    pub capped: bool,
}

impl SearchCursor {
    pub fn new(root: &Path) -> Self {
        let mut pending = VecDeque::new();
        pending.push_back(DirListing {
            dir: root.to_path_buf(),
            depth: 0,
            offset: 0,
            counted: false,
        });
        Self {
            pending,
            ..Self::default()
        }
    }

    /// A traversal is incomplete whenever it was capped, skipped entries, or
    /// has not yet exhausted the tree.
    #[allow(dead_code)]
    pub fn incomplete(&self) -> bool {
        self.capped || self.unreadable > 0 || !self.done
    }
}

/// One resumable content-search batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentBatch {
    pub hits: Vec<SearchHit>,
    pub cursor: SearchCursor,
}

/// Resumable traversal state for a filename index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexCursor {
    pub pending: VecDeque<(PathBuf, usize)>,
    /// Listed-but-not-yet-emitted entries for the current directory: path,
    /// whether it is a directory that still needs descending, and (for
    /// directories) the depth at which it will be listed.
    pub queue: VecDeque<(PathBuf, bool, usize)>,
    pub visited: Vec<(u64, u64)>,
    pub entries: usize,
    pub dirs_seen: usize,
    pub unreadable: usize,
    pub done: bool,
    pub capped: bool,
}

impl IndexCursor {
    pub fn new(root: &Path) -> Self {
        let mut pending = VecDeque::new();
        pending.push_back((root.to_path_buf(), 0));
        Self {
            pending,
            ..Self::default()
        }
    }
}

/// One resumable filename-index batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexBatch {
    pub paths: Vec<PathBuf>,
    pub cursor: IndexCursor,
}

/// Bounded filename-index request parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRequest {
    pub excludes: Vec<String>,
    pub max_entries: usize,
    pub batch_entries: usize,
    pub max_depth: usize,
    pub max_pending: usize,
    pub cursor: IndexCursor,
}

/// Bounded content-search request parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentRequest {
    pub query: SearchQuery,
    pub excludes: Vec<String>,
    pub limits: SearchLimits,
    pub cursor: SearchCursor,
}

impl ContentRequest {
    #[allow(dead_code)]
    pub fn new(
        root: &Path,
        query: SearchQuery,
        excludes: Vec<String>,
        limits: SearchLimits,
    ) -> Self {
        Self {
            query,
            excludes,
            limits,
            cursor: SearchCursor::new(root),
        }
    }
}

/// A resumable search worker request, discriminated by result kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchJob {
    Index(IndexRequest),
    Content(ContentRequest),
}

fn is_excluded(name: &OsStr, excludes: &[String]) -> bool {
    excludes.iter().any(|exclude| name == exclude.as_str())
}

#[cfg(unix)]
fn dir_identity(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn dir_identity(_meta: &std::fs::Metadata) -> (u64, u64) {
    (0, 0)
}

/// Pop the next directory to scan, if any remain under the depth bound.
fn next_directory(
    pending: &mut VecDeque<(PathBuf, usize)>,
    max_depth: usize,
    capped: &mut bool,
) -> Option<(PathBuf, usize)> {
    while let Some((dir, depth)) = pending.pop_front() {
        if depth > max_depth {
            *capped = true;
            continue;
        }
        return Some((dir, depth));
    }
    None
}

/// Pop the next content listing to advance, skipping directories beyond the
/// depth bound (which marks the traversal capped, as before).
fn next_content_listing(
    pending: &mut VecDeque<DirListing>,
    max_depth: usize,
    capped: &mut bool,
) -> Option<DirListing> {
    while let Some(listing) = pending.pop_front() {
        if listing.depth > max_depth {
            *capped = true;
            continue;
        }
        return Some(listing);
    }
    None
}

/// Find `needle` in `hay`, returning its byte offset. ASCII case folding keeps
/// every offset byte-accurate for arbitrary UTF-8 content.
pub fn find_bytes(hay: &[u8], needle: &[u8], case_sensitive: bool) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).position(|window| {
        window.iter().zip(needle).all(|(a, b)| {
            if case_sensitive {
                a == b
            } else {
                a.eq_ignore_ascii_case(b)
            }
        })
    })
}

fn build_excerpt(line: &[u8], match_at: usize, max_bytes: usize) -> String {
    let half = max_bytes / 2;
    let start = match_at.saturating_sub(half).min(line.len());
    let end = start.saturating_add(max_bytes).min(line.len());
    String::from_utf8_lossy(&line[start..end])
        .trim()
        .to_string()
}

/// Binary detection: any NUL byte in the scanned window marks the file binary.
/// The window is exactly the read window, itself bounded by `max_file_bytes`, so
/// a NUL is never missed inside the bytes this search actually inspects.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

/// Scan one file into `hits`, bounded by the remaining hit budget. Returns the
/// number of bytes read so the caller can charge the global byte budget.
fn scan_file(
    path: &Path,
    query: &SearchQuery,
    limits: &SearchLimits,
    remaining_hits: usize,
    hits: &mut Vec<SearchHit>,
) -> Result<u64, ()> {
    let file = std::fs::File::open(path).map_err(|_| ())?;
    let mut bytes = Vec::with_capacity((limits.max_file_bytes.min(64 * 1024)) as usize);
    file.take(limits.max_file_bytes)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if looks_binary(&bytes) {
        return Ok(bytes.len() as u64);
    }
    let needle = query.text.as_bytes();
    let mut line_number = 0u64;
    let mut cursor = 0usize;
    while cursor <= bytes.len() {
        let end = bytes[cursor..]
            .iter()
            .position(|&byte| byte == b'\n')
            .map_or(bytes.len(), |offset| cursor + offset);
        line_number += 1;
        let raw = &bytes[cursor..end];
        let line = raw.strip_suffix(b"\r").unwrap_or(raw);
        if let Some(column) = find_bytes(line, needle, query.case_sensitive) {
            hits.push(SearchHit {
                path: path.to_path_buf(),
                line: line_number,
                byte: (cursor + column) as u64,
                column: column as u64,
                excerpt: build_excerpt(line, column, limits.max_excerpt_bytes),
            });
            if hits.len() >= remaining_hits {
                break;
            }
        }
        if end >= bytes.len() {
            break;
        }
        cursor = end + 1;
    }
    Ok(bytes.len() as u64)
}

/// Advance a content search by at most one resumable batch.
///
/// File scans come first (FIFO), then the next directory listing is advanced.
/// A listing that cannot enqueue another entry because the retained structures
/// are full is requeued with its saved position, so a continuation makes
/// progress instead of re-listing the same directory from zero. Reaching the
/// pending bound is therefore a resumable pause, never a cap; only the genuine
/// budgets (`max_hits`/`max_files`/`max_bytes_scanned`/depth) and a cancellation
/// deadline set `capped`.
pub fn run_content(request: &ContentRequest, stopped: &dyn Fn() -> bool) -> ContentBatch {
    let mut cursor = request.cursor.clone();
    let mut hits = Vec::new();
    let mut files_this_batch = 0usize;
    loop {
        if stopped() {
            cursor.capped = true;
            break;
        }
        if hits.len() >= request.limits.batch_hits || files_this_batch >= request.limits.batch_files
        {
            break;
        }
        if cursor.hits_seen.saturating_add(hits.len()) >= request.limits.max_hits
            || cursor.files_seen >= request.limits.max_files
            || cursor.bytes_seen >= request.limits.max_bytes_scanned
        {
            cursor.capped = true;
            break;
        }
        if let Some(path) = cursor.queue.pop_front() {
            let remaining = request
                .limits
                .max_hits
                .saturating_sub(cursor.hits_seen)
                .saturating_sub(hits.len());
            if remaining == 0 {
                cursor.capped = true;
                break;
            }
            cursor.files_seen = cursor.files_seen.saturating_add(1);
            files_this_batch += 1;
            match scan_file(&path, &request.query, &request.limits, remaining, &mut hits) {
                Ok(bytes) => cursor.bytes_seen = cursor.bytes_seen.saturating_add(bytes),
                Err(()) => cursor.unreadable = cursor.unreadable.saturating_add(1),
            }
            continue;
        }
        let Some(mut listing) = next_content_listing(
            &mut cursor.pending,
            request.limits.max_depth,
            &mut cursor.capped,
        ) else {
            cursor.done = true;
            break;
        };
        if !listing.counted {
            cursor.dirs_seen = cursor.dirs_seen.saturating_add(1);
            listing.counted = true;
        }
        if !advance_listing(&mut listing, &mut cursor, request) {
            // The listing is paused with its position saved; give the
            // directories it already discovered a turn before resuming it.
            cursor.pending.push_back(listing);
        }
    }
    cursor.hits_seen = cursor.hits_seen.saturating_add(hits.len());
    ContentBatch { hits, cursor }
}

/// Collect a directory's entries in deterministic name order. `read_dir` yields
/// an OS-defined order, so sorting is what makes FIFO batching reproducible.
fn sorted_entries(dir: &Path) -> Option<Vec<std::fs::DirEntry>> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut collected: Vec<std::fs::DirEntry> = entries.filter_map(Result::ok).collect();
    collected.sort_by_key(|entry| entry.file_name());
    Some(collected)
}

/// Advance one directory listing by enqueuing at least one remaining entry and
/// then as many more as the retained structures can hold.
///
/// The pending directories and the queued files are charged jointly against
/// `request.limits.max_pending`. That bound is *soft* by design: the next entry
/// is consumed before the bound is re-checked, because popping a listing does
/// not itself reduce `pending.len()` while other listings remain, so a listing
/// that paused with an exhausted file queue would otherwise be requeued
/// unchanged forever. Consuming one entry first guarantees strict progress on
/// every continuation (the caller performs at most one `pop_front`, so
/// `pending.len()` momentarily has room for one more retained entry). The
/// retained set can therefore overshoot `max_pending`: measurement shows it
/// settles at roughly `2 * max_pending + children_per_dir` once nested
/// directories are discovered after `visited` saturates. That overshoot is
/// bounded by shape rather than by tree size and stays inside the job envelope
/// at default limits because `search_cursor_bytes` charges the real retained
/// length. `listing.offset` is updated to the first unprocessed entry,
/// so resuming never re-processes an entry and never drops one. Returns `true`
/// when the listing is exhausted, `false` when it paused at the bound.
fn advance_listing(
    listing: &mut DirListing,
    cursor: &mut SearchCursor,
    request: &ContentRequest,
) -> bool {
    let max_pending = request.limits.max_pending;
    let entries = match sorted_entries(&listing.dir) {
        Some(entries) => entries,
        None => {
            cursor.unreadable = cursor.unreadable.saturating_add(1);
            return true;
        }
    };
    let mut first = true;
    while listing.offset < entries.len() {
        // The first entry of every advance is unconditional; subsequent entries
        // require room under the joint retained bound.
        if !first && cursor.pending.len().saturating_add(cursor.queue.len()) >= max_pending {
            return false;
        }
        first = false;
        let entry = &entries[listing.offset];
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                cursor.unreadable = cursor.unreadable.saturating_add(1);
                listing.offset += 1;
                continue;
            }
        };
        if file_type.is_dir() {
            if is_excluded(&entry.file_name(), &request.excludes) {
                listing.offset += 1;
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                let identity = dir_identity(&meta);
                if identity != (0, 0) && cursor.visited.contains(&identity) {
                    listing.offset += 1;
                    continue;
                }
                if cursor.visited.len() < max_pending {
                    cursor.visited.push(identity);
                }
            }
            cursor.pending.push_back(DirListing {
                dir: entry.path(),
                depth: listing.depth + 1,
                offset: 0,
                counted: false,
            });
        } else {
            cursor.queue.push_back(entry.path());
        }
        listing.offset += 1;
    }
    true
}

/// List a directory's children into `cursor.queue`, honouring exclusions,
/// symlink safety and the pending bound. Returns false when a bound was hit.
fn list_index_children(
    dir: &Path,
    depth: usize,
    request: &IndexRequest,
    cursor: &mut IndexCursor,
) -> bool {
    let entries = match sorted_entries(dir) {
        Some(entries) => entries,
        None => {
            cursor.unreadable = cursor.unreadable.saturating_add(1);
            return true;
        }
    };
    for entry in entries {
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                cursor.unreadable = cursor.unreadable.saturating_add(1);
                continue;
            }
        };
        if cursor.queue.len() >= request.max_pending {
            return false;
        }
        if file_type.is_dir() {
            if is_excluded(&entry.file_name(), &request.excludes) {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                let identity = dir_identity(&meta);
                if identity != (0, 0) && cursor.visited.contains(&identity) {
                    continue;
                }
                if cursor.visited.len() < request.max_pending {
                    cursor.visited.push(identity);
                }
            }
            cursor.queue.push_back((entry.path(), true, depth + 1));
        } else {
            cursor.queue.push_back((entry.path(), false, 0));
        }
    }
    true
}

/// Advance a filename index by at most one resumable batch.
pub fn run_index(request: &IndexRequest, stopped: &dyn Fn() -> bool) -> IndexBatch {
    let mut cursor = request.cursor.clone();
    let mut paths = Vec::new();
    loop {
        if stopped() {
            cursor.capped = true;
            break;
        }
        if paths.len() >= request.batch_entries {
            break;
        }
        if cursor.entries >= request.max_entries {
            cursor.capped = true;
            break;
        }
        if let Some((path, is_dir, depth)) = cursor.queue.pop_front() {
            paths.push(path.clone());
            cursor.entries = cursor.entries.saturating_add(1);
            if is_dir {
                if depth > request.max_depth {
                    cursor.capped = true;
                    continue;
                }
                if cursor.pending.len() >= request.max_pending {
                    cursor.capped = true;
                    break;
                }
                cursor.pending.push_back((path, depth));
            }
            continue;
        }
        let Some((dir, depth)) =
            next_directory(&mut cursor.pending, request.max_depth, &mut cursor.capped)
        else {
            cursor.done = true;
            break;
        };
        cursor.dirs_seen = cursor.dirs_seen.saturating_add(1);
        if !list_index_children(&dir, depth, request, &mut cursor) {
            cursor.capped = true;
            break;
        }
    }
    IndexBatch { paths, cursor }
}

/// Build default exclusions as owned strings.
pub fn default_excludes() -> Vec<String> {
    DEFAULT_EXCLUDES
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

/// Drive a content search to completion synchronously (fallback/tests).
pub fn search_project(
    root: &Path,
    query: SearchQuery,
    excludes: &[String],
    limits: &SearchLimits,
) -> ContentBatch {
    let mut cursor = SearchCursor::new(root);
    let mut all = Vec::new();
    loop {
        let request = ContentRequest {
            query: query.clone(),
            excludes: excludes.to_vec(),
            limits: *limits,
            cursor: cursor.clone(),
        };
        let batch = run_content(&request, &|| false);
        let finished = batch.cursor.done || batch.cursor.capped;
        all.extend(batch.hits);
        cursor = batch.cursor;
        if finished {
            break;
        }
    }
    ContentBatch { hits: all, cursor }
}

/// Drive a filename index to completion synchronously (fallback/tests).
#[allow(dead_code)]
pub fn index_project(root: &Path, max_entries: usize, excludes: &[String]) -> IndexBatch {
    let mut cursor = IndexCursor::new(root);
    let mut all = Vec::new();
    loop {
        let request = IndexRequest {
            excludes: excludes.to_vec(),
            max_entries,
            batch_entries: 32,
            max_depth: DEFAULT_MAX_DEPTH,
            max_pending: DEFAULT_MAX_PENDING,
            cursor: cursor.clone(),
        };
        let batch = run_index(&request, &|| false);
        let finished = batch.cursor.done || batch.cursor.capped;
        all.extend(batch.paths);
        cursor = batch.cursor;
        if finished {
            break;
        }
    }
    IndexBatch { paths: all, cursor }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn project() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path();
        fs::write(root_path.join("config.yaml"), "training:\n  lr: 0.001\n").unwrap();
        fs::write(root_path.join("README.md"), "no match here\n").unwrap();
        fs::write(
            root_path.join("unicode_π.txt"),
            "café settings: π = 3.14159\nsecond line\n",
        )
        .unwrap();
        fs::write(root_path.join("binary.dat"), b"\x00\x01\x02training\x00").unwrap();
        // A file larger than the (test-reduced) cap must be truncated, not read.
        let mut large = String::new();
        for index in 0..1000 {
            large.push_str(&format!("line {index} filler filler\n"));
        }
        large.push_str("late training marker\n");
        fs::write(root_path.join("large.log"), large).unwrap();
        fs::create_dir_all(root_path.join(".venv").join("lib")).unwrap();
        fs::write(
            root_path.join(".venv").join("lib").join("hidden.py"),
            "training\n",
        )
        .unwrap();
        fs::create_dir_all(root_path.join("target")).unwrap();
        fs::write(root_path.join("target").join("artifact"), "training\n").unwrap();
        project_extras(root_path);
        root
    }

    #[cfg(unix)]
    fn project_extras(root: &Path) {
        use std::os::unix::fs::{symlink, PermissionsExt};
        // Symlink loop: a directory symlink that points back at the root.
        let _ = symlink(root, root.join("loop"));
        // Unreadable directory (best effort: root can bypass, so also exercise
        // a missing directory via a dangling symlink entry).
        let _ = symlink(root.join("does-not-exist"), root.join("dangling"));
        let unreadable = root.join("sealed");
        let _ = fs::create_dir(&unreadable);
        let _ = fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000));
    }

    #[cfg(not(unix))]
    fn project_extras(root: &Path) {
        let _ = std::fs::create_dir(root.join("sealed"));
    }

    fn default_limits() -> SearchLimits {
        SearchLimits::default()
    }

    #[test]
    fn search_plan_temporary_project_assertions_hold() {
        let root = project();
        let limits = SearchLimits {
            max_file_bytes: 4096,
            ..default_limits()
        };
        let outcome = search_project(
            root.path(),
            SearchQuery::new("training", false),
            &DEFAULT_EXCLUDES
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
            &limits,
        );
        let hits = outcome.hits;
        assert!(!hits.is_empty());
        let config = root.path().join("config.yaml");
        assert_eq!(hits[0].path, config);
        assert_eq!(hits[0].line, 1);
        assert!(
            !hits
                .iter()
                .any(|h| h.path.starts_with(root.path().join(".venv"))),
            "excluded directory leaked into results"
        );
        assert!(
            !hits
                .iter()
                .any(|h| h.path.starts_with(root.path().join("target"))),
            "excluded target directory leaked into results"
        );
        assert!(
            !hits
                .iter()
                .any(|h| h.path == root.path().join("binary.dat")),
            "binary file must be skipped"
        );
        // Truncated large file still yields hits within the retained window.
        assert!(outcome.cursor.done || outcome.cursor.capped);
    }

    #[test]
    fn search_unicode_names_and_contents_match_without_invalid_offsets() {
        let root = project();
        let outcome = search_project(
            root.path(),
            SearchQuery::new("π", true),
            &default_excludes(),
            &default_limits(),
        );
        let hit = outcome
            .hits
            .iter()
            .find(|h| h.path.ends_with("unicode_π.txt"))
            .expect("unicode file matched");
        let contents = fs::read(&hit.path).unwrap();
        let at = hit.byte as usize;
        assert_eq!(
            &contents[at..at + "π".len()],
            "π".as_bytes(),
            "byte offset must be a valid UTF-8 boundary"
        );
    }

    #[test]
    fn search_ascii_case_insensitive_keeps_offsets() {
        let root = project();
        let outcome = search_project(
            root.path(),
            SearchQuery::new("TRAINING", false),
            &default_excludes(),
            &default_limits(),
        );
        assert!(outcome
            .hits
            .iter()
            .any(|h| h.path.ends_with("config.yaml") && h.line == 1));
    }

    #[test]
    fn search_symlink_loop_terminates_because_dirs_are_not_followed() {
        let root = project();
        let outcome = search_project(
            root.path(),
            SearchQuery::new("training", false),
            &default_excludes(),
            &SearchLimits {
                max_files: 10_000,
                ..default_limits()
            },
        );
        // Reaching here without infinite recursion while producing bounded
        // results is the assertion; the loop symlink is never descended.
        assert!(outcome.cursor.dirs_seen < 10_000);
        assert!(outcome
            .hits
            .iter()
            .all(|h| !h.path.starts_with(root.path().join("loop"))));
    }

    #[test]
    fn search_reports_cap_when_hit_budget_is_reached() {
        let root = project();
        let outcome = search_project(
            root.path(),
            SearchQuery::new("training", false),
            &default_excludes(),
            &SearchLimits {
                max_hits: 1,
                batch_hits: 1,
                ..default_limits()
            },
        );
        assert_eq!(outcome.hits.len(), 1);
        assert!(outcome.cursor.capped);
        assert!(outcome.cursor.incomplete());
    }

    #[test]
    fn search_batches_are_resumable_and_deterministic() {
        let root = project();
        let request = ContentRequest::new(
            root.path(),
            SearchQuery::new("training", false),
            default_excludes(),
            SearchLimits {
                batch_files: 1,
                ..default_limits()
            },
        );
        let first = run_content(&request, &|| false);
        let mut cursor = first.cursor.clone();
        let mut all: Vec<SearchHit> = first.hits;
        while !cursor.done && !cursor.capped {
            let next = run_content(
                &ContentRequest {
                    cursor: cursor.clone(),
                    ..request.clone()
                },
                &|| false,
            );
            all.extend(next.hits);
            cursor = next.cursor;
        }
        let single = search_project(
            root.path(),
            SearchQuery::new("training", false),
            &default_excludes(),
            &default_limits(),
        );
        assert_eq!(all, single.hits, "batched traversal is not deterministic");
    }

    #[test]
    fn search_symlink_to_file_is_still_scanned() {
        #[cfg(unix)]
        {
            let root = tempfile::tempdir().unwrap();
            fs::write(root.path().join("real.txt"), "needle\n").unwrap();
            std::os::unix::fs::symlink(root.path().join("real.txt"), root.path().join("link.txt"))
                .unwrap();
            let outcome = search_project(
                root.path(),
                SearchQuery::new("needle", true),
                &default_excludes(),
                &default_limits(),
            );
            assert!(outcome.hits.iter().any(|h| h.path.ends_with("link.txt")));
        }
    }

    #[test]
    fn search_stopped_batch_reports_capped_incomplete() {
        let root = project();
        let request = ContentRequest::new(
            root.path(),
            SearchQuery::new("training", false),
            default_excludes(),
            SearchLimits::default(),
        );
        let batch = run_content(&request, &|| true);
        assert!(batch.cursor.capped);
        assert!(batch.cursor.incomplete());
    }

    #[test]
    fn index_project_respects_exclusions_and_reports_completion() {
        let root = project();
        let outcome = index_project(root.path(), 10_000, &default_excludes());
        assert!(outcome.paths.iter().any(|p| p.ends_with("config.yaml")));
        assert!(outcome.paths.iter().any(|p| p.ends_with("unicode_π.txt")));
        assert!(!outcome
            .paths
            .iter()
            .any(|p| p.starts_with(root.path().join(".venv"))));
        assert!(outcome.cursor.done);
    }

    #[test]
    fn index_batches_are_resumable_and_bounded() {
        let root = project();
        let mut cursor = IndexCursor::new(root.path());
        let mut all = Vec::new();
        let mut batches = 0;
        loop {
            let batch = run_index(
                &IndexRequest {
                    excludes: default_excludes(),
                    max_entries: 10_000,
                    batch_entries: 2,
                    max_depth: DEFAULT_MAX_DEPTH,
                    max_pending: DEFAULT_MAX_PENDING,
                    cursor: cursor.clone(),
                },
                &|| false,
            );
            batches += 1;
            assert!(batch.paths.len() <= 2);
            all.extend(batch.paths);
            let done = batch.cursor.done || batch.cursor.capped;
            cursor = batch.cursor;
            if done {
                break;
            }
            assert!(batches < 10_000);
        }
        assert!(all.iter().any(|p| p.ends_with("config.yaml")));
    }

    #[test]
    fn content_wide_directory_is_resumable_not_capped() {
        // A directory with more files than the pending bound used to report a
        // hard cap and drop the rest of the listing. It is now a resumable
        // pause: every continuation strictly advances, the cursor stays inside
        // the bound, and all matching files are eventually returned.
        let root = tempfile::tempdir().unwrap();
        for index in 0..64 {
            fs::write(
                root.path().join(format!("f{index:02}.txt")),
                format!("needle {index}\n"),
            )
            .unwrap();
        }
        let limits = SearchLimits {
            max_pending: 8,
            batch_files: 2,
            ..SearchLimits::default()
        };
        let request = ContentRequest::new(
            root.path(),
            SearchQuery::new("needle", false),
            Vec::new(),
            limits,
        );
        let mut cursor = request.cursor.clone();
        let mut hits = 0usize;
        let mut previous_files = 0usize;
        let mut iterations = 0usize;
        loop {
            let batch = run_content(
                &ContentRequest {
                    cursor: cursor.clone(),
                    ..request.clone()
                },
                &|| false,
            );
            assert!(!batch.cursor.capped, "the pending bound is not a cap");
            assert!(batch.cursor.queue.len() <= limits.max_pending);
            assert!(batch.cursor.pending.len() <= limits.max_pending);
            assert!(
                batch.cursor.files_seen > previous_files || batch.cursor.done,
                "each continuation must strictly advance"
            );
            previous_files = batch.cursor.files_seen;
            hits += batch.hits.len();
            let next = batch.cursor;
            iterations += 1;
            assert!(iterations < 10_000);
            let finished = next.done || next.capped;
            cursor = next;
            if finished {
                break;
            }
        }
        assert_eq!(hits, 64, "every reachable file must be scanned");
        assert!(cursor.done && !cursor.capped);
    }

    #[test]
    fn content_nul_after_the_sniff_window_is_still_binary() {
        // A NUL beyond any naive 8192-byte sniff window still marks the file
        // binary: the scan window is exactly the bounded read.
        let root = tempfile::tempdir().unwrap();
        let mut payload = vec![b'a'; 20_000];
        payload.extend_from_slice(b"\0needle\n");
        fs::write(root.path().join("late_nul.dat"), &payload).unwrap();
        let outcome = search_project(
            root.path(),
            SearchQuery::new("needle", false),
            &default_excludes(),
            &SearchLimits::default(),
        );
        assert!(
            outcome.hits.is_empty(),
            "late NUL must not be searched as text"
        );

        // Control: the same bytes without the NUL are searched normally.
        let mut text = vec![b'a'; 20_000];
        text.extend_from_slice(b"needle\n");
        fs::write(root.path().join("late_text.dat"), &text).unwrap();
        let outcome = search_project(
            root.path(),
            SearchQuery::new("needle", false),
            &default_excludes(),
            &SearchLimits::default(),
        );
        assert!(outcome
            .hits
            .iter()
            .any(|hit| hit.path.ends_with("late_text.dat")));
    }

    #[test]
    fn content_hit_reports_within_line_column_for_lf_and_crlf() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("lf.txt"), b"aaa\nxx needle\n").unwrap();
        fs::write(root.path().join("crlf.txt"), b"aaa\r\nxx needle\r\n").unwrap();
        let outcome = search_project(
            root.path(),
            SearchQuery::new("needle", false),
            &default_excludes(),
            &SearchLimits::default(),
        );
        for name in ["lf.txt", "crlf.txt"] {
            let hit = outcome
                .hits
                .iter()
                .find(|hit| hit.path.ends_with(name))
                .unwrap_or_else(|| panic!("missing hit for {name}"));
            assert_eq!(hit.line, 2, "{name}");
            assert_eq!(hit.column, 3, "{name}: column is buffer-relative");
        }
        // `byte` keeps its absolute raw-file contract: CRLF shifts it by one
        // for every preceding pair.
        let crlf = outcome
            .hits
            .iter()
            .find(|hit| hit.path.ends_with("crlf.txt"))
            .unwrap();
        assert_eq!(crlf.byte, 8);
        let lf = outcome
            .hits
            .iter()
            .find(|hit| hit.path.ends_with("lf.txt"))
            .unwrap();
        assert_eq!(lf.byte, 7);
    }

    #[test]
    fn content_queue_is_fifo_across_batches() {
        // Files must be scanned in the order they were enqueued (FIFO), so a
        // resumable batch ordering is deterministic and reproducible.
        let root = tempfile::tempdir().unwrap();
        for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
            fs::write(root.path().join(name), b"needle\n").unwrap();
        }
        let request = ContentRequest::new(
            root.path(),
            SearchQuery::new("needle", false),
            Vec::new(),
            SearchLimits {
                batch_files: 1,
                ..SearchLimits::default()
            },
        );
        let mut cursor = request.cursor.clone();
        let mut order = Vec::new();
        for _ in 0..16 {
            let batch = run_content(
                &ContentRequest {
                    cursor: cursor.clone(),
                    ..request.clone()
                },
                &|| false,
            );
            order.extend(
                batch
                    .hits
                    .iter()
                    .map(|hit| hit.path.file_name().unwrap().to_string_lossy().into_owned()),
            );
            let finished = batch.cursor.done || batch.cursor.capped;
            cursor = batch.cursor;
            if finished {
                break;
            }
        }
        assert_eq!(order, vec!["a.txt", "b.txt", "c.txt", "d.txt"]);
    }

    #[test]
    fn limits_validate_rejects_each_zero_field() {
        assert!(SearchLimits::default().validate().is_ok());
        for limits in [
            SearchLimits {
                max_file_bytes: 0,
                ..default_limits()
            },
            SearchLimits {
                max_files: 0,
                ..default_limits()
            },
            SearchLimits {
                max_hits: 0,
                ..default_limits()
            },
            SearchLimits {
                max_bytes_scanned: 0,
                ..default_limits()
            },
            SearchLimits {
                batch_files: 0,
                ..default_limits()
            },
            SearchLimits {
                batch_hits: 0,
                ..default_limits()
            },
            SearchLimits {
                max_excerpt_bytes: 0,
                ..default_limits()
            },
            SearchLimits {
                max_depth: 0,
                ..default_limits()
            },
            SearchLimits {
                max_pending: 0,
                ..default_limits()
            },
        ] {
            assert!(limits.validate().is_err());
        }
    }

    #[test]
    fn content_traversal_reports_unreadable_missing_and_depth_limits() {
        let limits = SearchLimits::default();
        // A directory that cannot be read is counted, not fatal.
        let mut missing_dir = SearchCursor::default();
        missing_dir.pending.push_back(DirListing {
            dir: PathBuf::from("/definitely/not/here"),
            depth: 0,
            offset: 0,
            counted: false,
        });
        let batch = run_content(
            &ContentRequest {
                query: SearchQuery::new("x", true),
                excludes: Vec::new(),
                limits,
                cursor: missing_dir,
            },
            &|| false,
        );
        assert!(batch.cursor.unreadable >= 1);
        assert!(batch.cursor.done);

        // A queued file that cannot be opened is counted as unreadable.
        let mut missing_file = SearchCursor::default();
        missing_file
            .queue
            .push_back(PathBuf::from("/definitely/not/here.txt"));
        let batch = run_content(
            &ContentRequest {
                query: SearchQuery::new("x", true),
                excludes: Vec::new(),
                limits,
                cursor: missing_file,
            },
            &|| false,
        );
        assert!(batch.cursor.unreadable >= 1);

        // A pending directory beyond the depth bound is skipped and capped.
        let mut pending = VecDeque::new();
        pending.push_back(DirListing {
            dir: PathBuf::from("/root"),
            depth: 9,
            offset: 0,
            counted: false,
        });
        let mut capped = false;
        assert!(next_content_listing(&mut pending, 2, &mut capped).is_none());
        assert!(capped);
    }

    #[test]
    fn content_pending_bound_is_a_resumable_pause_not_a_cap() {
        // `max_pending = 1` with a file and a subdirectory: the first batch
        // pauses with the listing position saved, and every continuation keeps
        // making progress until the traversal genuinely completes.
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("f.txt"), "x\n").unwrap();
        fs::create_dir(root.path().join("sub")).unwrap();
        let request = ContentRequest {
            query: SearchQuery::new("x", true),
            excludes: Vec::new(),
            limits: SearchLimits {
                max_pending: 1,
                ..SearchLimits::default()
            },
            cursor: SearchCursor::new(root.path()),
        };
        let mut cursor = request.cursor.clone();
        let mut hits = 0usize;
        let mut iterations = 0usize;
        loop {
            let batch = run_content(
                &ContentRequest {
                    cursor: cursor.clone(),
                    ..request.clone()
                },
                &|| false,
            );
            assert!(batch.cursor.queue.len() <= 1);
            assert!(batch.cursor.pending.len() <= 1);
            hits += batch.hits.len();
            let next = batch.cursor;
            iterations += 1;
            assert!(iterations < 100);
            let finished = next.done || next.capped;
            cursor = next;
            if finished {
                break;
            }
        }
        assert_eq!(hits, 1, "the file was still scanned");
        assert!(cursor.done && !cursor.capped);
    }

    #[test]
    fn index_reports_unreadable_caps_and_stopped_batches() {
        let mut missing = IndexCursor::default();
        missing
            .pending
            .push_back((PathBuf::from("/definitely/not/here"), 0));
        let batch = run_index(
            &IndexRequest {
                excludes: Vec::new(),
                max_entries: 4,
                batch_entries: 4,
                max_depth: 4,
                max_pending: 16,
                cursor: missing,
            },
            &|| false,
        );
        assert!(batch.cursor.unreadable >= 1);
        assert!(batch.cursor.done);

        let root = tempfile::tempdir().unwrap();
        for index in 0..8 {
            fs::write(root.path().join(format!("f{index}.txt")), b"x").unwrap();
        }
        let capped = run_index(
            &IndexRequest {
                excludes: Vec::new(),
                max_entries: 1,
                batch_entries: 100,
                max_depth: 4,
                max_pending: 16,
                cursor: IndexCursor::new(root.path()),
            },
            &|| false,
        );
        assert!(capped.cursor.capped);

        let stopped = run_index(
            &IndexRequest {
                excludes: Vec::new(),
                max_entries: 100,
                batch_entries: 100,
                max_depth: 4,
                max_pending: 16,
                cursor: IndexCursor::new(root.path()),
            },
            &|| true,
        );
        assert!(stopped.cursor.capped);
        assert!(stopped.paths.is_empty());
    }

    #[test]
    fn find_bytes_handles_empty_and_multibyte_needles() {
        assert_eq!(find_bytes(b"abc", b"", true), None);
        assert_eq!(find_bytes("aπb".as_bytes(), "π".as_bytes(), true), Some(1));
        assert_eq!(find_bytes(b"ABC", b"abc", false), Some(0));
        assert_eq!(find_bytes(b"ABC", b"abc", true), None);
    }

    /// Reviewer reproduction shape for `FileManagerTUI-5yf`, scaled down: a root
    /// whose direct subdirectory count exceeds `max_pending`, plus a file that
    /// name-sorts after every one of them. The traversal must reach the file and
    /// finish honestly instead of re-listing the root and aborting with zero
    /// hits.
    #[test]
    fn content_directory_above_pending_bound_still_returns_reachable_hits() {
        let root = tempfile::tempdir().unwrap();
        let limit = 8;
        for index in 0..20 {
            fs::create_dir(root.path().join(format!("d{index:02}"))).unwrap();
        }
        fs::write(root.path().join("zzz.txt"), b"needle\n").unwrap();
        let limits = SearchLimits {
            max_pending: limit,
            batch_files: 2,
            ..SearchLimits::default()
        };
        let request = ContentRequest::new(
            root.path(),
            SearchQuery::new("needle", false),
            Vec::new(),
            limits,
        );
        let mut cursor = request.cursor.clone();
        let mut hits = Vec::new();
        let mut iterations = 0usize;
        loop {
            let batch = run_content(
                &ContentRequest {
                    cursor: cursor.clone(),
                    ..request.clone()
                },
                &|| false,
            );
            // The retained bound is soft by design: the in-flight listing
            // consumes its next entry before the bound is re-checked, and
            // nested directories discovered after `visited` saturates
            // overshoot it (measured roughly `2 * limit + children_per_dir`,
            // bounded by shape, not tree size). Assert the documented slack so
            // runaway growth still fails without pinning an invariant the
            // implementation does not promise.
            assert!(
                batch.cursor.pending.len() <= limit * 2 + 64,
                "pending cursor grew beyond its documented slack"
            );
            assert!(
                batch.cursor.queue.len() <= limit,
                "file queue exceeded its bound"
            );
            hits.extend(batch.hits);
            let next = batch.cursor;
            iterations += 1;
            assert!(iterations < 10_000, "traversal must terminate");
            let finished = next.done || next.capped;
            cursor = next;
            if finished {
                break;
            }
        }
        assert!(
            hits.iter().any(|hit| hit.path.ends_with("zzz.txt")),
            "the reachable file after the wide directory must be scanned"
        );
        assert!(cursor.done, "the traversal must run to completion");
        assert!(!cursor.capped, "no genuine cap occurred");
    }

    /// The same mechanism at the default `max_pending` (>= 4096): a root with
    /// more direct subdirectories than the bound plus one trailing file. The
    /// cursor stays bounded on every continuation and the hit is still returned.
    #[test]
    fn content_directory_above_default_pending_bound_returns_reachable_hits() {
        let root = tempfile::tempdir().unwrap();
        let count = DEFAULT_MAX_PENDING + 8;
        for index in 0..count {
            fs::create_dir(root.path().join(format!("d{index:05}"))).unwrap();
        }
        fs::write(root.path().join("zzz.txt"), b"needle\n").unwrap();
        let limits = SearchLimits::default();
        let request = ContentRequest::new(
            root.path(),
            SearchQuery::new("needle", false),
            Vec::new(),
            limits,
        );
        let mut cursor = request.cursor.clone();
        let mut hits = Vec::new();
        let mut iterations = 0usize;
        loop {
            let batch = run_content(
                &ContentRequest {
                    cursor: cursor.clone(),
                    ..request.clone()
                },
                &|| false,
            );
            assert!(batch.cursor.pending.len() <= limits.max_pending);
            assert!(batch.cursor.queue.len() <= limits.max_pending);
            hits.extend(batch.hits);
            let next = batch.cursor;
            iterations += 1;
            assert!(iterations < 10_000);
            let finished = next.done || next.capped;
            cursor = next;
            if finished {
                break;
            }
        }
        assert!(hits.iter().any(|hit| hit.path.ends_with("zzz.txt")));
        assert!(cursor.done && !cursor.capped);
    }

    /// Drive a content request to completion while asserting that the traversal
    /// always makes progress. `steps` is a hard watchdog: a livelock (a
    /// continuation that neither advances nor finishes) fails the test instead
    /// of hanging it. Returns the final cursor.
    fn drive_content_to_completion(
        request: &ContentRequest,
        limit: usize,
        max_steps: usize,
    ) -> SearchCursor {
        let mut cursor = request.cursor.clone();
        let mut steps = 0usize;
        loop {
            let before = (
                cursor.pending.len(),
                cursor.queue.len(),
                cursor.files_seen,
                cursor.dirs_seen,
            );
            let batch = run_content(
                &ContentRequest {
                    cursor: cursor.clone(),
                    ..request.clone()
                },
                &|| false,
            );
            // The retained bound is soft by design: the in-flight listing
            // consumes its next entry before the bound is re-checked, and
            // nested directories discovered after `visited` saturates
            // overshoot it (measured roughly `2 * limit + children_per_dir`,
            // bounded by shape, not tree size). Assert the documented slack so
            // runaway growth still fails without pinning an invariant the
            // implementation does not promise.
            assert!(
                batch.cursor.pending.len() <= limit * 2 + 64,
                "pending cursor grew beyond its documented slack"
            );
            assert!(
                batch.cursor.queue.len() <= limit,
                "file queue exceeded its bound"
            );
            let next = batch.cursor;
            steps += 1;
            if next.done || next.capped {
                return next;
            }
            let after = (
                next.pending.len(),
                next.queue.len(),
                next.files_seen,
                next.dirs_seen,
            );
            assert!(
                after != before,
                "continuation {steps} made no progress (livelock): \
                 pending={} queue={} files_seen={} dirs_seen={}",
                after.0,
                after.1,
                after.2,
                after.3
            );
            assert!(
                steps < max_steps,
                "traversal did not finish within {max_steps} steps (livelock)"
            );
            cursor = next;
        }
    }

    /// Livelock regression: a wide directory listing whose discovered children
    /// are themselves NON-EMPTY. Popping one paused listing does not reduce
    /// `pending.len()`, so a full pending set with an empty file queue can never
    /// be relieved. The traversal must still make progress on every
    /// continuation.
    #[test]
    fn content_full_pending_set_of_non_empty_directories_makes_progress() {
        let root = tempfile::tempdir().unwrap();
        let limit = 8;
        for index in 0..limit {
            let child = root.path().join(format!("d{index:02}"));
            fs::create_dir(&child).unwrap();
            fs::write(child.join("inner.txt"), b"needle\n").unwrap();
        }
        fs::write(root.path().join("zzz.txt"), b"needle\n").unwrap();
        let limits = SearchLimits {
            max_pending: limit,
            batch_files: 4,
            ..SearchLimits::default()
        };
        let request = ContentRequest::new(
            root.path(),
            SearchQuery::new("needle", false),
            Vec::new(),
            limits,
        );
        let cursor = drive_content_to_completion(&request, limit, 10_000);
        assert!(cursor.done, "traversal must complete");
        assert!(!cursor.capped, "no genuine cap occurred");
    }

    /// The reviewer's exact reproduction at the default configuration, scaled to
    /// the default `max_pending = DEFAULT_MAX_PENDING`: a root with exactly
    /// `DEFAULT_MAX_PENDING` subdirectories that each contain a file, plus a
    /// trailing `zzz.txt`. The pre-fix tree livelocked on the first batch with
    /// `files_seen=0 dirs_seen=4097 pending=4097 queue=0 hits=0` (a single
    /// `run_content` call never returned). The traversal must now make strict
    /// progress on every continuation and reach the trailing file.
    #[test]
    fn content_default_bound_non_empty_directories_do_not_livelock() {
        let root = tempfile::tempdir().unwrap();
        let count = DEFAULT_MAX_PENDING;
        for index in 0..count {
            let child = root.path().join(format!("d{index:05}"));
            fs::create_dir(&child).unwrap();
            fs::write(child.join("inner.txt"), b"needle\n").unwrap();
        }
        fs::write(root.path().join("zzz.txt"), b"needle\n").unwrap();
        let limits = SearchLimits::default();
        let request = ContentRequest::new(
            root.path(),
            SearchQuery::new("needle", false),
            Vec::new(),
            limits,
        );
        let mut hits = Vec::new();
        let mut cursor = request.cursor.clone();
        let mut steps = 0usize;
        // Non-batch-limited drive: each step must strictly change the cursor and
        // the whole traversal must finish well inside the watchdog.
        loop {
            let before = (
                cursor.pending.len(),
                cursor.queue.len(),
                cursor.files_seen,
                cursor.dirs_seen,
            );
            let batch = run_content(
                &ContentRequest {
                    cursor: cursor.clone(),
                    ..request.clone()
                },
                &|| false,
            );
            assert!(batch.cursor.pending.len() <= limits.max_pending);
            assert!(batch.cursor.queue.len() <= limits.max_pending);
            hits.extend(batch.hits);
            let next = batch.cursor;
            steps += 1;
            if next.done || next.capped {
                cursor = next;
                break;
            }
            let after = (
                next.pending.len(),
                next.queue.len(),
                next.files_seen,
                next.dirs_seen,
            );
            assert!(
                after != before,
                "step {steps} made no progress (livelock): pending={} queue={} \
                 files_seen={} dirs_seen={}",
                after.0,
                after.1,
                after.2,
                after.3
            );
            assert!(steps < 200_000, "livelock: {steps} steps without finishing");
            cursor = next;
        }
        // With the default 2000-hit budget the deepest subdirectories and the
        // trailing file fall outside the budget, so the traversal must report an
        // honest cap rather than a false completion or a stall, and it must have
        // made real progress (files/directories actually visited).
        assert!(
            cursor.capped,
            "a 4096-hit tree exceeds the default hit budget"
        );
        assert!(!cursor.done);
        assert!(cursor.incomplete());
        assert!(cursor.files_seen > 0, "no files were visited at all");
        assert!(cursor.dirs_seen > 0, "no directories were visited at all");
        assert!(!hits.is_empty(), "no hits were returned at all");
    }

    /// The synchronous fallback path has no deadline; it must terminate and
    /// return hits for the same non-empty shape rather than hang.
    #[test]
    fn content_sync_fallback_terminates_on_full_pending_non_empty_shape() {
        let root = tempfile::tempdir().unwrap();
        let limit = 8;
        for index in 0..(limit + 2) {
            let child = root.path().join(format!("d{index:02}"));
            fs::create_dir(&child).unwrap();
            fs::write(child.join("inner.txt"), b"needle\n").unwrap();
        }
        fs::write(root.path().join("zzz.txt"), b"needle\n").unwrap();
        let outcome = search_project(
            root.path(),
            SearchQuery::new("needle", false),
            &default_excludes(),
            &SearchLimits {
                max_pending: limit,
                ..SearchLimits::default()
            },
        );
        assert!(outcome.hits.iter().any(|hit| hit.path.ends_with("zzz.txt")));
        assert!(outcome.cursor.done);
    }
}
