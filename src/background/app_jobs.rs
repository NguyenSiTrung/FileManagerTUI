//! One typed bounded App pool: native scans, summaries, S3 and clipboard work.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::background::{
    AdmissionError, Cancellation, JobError, Limits, Payload, RequestDomain, RequestGeneration,
    Scheduler, Worker,
};
use crate::fs::tree::{DirSnapshot, PreparedSnapshot, SnapshotOptions};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Root(PathBuf),
    Snapshot(PathBuf),
    Count(PathBuf),
    Summary {
        path: PathBuf,
        deep: bool,
    },
    S3Root(PathBuf),
    S3Expand(PathBuf),
    S3Head(PathBuf),
    Clipboard(PathBuf),
    Paste(PathBuf, u64),
    /// Versioned preview load; all previews share one superseding panel domain.
    Preview(crate::highlighting::PreviewKey),
    /// Incremental filename index keyed by project root; a new request for the
    /// same root supersedes the previous one through the shared domain slot.
    FilenameIndex(PathBuf),
    /// Literal project content search keyed by project root.
    ContentSearch(PathBuf),
}
impl Target {
    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::Root(path) | Self::Snapshot(path) | Self::Count(path) => path,
            Self::Summary { path, .. } => path,
            Self::S3Root(path) | Self::S3Expand(path) | Self::S3Head(path) => path,
            Self::Clipboard(path) | Self::Paste(path, _) => path,
            Self::Preview(key) => &key.path,
            Self::FilenameIndex(path) | Self::ContentSearch(path) => path,
        }
    }

    pub(crate) fn is_snapshot(&self) -> bool {
        matches!(self, Self::Root(_) | Self::Snapshot(_))
    }
    pub(crate) fn is_summary(&self) -> bool {
        matches!(self, Self::Summary { .. })
    }
    pub(crate) fn is_s3(&self) -> bool {
        matches!(self, Self::S3Root(_) | Self::S3Expand(_) | Self::S3Head(_))
    }
    pub(crate) fn is_preview(&self) -> bool {
        matches!(self, Self::Preview(_))
    }
    fn same_domain(&self, other: &Self) -> bool {
        self == other
            || (self.is_summary() && other.is_summary())
            || (self.is_preview() && other.is_preview())
            || matches!(
                (self, other),
                (Self::S3Head(_), Self::S3Head(_))
                    | (Self::S3Root(_), Self::S3Root(_))
                    | (Self::Clipboard(_), Self::Clipboard(_))
            )
    }
    fn retained_bytes(&self) -> usize {
        match self {
            Self::Root(path) | Self::Snapshot(path) | Self::Count(path) => path.capacity(),
            Self::Summary { path, .. } => path.capacity(),
            Self::S3Root(path) | Self::S3Expand(path) | Self::S3Head(path) => path.capacity(),
            Self::Clipboard(path) | Self::Paste(path, _) => path.capacity(),
            // A preview's backing file is not part of the job envelope: the worker's
            // bounded reader decides how much of it becomes result state. Charging
            // it as job payload would refuse a legitimate preview before the
            // worker can classify it into full or bounded head/tail.
            Self::Preview(key) => key
                .path
                .capacity()
                .saturating_add(std::mem::size_of_val(key)),
            Self::FilenameIndex(path) | Self::ContentSearch(path) => path.capacity(),
        }
    }
}

/// Theme name and resolved colors required to prepare a preview off-thread.
pub(crate) struct PreviewTheme {
    pub(crate) name: String,
    pub(crate) colors: crate::theme::ThemeColors,
}

pub(crate) type ClipboardBackend = Arc<dyn Fn(&str, &dyn Fn() -> bool) -> bool + Send + Sync>;
pub(crate) enum ClipboardJob {
    Copy {
        text: String,
        backend: ClipboardBackend,
    },
    Paste {
        paths: Vec<PathBuf>,
        cut: bool,
        interrupt: Arc<std::sync::atomic::AtomicBool>,
    },
}
pub(crate) struct PasteResult {
    pub(crate) result: crate::event::OperationResult,
    pub(crate) moves: Vec<(PathBuf, PathBuf)>,
    pub(crate) copies: Vec<PathBuf>,
    pub(crate) completed_entries: usize,
}
fn paths_bytes(paths: &Vec<PathBuf>) -> usize {
    paths.iter().fold(
        paths
            .capacity()
            .saturating_mul(std::mem::size_of::<PathBuf>()),
        |bytes, path| bytes.saturating_add(path.capacity()),
    )
}
/// Conservative simultaneous output, undo and diagnostic envelope. Computed
/// before admission and before any filesystem mutation.
pub(crate) fn operation_envelope(paths: &[PathBuf], dest: &Path) -> usize {
    paths.iter().fold(
        8192usize.saturating_add(dest.as_os_str().len()),
        |bytes, path| {
            bytes.saturating_add(4096).saturating_add(
                8usize.saturating_mul(
                    dest.as_os_str()
                        .len()
                        .saturating_add(path.as_os_str().len())
                        .saturating_add(512),
                ),
            )
        },
    )
}

pub(crate) struct NativeJob {
    pub(crate) target: Target,
    pub(crate) max_entries: usize,
    pub(crate) timeout: Duration,
    pub(crate) result_bytes: usize,
    pub(crate) snapshot_options: SnapshotOptions,
    pub(crate) summary_colors: Option<crate::theme::ThemeColors>,
    pub(crate) progress: Option<Arc<SummaryProgress>>,
    pub(crate) s3_profile: Option<String>,
    pub(crate) s3_head_lines: usize,
    pub(crate) clipboard: Option<ClipboardJob>,
    pub(crate) preview_theme: Option<PreviewTheme>,
    /// Filename-index or literal content-search request parameters and cursor.
    pub(crate) search: Option<crate::search::SearchJob>,
}
impl Payload for NativeJob {
    fn payload_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.target.retained_bytes())
            .saturating_add(self.s3_profile.as_ref().map_or(0, String::capacity))
            .saturating_add(match &self.clipboard {
                Some(ClipboardJob::Copy { text, .. }) => text.capacity(),
                Some(ClipboardJob::Paste { paths, .. }) => paths_bytes(paths)
                    .saturating_add(std::mem::size_of::<std::sync::atomic::AtomicBool>()),
                None => 0,
            })
            .saturating_add(self.preview_theme.as_ref().map_or(0, |theme| {
                theme.name.capacity() + std::mem::size_of::<PreviewTheme>()
            }))
            .saturating_add(search_job_bytes(&self.search))
            .saturating_add(
                if self.target.is_summary() || matches!(self.target, Target::Paste(..)) {
                    std::mem::size_of::<SummaryProgress>()
                } else {
                    0
                },
            )
            // A preview job's envelope is its identity plus theme only. The
            // backing file is read by the bounded worker reader, so charging
            // file bytes here would refuse a legitimate preview before the
            // worker can classify it into full or bounded head/tail.
            .saturating_sub(preview_file_bytes(&self.target))
    }
}

/// Bytes of a preview's backing file that must not count against its job
/// envelope: the worker's bounded reader decides how much becomes result state.
fn preview_file_bytes(target: &Target) -> usize {
    match target {
        Target::Preview(key) => key.path.capacity(),
        _ => 0,
    }
}

fn pending_paths_bytes<T>(pending: &std::collections::VecDeque<T>) -> usize {
    pending
        .len()
        .saturating_mul(std::mem::size_of::<T>())
        .min(64 * 1024)
}

fn search_cursor_bytes(cursor: &crate::search::SearchCursor) -> usize {
    let paths = path_bytes_owned(cursor.queue.iter());
    // Each pending entry may be a partially enumerated directory carrying a
    // saved listing position; charge both its buffer and its path bytes.
    let pending = cursor.pending.iter().fold(
        cursor
            .pending
            .len()
            .saturating_mul(std::mem::size_of::<crate::search::DirListing>()),
        |bytes, listing| bytes.saturating_add(listing.dir.capacity()),
    );
    paths.saturating_add(pending).saturating_add(
        cursor
            .visited
            .capacity()
            .saturating_mul(std::mem::size_of::<(u64, u64)>()),
    )
}

fn index_cursor_bytes(cursor: &crate::search::IndexCursor) -> usize {
    let paths = path_bytes_owned(cursor.queue.iter().map(|(path, _, _)| path));
    paths
        .saturating_add(pending_paths_bytes(&cursor.pending))
        .saturating_add(
            cursor
                .visited
                .capacity()
                .saturating_mul(std::mem::size_of::<(u64, u64)>()),
        )
}

fn path_bytes_owned<'a>(paths: impl Iterator<Item = &'a PathBuf>) -> usize {
    paths
        .map(|path| path.capacity())
        .fold(0usize, usize::saturating_add)
}

/// Conservative envelope for a resumable search request. The cursor's pending
/// path buffers dominate; traversal bounds keep this finite.
fn search_job_bytes(job: &Option<crate::search::SearchJob>) -> usize {
    match job {
        None => 0,
        Some(crate::search::SearchJob::Index(request)) => request
            .excludes
            .iter()
            .map(String::capacity)
            .fold(0usize, usize::saturating_add)
            .saturating_add(index_cursor_bytes(&request.cursor)),
        Some(crate::search::SearchJob::Content(request)) => request
            .query
            .text
            .capacity()
            .saturating_add(
                request
                    .excludes
                    .iter()
                    .map(String::capacity)
                    .fold(0usize, usize::saturating_add),
            )
            .saturating_add(search_cursor_bytes(&request.cursor)),
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Statistics {
    pub(crate) files: u64,
    pub(crate) dirs: u64,
    pub(crate) size: u64,
}
/// One coalesced fixed-size value per owned request; no queue or waiting producer.
pub(crate) struct SummaryProgress {
    state: Mutex<(bool, Option<Statistics>)>,
    notify: Arc<tokio::sync::Notify>,
}
impl SummaryProgress {
    pub(crate) fn publish(&self, statistics: Statistics) {
        let mut state = self.state.lock().unwrap();
        if !state.0 {
            state.1 = Some(statistics);
            self.notify.notify_one();
        }
    }
    fn close(&self) {
        *self.state.lock().unwrap() = (true, None);
        self.notify.notify_one();
    }
}
pub(crate) enum NativeOutput {
    Snapshot(PreparedSnapshot),
    Count {
        count: usize,
        complete: bool,
    },
    Progress(Statistics),
    Deep {
        statistics: Statistics,
        complete: bool,
    },
    Shallow(crate::preview_content::ShallowSummary),
    S3Listing(Vec<crate::s3::S3Entry>),
    S3Head(String),
    Preview(crate::highlighting::PreparedPreview),
    Clipboard {
        text: String,
        native: bool,
    },
    OperationProgress(Statistics),
    Paste(PasteResult),
    FilenameIndex(crate::search::IndexBatch),
    ContentSearch(crate::search::ContentBatch),
    Failed(&'static str),
}
impl Payload for NativeOutput {
    fn payload_bytes(&self) -> usize {
        std::mem::size_of::<Self>().saturating_add(match self {
            Self::Snapshot(snapshot) => snapshot.retained_bytes(),
            Self::FilenameIndex(batch) => {
                paths_bytes(&batch.paths).saturating_add(index_cursor_bytes(&batch.cursor))
            }
            Self::ContentSearch(batch) => batch
                .hits
                .iter()
                .fold(0usize, |bytes, hit| {
                    bytes
                        .saturating_add(hit.path.capacity())
                        .saturating_add(hit.excerpt.capacity())
                        .saturating_add(std::mem::size_of::<crate::search::SearchHit>())
                })
                .saturating_add(search_cursor_bytes(&batch.cursor)),
            Self::Shallow(summary) => crate::preview_content::summary_lines_bytes(&summary.lines),
            Self::S3Listing(entries) => entries
                .capacity()
                .saturating_mul(std::mem::size_of::<crate::s3::S3Entry>())
                .saturating_add(
                    entries
                        .iter()
                        .map(|entry| {
                            entry
                                .name
                                .capacity()
                                .saturating_add(entry.modified.capacity())
                        })
                        .fold(0usize, usize::saturating_add),
                ),
            Self::S3Head(text) => text.capacity(),
            Self::Preview(prepared) => prepared.payload_bytes(),
            Self::Clipboard { text, .. } => text.capacity(),
            Self::Paste(paste) => paths_bytes(&paste.result.created_paths)
                .saturating_add(paths_bytes(&paste.result.source_paths))
                .saturating_add(paste.result.dest_dir.capacity())
                .saturating_add(
                    paste
                        .result
                        .errors
                        .capacity()
                        .saturating_mul(std::mem::size_of::<String>()),
                )
                .saturating_add(
                    paste
                        .result
                        .errors
                        .iter()
                        .map(String::capacity)
                        .sum::<usize>(),
                )
                .saturating_add(paths_bytes(&paste.copies))
                .saturating_add(
                    paste
                        .moves
                        .capacity()
                        .saturating_mul(std::mem::size_of::<(PathBuf, PathBuf)>()),
                )
                .saturating_add(
                    paste
                        .moves
                        .iter()
                        .map(|(a, b)| a.capacity().saturating_add(b.capacity()))
                        .sum::<usize>(),
                ),
            _ => 0,
        })
    }
}
pub(crate) struct Delivery {
    pub(crate) target: Target,
    pub(crate) generation: RequestGeneration,
    pub(crate) result: Result<NativeOutput, JobError>,
}
struct Pending {
    target: Target,
    generation: RequestGeneration,
    progress: Option<Arc<SummaryProgress>>,
}
pub(crate) struct AppJobs {
    scheduler: Scheduler<NativeJob, NativeOutput>,
    pending: Vec<Option<Pending>>,
    limits: Limits,
    progress_notify: Arc<tokio::sync::Notify>,
}
impl AppJobs {
    pub(crate) fn new(
        limits: Limits,
        worker: Worker<NativeJob, NativeOutput>,
    ) -> Result<Self, &'static str> {
        let scheduler = Scheduler::new(limits, worker)?;
        Ok(Self {
            scheduler,
            pending: (0..limits.request_domains).map(|_| None).collect(),
            limits,
            progress_notify: Arc::new(tokio::sync::Notify::new()),
        })
    }
    pub(crate) fn has_pending(&self) -> bool {
        self.pending.iter().any(Option::is_some)
    }
    pub(crate) fn limits(&self) -> Limits {
        self.limits
    }
    pub(crate) fn submit(
        &mut self,
        mut job: NativeJob,
    ) -> Result<RequestGeneration, AdmissionError> {
        if job.payload_bytes() > self.limits.job_bytes {
            return Err(AdmissionError::PayloadTooLarge);
        }
        let index = self
            .pending
            .iter()
            .position(|slot| {
                slot.as_ref()
                    .is_some_and(|p| p.target.same_domain(&job.target))
            })
            .or_else(|| self.pending.iter().position(Option::is_none))
            .ok_or(AdmissionError::DomainLimit)?;
        job.result_bytes = job.result_bytes.min(self.limits.result_bytes);
        let target = job.target.clone();
        job.progress =
            (job.target.is_summary() || matches!(job.target, Target::Paste(..))).then(|| {
                Arc::new(SummaryProgress {
                    state: Mutex::new((false, None)),
                    notify: self.progress_notify.clone(),
                })
            });
        let progress = job.progress.clone();
        let generation = self
            .scheduler
            .try_submit(RequestDomain(index as u64), job)
            .map_err(|rejection| rejection.reason)?;
        if let Some(old) = &self.pending[index] {
            if let Some(progress) = &old.progress {
                progress.close();
            }
        }
        self.pending[index] = Some(Pending {
            target,
            generation,
            progress,
        });
        Ok(generation)
    }
    pub(crate) fn cancel(&mut self, target: &Target) -> bool {
        let Some(index) = self
            .pending
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|p| p.target.same_domain(target)))
        else {
            return false;
        };
        let pending = self.pending[index].take().unwrap();
        if let Some(progress) = pending.progress {
            progress.close();
        }
        self.scheduler.cancel(pending.generation)
    }
    pub(crate) fn accepts_progress(&self, generation: RequestGeneration) -> bool {
        self.pending
            .iter()
            .flatten()
            .any(|pending| pending.generation == generation)
    }
    pub(crate) async fn next(&mut self) -> Option<Delivery> {
        loop {
            // Completion wins over progress and retires its fixed-state slot.
            let completion = tokio::select! {
                biased;
                completion = self.scheduler.next_result() => completion?,
                _ = self.progress_notify.notified() => {
                    for pending in self.pending.iter().flatten() {
                        if let Some(progress) = &pending.progress {
                            if let Some(statistics) = progress.state.lock().unwrap().1.take() {
                                return Some(Delivery {
                                    target: pending.target.clone(),
                                    generation: pending.generation,
                                    result: Ok(if matches!(pending.target, Target::Paste(..)) {
                                        NativeOutput::OperationProgress(statistics)
                                    } else { NativeOutput::Progress(statistics) }),
                                });
                            }
                        }
                    }
                    continue;
                }
            };
            let Some(index) = self.pending.iter().position(|slot| {
                slot.as_ref()
                    .is_some_and(|p| p.generation == completion.generation)
            }) else {
                continue;
            };
            let pending = self.pending[index].take().unwrap();
            if let Some(progress) = pending.progress {
                progress.close();
            }
            return Some(Delivery {
                target: pending.target,
                generation: pending.generation,
                result: completion.result,
            });
        }
    }
    pub(crate) fn cancel_under(&mut self, path: &Path) -> Vec<Target> {
        let mut retired = Vec::new();
        for slot in &mut self.pending {
            if slot.as_ref().is_some_and(|pending| {
                !matches!(pending.target, Target::Paste(..))
                    && pending.target.path().starts_with(path)
            }) {
                let pending = slot.take().unwrap();
                if let Some(progress) = pending.progress {
                    progress.close();
                }
                self.scheduler.cancel(pending.generation);
                retired.push(pending.target);
            }
        }
        retired
    }
    pub(crate) async fn shutdown(&mut self) -> Vec<Target> {
        for pending in self.pending.iter().flatten() {
            if let Some(progress) = &pending.progress {
                progress.close();
            }
        }
        self.scheduler.shutdown().await;
        self.pending
            .iter_mut()
            .filter_map(|slot| slot.take().map(|pending| pending.target))
            .collect()
    }
}

pub(crate) fn run(job: NativeJob, cancel: Cancellation) -> NativeOutput {
    if job.clipboard.is_some() {
        return run_clipboard(job, cancel);
    }
    let started = std::time::Instant::now();
    let stopped = || cancel.is_cancelled() || started.elapsed() >= job.timeout;
    if let Some(search) = job.search.as_ref() {
        return match search {
            crate::search::SearchJob::Index(request) => {
                NativeOutput::FilenameIndex(crate::search::run_index(request, &stopped))
            }
            crate::search::SearchJob::Content(request) => {
                NativeOutput::ContentSearch(crate::search::run_content(request, &stopped))
            }
        };
    }
    if let Target::Preview(key) = &job.target {
        let Some(theme) = &job.preview_theme else {
            return NativeOutput::Failed("Preview failed: missing theme options");
        };
        if stopped() {
            return NativeOutput::Failed("Preview cancelled or timed out");
        }
        if stopped() {
            return NativeOutput::Failed("Preview cancelled or timed out");
        }
        let prepared = crate::highlighting::load_file_preview(
            key,
            crate::highlighting::syntax_set(),
            &theme.name,
            &theme.colors,
        );
        // The scheduler bounds `NativeOutput::payload_bytes()`, which adds its own
        // enum overhead on top of the prepared payload.
        let envelope = job
            .result_bytes
            .saturating_sub(std::mem::size_of::<NativeOutput>());
        if crate::background::Payload::payload_bytes(&prepared) > envelope {
            // A full preview that cannot fit the result envelope degrades to a
            // bounded head/tail window that fits, instead of surfacing
            // PayloadTooLarge.
            let fallback = crate::highlighting::budgeted_fallback_preview(
                key,
                crate::highlighting::syntax_set(),
                &theme.name,
                &theme.colors,
                envelope,
            );
            return NativeOutput::Preview(fallback);
        }
        return NativeOutput::Preview(prepared);
    }
    if job.target.is_s3() {
        let Some(path) = crate::s3::S3Path::parse(&job.target.path().to_string_lossy()) else {
            return NativeOutput::Failed("S3 failed: invalid URI");
        };
        let backend = crate::s3::S3Backend::new(&crate::s3::S3Config {
            path: path.clone(),
            profile: job.s3_profile.clone(),
        });
        let result = if matches!(job.target, Target::S3Head(_)) {
            backend
                .head_bounded(&path, job.s3_head_lines, stopped)
                .map(NativeOutput::S3Head)
        } else {
            backend
                .list_bounded(
                    &path,
                    job.result_bytes
                        .saturating_sub(std::mem::size_of::<NativeOutput>()),
                    stopped,
                )
                .map(NativeOutput::S3Listing)
        };
        result.unwrap_or_else(NativeOutput::Failed)
    } else if let Target::Summary { deep, .. } = &job.target {
        if *deep {
            run_deep(&job, stopped)
        } else if let Some(colors) = &job.summary_colors {
            NativeOutput::Shallow(
                crate::preview_content::load_directory_summary_shallow_budgeted(
                    job.target.path(),
                    colors,
                    job.max_entries,
                    job.result_bytes
                        .saturating_sub(std::mem::size_of::<NativeOutput>()),
                    stopped,
                ),
            )
        } else {
            NativeOutput::Failed("Directory summary failed: missing formatting options")
        }
    } else if job.target.is_snapshot() {
        let bytes = job
            .result_bytes
            .saturating_sub(std::mem::size_of::<NativeOutput>());
        // Reserve half the result envelope for prepared nodes and their nested
        // paths/names. No consumer-side fallback/re-enumeration on refusal.
        match DirSnapshot::collect_budgeted(job.target.path(), job.max_entries, bytes / 2, stopped)
        {
            Ok(snapshot) => NativeOutput::Snapshot(snapshot.prepare(
                job.target.path(),
                job.snapshot_options,
                bytes,
                stopped,
            )),
            Err(_) => NativeOutput::Failed("Directory snapshot failed (I/O)"),
        }
    } else {
        if stopped() {
            return NativeOutput::Count {
                count: 0,
                complete: false,
            };
        }
        let entries = match std::fs::read_dir(job.target.path()) {
            Ok(entries) => entries,
            Err(_) => return NativeOutput::Failed("Directory count failed (I/O)"),
        };
        let mut count = 0;
        for entry in entries {
            if stopped() || count == job.max_entries {
                return NativeOutput::Count {
                    count,
                    complete: false,
                };
            }
            if entry.is_err() {
                return NativeOutput::Count {
                    count,
                    complete: false,
                };
            }
            count += 1;
        }
        NativeOutput::Count {
            count,
            complete: true,
        }
    }
}

fn run_clipboard(mut job: NativeJob, cancel: Cancellation) -> NativeOutput {
    let started = std::time::Instant::now();
    let stopped = || cancel.is_cancelled() || started.elapsed() >= job.timeout;
    match job.clipboard.take().unwrap() {
        ClipboardJob::Copy { text, backend } => {
            if stopped() {
                return NativeOutput::Failed("Clipboard copy incomplete: cancelled/deadline");
            }
            let native = backend(&text, &stopped);
            NativeOutput::Clipboard { text, native }
        }
        ClipboardJob::Paste {
            paths,
            cut,
            interrupt,
        } => {
            let dest = job.target.path().to_path_buf();
            if operation_envelope(&paths, &dest) > job.result_bytes {
                return NativeOutput::Failed("Operation result budget refused before mutation");
            }
            let total = paths.len();
            let mut output = PasteResult {
                result: crate::event::OperationResult {
                    success_count: 0,
                    errors: Vec::with_capacity(total + 1),
                    created_paths: Vec::with_capacity(total),
                    source_paths: Vec::with_capacity(total),
                    dest_dir: dest,
                    was_cut: cut,
                },
                moves: Vec::with_capacity(total),
                copies: Vec::with_capacity(total),
                completed_entries: 0,
            };
            let stopped = || stopped() || interrupt.load(std::sync::atomic::Ordering::SeqCst);
            for (i, src) in paths.iter().enumerate() {
                if stopped() {
                    output
                        .result
                        .errors
                        .push("Operation interrupted/deadline; partial results preserved".into());
                    break;
                }
                if let Some(progress) = &job.progress {
                    progress.publish(Statistics {
                        files: (i + 1) as u64,
                        dirs: total as u64,
                        size: 0,
                    });
                }
                let receipt = crate::fs::operations::transfer_with_policy(
                    src,
                    &output.result.dest_dir,
                    cut,
                    crate::fs::operations::TransferPolicy {
                        stopped: &stopped,
                        max_entries: job.max_entries.saturating_sub(output.completed_entries),
                    },
                );
                output.completed_entries = output
                    .completed_entries
                    .saturating_add(receipt.completed_entries);
                if let Some(dest) = receipt.destination {
                    output.result.created_paths.push(dest.clone());
                    output.result.source_paths.push(src.clone());
                    if receipt.moved {
                        output.moves.push((src.clone(), dest));
                    } else {
                        output.copies.push(dest);
                    }
                }
                if receipt.result.is_ok() {
                    output.result.success_count += 1;
                } else {
                    output.result.errors.push(
                        "Operation failed/interrupted/budget refused; partial destination retained"
                            .into(),
                    );
                }
            }
            NativeOutput::Paste(output)
        }
    }
}

/// Internal traversal bounds, in addition to the existing entry/deadline policy:
/// 2048 queued paths, 4096 visited directories, 4096 bytes/path, 8 MiB path stack.
/// Native syscalls are cooperative boundaries, never forcibly preempted.
fn run_deep(job: &NativeJob, stopped: impl Fn() -> bool) -> NativeOutput {
    const STACK: usize = 2048;
    const VISITED: usize = 4096;
    const PATH_BYTES: usize = 4096;
    const STACK_BYTES: usize = 8 * 1024 * 1024;
    let path = job.target.path();
    if path.as_os_str().len() > PATH_BYTES {
        return NativeOutput::Failed("Directory summary incomplete: path budget");
    }
    let mut statistics = Statistics::default();
    if stopped() {
        return NativeOutput::Deep {
            statistics,
            complete: false,
        };
    }
    #[cfg(unix)]
    let mut visited = std::collections::HashSet::with_capacity(VISITED);
    #[cfg(not(unix))]
    let mut visited = std::collections::HashSet::with_capacity(VISITED);
    macro_rules! visit {
        ($path:expr, $meta:expr) => {{
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                visited.insert(($meta.dev(), $meta.ino()))
            }
            #[cfg(not(unix))]
            {
                let canonical = match std::fs::canonicalize($path) {
                    Ok(path) if path.capacity() <= PATH_BYTES => path,
                    _ => {
                        return NativeOutput::Deep {
                            statistics,
                            complete: false,
                        }
                    }
                };
                visited.insert(canonical)
            }
        }};
    }
    let root_meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return NativeOutput::Failed("Directory summary failed (I/O)"),
    };
    visit!(path, root_meta);
    let mut stack = Vec::with_capacity(STACK);
    stack.push(path.to_path_buf());
    let mut stack_bytes = stack[0].capacity();
    let mut scanned = 0usize;
    let mut complete = true;
    'scan: while let Some(path) = stack.pop() {
        stack_bytes -= path.capacity();
        if stopped() {
            complete = false;
            break;
        }
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        for entry in entries {
            if stopped() || scanned == job.max_entries {
                complete = false;
                break 'scan;
            }
            scanned += 1;
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    complete = false;
                    continue;
                }
            };
            if stopped() {
                complete = false;
                break 'scan;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => {
                    complete = false;
                    continue;
                }
            };
            if metadata.is_dir() {
                if stopped() {
                    complete = false;
                    break 'scan;
                }
                let name = entry.file_name();
                if name
                    .len()
                    .saturating_add(path.as_os_str().len())
                    .saturating_add(2)
                    > PATH_BYTES
                    || stack.len() == STACK
                    || visited.len() == VISITED
                {
                    complete = false;
                    break 'scan;
                }
                let child = entry.path();
                if child.capacity() > STACK_BYTES.saturating_sub(stack_bytes) {
                    complete = false;
                    break 'scan;
                }
                if visit!(&child, metadata) {
                    statistics.dirs = statistics.dirs.saturating_add(1);
                    stack_bytes += child.capacity();
                    stack.push(child);
                }
            } else {
                statistics.files = statistics.files.saturating_add(1);
                statistics.size = statistics.size.saturating_add(metadata.len());
            }
            if scanned.is_multiple_of(1000) {
                if let Some(progress) = &job.progress {
                    progress.publish(statistics);
                }
            }
        }
    }
    NativeOutput::Deep {
        statistics,
        complete,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::tree::SortBy;

    fn options() -> SnapshotOptions {
        SnapshotOptions {
            sort_by: SortBy::Name,
            dirs_first: true,
            page_size: 100,
            child_depth: 1,
        }
    }

    fn search_job(target: Target, search: crate::search::SearchJob) -> NativeJob {
        NativeJob {
            target,
            max_entries: 16,
            timeout: Duration::from_secs(1),
            result_bytes: 1024 * 1024,
            snapshot_options: options(),
            summary_colors: None,
            progress: None,
            s3_profile: None,
            s3_head_lines: 0,
            clipboard: None,
            preview_theme: None,
            search: Some(search),
        }
    }

    #[test]
    fn search_targets_group_by_root_and_variant_only() {
        let a = Target::ContentSearch(PathBuf::from("/root"));
        let same = Target::ContentSearch(PathBuf::from("/root"));
        let other = Target::ContentSearch(PathBuf::from("/other"));
        let index = Target::FilenameIndex(PathBuf::from("/root"));
        assert!(a.same_domain(&same));
        assert!(!a.same_domain(&other));
        assert!(!a.same_domain(&index));
        assert_eq!(a.path(), Path::new("/root"));
    }

    #[tokio::test]
    async fn search_jobs_are_admitted_then_superseded_or_cancelled_by_root() {
        let worker = Worker::Blocking(Arc::new(|_: NativeJob, _| {
            NativeOutput::Failed("fake worker")
        }));
        let mut jobs = AppJobs::new(Limits::default(), worker).unwrap();
        let request = || {
            crate::search::SearchJob::Content(crate::search::ContentRequest::new(
                Path::new("/root"),
                crate::search::SearchQuery::new("needle", false),
                vec![".git".to_string()],
                crate::search::SearchLimits::default(),
            ))
        };
        jobs.submit(search_job(
            Target::ContentSearch(PathBuf::from("/root")),
            request(),
        ))
        .unwrap();
        assert_eq!(
            jobs.pending
                .iter()
                .flatten()
                .filter(|pending| matches!(pending.target, Target::ContentSearch(_)))
                .count(),
            1
        );
        // A second query for the same root reuses the same domain slot.
        jobs.submit(search_job(
            Target::ContentSearch(PathBuf::from("/root")),
            request(),
        ))
        .unwrap();
        assert_eq!(
            jobs.pending
                .iter()
                .flatten()
                .filter(|pending| matches!(pending.target, Target::ContentSearch(_)))
                .count(),
            1
        );
        // Filename indexing for the same root is an independent domain.
        jobs.submit(search_job(
            Target::FilenameIndex(PathBuf::from("/root")),
            crate::search::SearchJob::Index(crate::search::IndexRequest {
                excludes: vec![".git".to_string()],
                max_entries: 16,
                batch_entries: 4,
                max_depth: 4,
                max_pending: 64,
                cursor: crate::search::IndexCursor::new(Path::new("/root")),
            }),
        ))
        .unwrap();
        assert!(jobs.cancel(&Target::ContentSearch(PathBuf::from("/root"))));
        assert!(!jobs.cancel(&Target::ContentSearch(PathBuf::from("/root"))));
        assert!(jobs.cancel(&Target::FilenameIndex(PathBuf::from("/root"))));
        jobs.shutdown().await;
    }

    #[test]
    fn search_output_payload_counts_hits_and_cursor_paths() {
        let mut cursor = crate::search::SearchCursor::new(Path::new("/root"));
        cursor
            .queue
            .push_back(PathBuf::from("/root/some/reasonably/long/file.txt"));
        let batch = crate::search::ContentBatch {
            hits: vec![crate::search::SearchHit {
                path: PathBuf::from("/root/some/reasonably/long/file.txt"),
                line: 1,
                byte: 0,
                column: 0,
                excerpt: "x".repeat(200),
            }],
            cursor,
        };
        let empty = crate::search::ContentBatch {
            hits: Vec::new(),
            cursor: crate::search::SearchCursor::new(Path::new("/root")),
        };
        let output = NativeOutput::ContentSearch(batch);
        let baseline = NativeOutput::ContentSearch(empty).payload_bytes();
        assert!(output.payload_bytes() > baseline);
    }

    #[test]
    fn index_output_payload_counts_paths() {
        let batch = crate::search::IndexBatch {
            paths: vec![PathBuf::from("/root/a/long/path/entry.txt")],
            cursor: crate::search::IndexCursor::new(Path::new("/root")),
        };
        assert!(NativeOutput::FilenameIndex(batch).payload_bytes() > 0);
    }
}
