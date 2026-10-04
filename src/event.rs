use std::collections::VecDeque;
use std::mem::size_of;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crossterm::event::{self, Event as CrosstermEvent, KeyEvent, MouseEvent};
use tokio::sync::Notify;

use crate::error::Result;

use crate::fs::tree::DirSnapshot;

/// Progress update from an async file operation.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Compatibility envelope; migrated operations drain typed results.
pub struct ProgressUpdate {
    /// Current file being processed.
    pub current_file: String,
    /// Index of current item (1-based).
    pub current: usize,
    /// Total number of items.
    pub total: usize,
}

/// Result of a completed async operation.
#[derive(Debug)]
pub struct OperationResult {
    /// Number of successfully processed items.
    pub success_count: usize,
    /// Error messages, if any.
    pub errors: Vec<String>,
    /// Paths that were created (for undo support).
    #[allow(dead_code)]
    pub created_paths: Vec<PathBuf>,
    /// Source paths that were involved (for tree refresh).
    pub source_paths: Vec<PathBuf>,
    /// Destination directory.
    pub dest_dir: PathBuf,
    /// Whether this was a cut (move) operation.
    pub was_cut: bool,
}

/// Application events.
#[derive(Debug)]
pub enum Event {
    /// A key press event.
    Key(KeyEvent),
    /// A mouse event.
    Mouse(MouseEvent),
    /// Literal bracketed-paste text, never normal-mode key commands.
    Paste(String),
    /// Terminal resize event.
    #[allow(dead_code)]
    Resize(u16, u16),
    /// Progress update from an async file operation.
    #[allow(dead_code)]
    Progress(ProgressUpdate),
    /// Async file operation completed.
    #[allow(dead_code)]
    OperationComplete(OperationResult),
    /// Filesystem change detected by watcher.
    FsChange(Vec<PathBuf>),
    /// Raw output from the embedded terminal PTY.
    TerminalOutput {
        session: u64,
        data: Vec<u8>,
    },
    /// Rejected whole payload; never a truncated edit/output/completion.
    TransportRejected(&'static str),
    TerminalClosed {
        session: u64,
    },
    /// One ordered outcome per accepted stdin packet. Fixed-size; session
    /// identity prevents an old writer failure affecting a restarted terminal.
    TerminalInputComplete {
        session: u64,
        sequence: u64,
        outcome: crate::terminal::pty::InputOutcome,
    },
    /// Async directory snapshot collection completed.
    #[allow(dead_code)]
    DirScanComplete {
        path: PathBuf,
        snapshot: DirSnapshot,
    },
    /// Async directory child count completed.
    #[allow(dead_code)]
    DirCountComplete {
        path: PathBuf,
        count: usize,
    },
    /// Async directory summary update (streaming).
    #[allow(dead_code)]
    DirSummaryUpdate {
        path: PathBuf,
        files: u64,
        dirs: u64,
        size: u64,
        done: bool,
    },
    /// Shallow (depth-1) directory summary completed.
    /// Legacy untargeted envelope: ignored by main after summary migration.
    #[allow(dead_code)]
    ShallowDirSummary {
        path: PathBuf,
        lines: Vec<ratatui::text::Line<'static>>,
        total: usize,
    },
    /// Async system clipboard copy completed.
    #[allow(dead_code)]
    ClipboardCopyComplete(String),
    /// Native clipboard failed — show text for manual browser copy.
    /// Legacy untargeted envelope, ignored after typed clipboard migration.
    #[allow(dead_code)]
    ShowCopyableText(String),
    /// An outcome from a language-server session. `language` + `generation`
    /// tag the emitting process so restarted servers can't interleave stale
    /// results with the live generation.
    #[allow(dead_code)]
    Lsp {
        language: String,
        generation: u64,
        event: crate::lsp::client::ClientEvent,
    },
    /// Async S3 directory listing completed.
    /// Legacy untargeted envelope, ignored by main after pool migration.
    #[allow(dead_code)]
    S3ListingComplete {
        s3_uri: String,
        entries: Vec<crate::s3::S3Entry>,
    },
    /// Async S3 head preview streaming completed.
    /// Legacy untargeted envelope, ignored by main after pool migration.
    #[allow(dead_code)]
    S3HeadComplete {
        s3_uri: String,
        content: std::result::Result<String, String>,
    },
    /// Filesystem watcher initialization failed.
    WatcherInitFailed(String),
    /// Read-only Git status refresh result, tagged with the generation and
    /// workspace root that produced it so stale results can be rejected.
    GitState(crate::git::GitRefresh),
}

/// Capacities include nested allocation capacity, not merely text length.
#[derive(Debug, Clone, Copy)]
pub struct TransportLimits {
    pub slots: usize,
    pub retained_bytes: usize,
    pub event_bytes: usize,
}

impl Default for TransportLimits {
    fn default() -> Self {
        Self {
            slots: 128,
            retained_bytes: 32 * 1024 * 1024,
            event_bytes: 32 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    Closed,
    TooLarge,
}

struct State {
    queue: VecDeque<(Event, usize)>,
    bytes: usize,
    closed: bool,
    senders: usize,
}

struct Channel {
    state: Mutex<State>,
    changed: Condvar,
    readable: Notify,
    writable: Notify,
    limits: TransportLimits,
}

impl Channel {
    fn admits(&self, state: &State, event: &Event, bytes: usize) -> bool {
        // Reserve 1/8 of slots in queues >=8 for input and completions. Admission
        // priority never reorders anything already accepted into the FIFO.
        let bulk = matches!(
            event,
            Event::TerminalOutput { .. }
                | Event::Progress(_)
                | Event::DirSummaryUpdate { done: false, .. }
                | Event::FsChange(_)
        );
        let reserve = if bulk && self.limits.slots >= 8 {
            self.limits.slots / 8
        } else {
            0
        };
        state.queue.len() < self.limits.slots - reserve
            && bytes <= self.limits.retained_bytes - state.bytes
    }
}

pub struct EventSender {
    channel: Arc<Channel>,
    stopped: Arc<AtomicBool>,
}

pub struct EventReceiver {
    channel: Arc<Channel>,
}

pub fn event_channel(limits: TransportLimits) -> (EventSender, EventReceiver) {
    assert!(
        limits.slots > 0
            && limits.event_bytes >= size_of::<Event>()
            && limits.event_bytes <= limits.retained_bytes,
        "Invalid Event transport limits"
    );
    let channel = Arc::new(Channel {
        state: Mutex::new(State {
            queue: VecDeque::new(),
            bytes: 0,
            closed: false,
            senders: 1,
        }),
        changed: Condvar::new(),
        readable: Notify::new(),
        writable: Notify::new(),
        limits,
    });
    (
        EventSender {
            channel: channel.clone(),
            stopped: Arc::new(AtomicBool::new(false)),
        },
        EventReceiver { channel },
    )
}

impl Clone for EventSender {
    fn clone(&self) -> Self {
        self.channel.state.lock().expect("event lock").senders += 1;
        Self {
            channel: self.channel.clone(),
            stopped: self.stopped.clone(),
        }
    }
}

impl Drop for EventSender {
    fn drop(&mut self) {
        self.channel.state.lock().expect("event lock").senders -= 1;
        self.channel.readable.notify_waiters();
        self.channel.changed.notify_all();
    }
}

impl EventSender {
    /// A locally cancellable producer, sharing the single bounded queue.
    pub fn producer(&self) -> Self {
        let mut sender = self.clone();
        sender.stopped = Arc::new(AtomicBool::new(false));
        sender
    }

    pub fn stop(&self) {
        // Synchronize cancellation with blocking Condvar registration.
        let _state = self.channel.state.lock().expect("event lock");
        self.stopped.store(true, Ordering::Release);
        self.channel.changed.notify_all();
        self.channel.writable.notify_waiters();
    }

    pub fn is_closed(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
            || self.channel.state.lock().expect("event lock").closed
    }

    pub fn max_event_bytes(&self) -> usize {
        self.channel.limits.event_bytes
    }

    fn prepare(&self, event: Event) -> (Event, usize, bool) {
        let bytes = event.retained_bytes();
        let rejected = bytes > self.channel.limits.event_bytes
            || matches!(&event, Event::Paste(text) if text.len() > 1024 * 1024);
        if rejected {
            let reason = if matches!(event, Event::Paste(_)) {
                "Paste exceeds the 1 MiB or transport allocation limit"
            } else {
                "Background event exceeds transport allocation limit"
            };
            (Event::TransportRejected(reason), size_of::<Event>(), true)
        } else {
            (event, bytes, false)
        }
    }

    /// Async producers await pressure; never call this from the event consumer.
    #[allow(dead_code)] // Retained canonical transport API, exercised by its tests.
    pub async fn send(&self, event: Event) -> std::result::Result<(), SendError> {
        let (event, bytes, rejected) = self.prepare(event);
        let mut event = Some(event);
        loop {
            let changed = self.channel.writable.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = self.channel.state.lock().expect("event lock");
                if state.closed || self.stopped.load(Ordering::Acquire) {
                    return Err(SendError::Closed);
                }
                if self
                    .channel
                    .admits(&state, event.as_ref().expect("unsent event"), bytes)
                {
                    state.bytes += bytes;
                    state
                        .queue
                        .push_back((event.take().expect("unsent event"), bytes));
                    self.channel.readable.notify_waiters();
                    return if rejected {
                        Err(SendError::TooLarge)
                    } else {
                        Ok(())
                    };
                }
            }
            changed.await;
        }
    }

    /// Blocking callbacks/readers only; no Tokio/consumer-thread blocking send.
    pub fn blocking_send(&self, event: Event) -> std::result::Result<(), SendError> {
        let (event, bytes, rejected) = self.prepare(event);
        let mut state = self.channel.state.lock().expect("event lock");
        loop {
            if state.closed || self.stopped.load(Ordering::Acquire) {
                return Err(SendError::Closed);
            }
            if self.channel.admits(&state, &event, bytes) {
                state.bytes += bytes;
                state.queue.push_back((event, bytes));
                self.channel.readable.notify_waiters();
                return if rejected {
                    Err(SendError::TooLarge)
                } else {
                    Ok(())
                };
            }
            state = self.channel.changed.wait(state).expect("event lock");
        }
    }

    /// Wait for consumer closure, for owned watcher lifetime (no periodic poll).
    pub fn wait_closed(&self) {
        let mut state = self.channel.state.lock().expect("event lock");
        while !state.closed && !self.stopped.load(Ordering::Acquire) {
            state = self.channel.changed.wait(state).expect("event lock");
        }
    }
}

impl EventReceiver {
    pub fn try_recv(
        &mut self,
    ) -> std::result::Result<Event, tokio::sync::mpsc::error::TryRecvError> {
        use tokio::sync::mpsc::error::TryRecvError;
        let mut state = self.channel.state.lock().expect("event lock");
        if let Some((event, bytes)) = state.queue.pop_front() {
            state.bytes -= bytes;
            self.channel.changed.notify_all();
            self.channel.writable.notify_waiters();
            return Ok(event);
        }
        Err(if state.closed || state.senders == 0 {
            TryRecvError::Disconnected
        } else {
            TryRecvError::Empty
        })
    }

    pub async fn recv(&mut self) -> Option<Event> {
        let channel = self.channel.clone();
        loop {
            let readable = channel.readable.notified();
            tokio::pin!(readable);
            readable.as_mut().enable();
            match self.try_recv() {
                Ok(event) => return Some(event),
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return None,
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
            }
            readable.await;
        }
    }

    pub fn close(&mut self) {
        self.channel.state.lock().expect("event lock").closed = true;
        self.channel.changed.notify_all();
        self.channel.readable.notify_waiters();
        self.channel.writable.notify_waiters();
    }
}

impl Drop for EventReceiver {
    fn drop(&mut self) {
        self.close();
        let mut state = self.channel.state.lock().expect("event lock");
        state.queue.clear();
        state.bytes = 0;
    }
}

impl Event {
    pub fn retained_bytes(&self) -> usize {
        fn paths(paths: &Vec<PathBuf>) -> usize {
            paths.iter().fold(
                paths.capacity().saturating_mul(size_of::<PathBuf>()),
                |n, p| n.saturating_add(p.capacity()),
            )
        }
        fn lines(lines: &Vec<ratatui::text::Line<'static>>) -> usize {
            lines.iter().fold(
                lines
                    .capacity()
                    .saturating_mul(size_of::<ratatui::text::Line<'static>>()),
                |n, line| {
                    line.spans.iter().fold(
                        n.saturating_add(
                            line.spans
                                .capacity()
                                .saturating_mul(size_of::<ratatui::text::Span<'static>>()),
                        ),
                        |n, span| {
                            n.saturating_add(match &span.content {
                                std::borrow::Cow::Owned(text) => text.capacity(),
                                std::borrow::Cow::Borrowed(_) => 0,
                            })
                        },
                    )
                },
            )
        }
        let dynamic = match self {
            Self::Paste(text)
            | Self::ClipboardCopyComplete(text)
            | Self::ShowCopyableText(text)
            | Self::WatcherInitFailed(text) => text.capacity(),
            Self::Progress(p) => p.current_file.capacity(),
            Self::TerminalOutput { data, .. } => data.capacity(),
            Self::FsChange(p) => paths(p),
            Self::OperationComplete(r) => paths(&r.created_paths)
                .saturating_add(paths(&r.source_paths))
                .saturating_add(r.dest_dir.capacity())
                .saturating_add(r.errors.capacity().saturating_mul(size_of::<String>()))
                .saturating_add(
                    r.errors
                        .iter()
                        .fold(0usize, |n, e| n.saturating_add(e.capacity())),
                ),
            Self::DirScanComplete { path, snapshot } => snapshot.entries.iter().fold(
                path.capacity().saturating_add(
                    snapshot
                        .entries
                        .capacity()
                        .saturating_mul(size_of::<crate::fs::tree::SnapshotEntry>()),
                ),
                |n, e| n.saturating_add(e.name.capacity()),
            ),
            Self::DirCountComplete { path, .. } | Self::DirSummaryUpdate { path, .. } => {
                path.capacity()
            }
            Self::ShallowDirSummary { path, lines: l, .. } => {
                path.capacity().saturating_add(lines(l))
            }
            Self::S3ListingComplete { s3_uri, entries } => entries.iter().fold(
                s3_uri.capacity().saturating_add(
                    entries
                        .capacity()
                        .saturating_mul(size_of::<crate::s3::S3Entry>()),
                ),
                |n, e| {
                    n.saturating_add(e.name.capacity())
                        .saturating_add(e.modified.capacity())
                },
            ),
            Self::S3HeadComplete { s3_uri, content } => {
                s3_uri.capacity().saturating_add(match content {
                    Ok(t) | Err(t) => t.capacity(),
                })
            }
            Self::GitState(refresh) => refresh.retained_bytes(),
            _ => 0,
        };
        size_of::<Self>().saturating_add(dynamic)
    }
}

/// Owns the real input thread and canonical bounded receiver.
pub struct EventHandler {
    pub(crate) rx: EventReceiver,
    tx: EventSender,
    input: Option<std::thread::JoinHandle<()>>,
}

fn forward_input(input: CrosstermEvent) -> Option<Event> {
    match input {
        CrosstermEvent::Key(key) => Some(Event::Key(key)),
        CrosstermEvent::Mouse(mouse) => Some(Event::Mouse(mouse)),
        CrosstermEvent::Resize(width, height) => Some(Event::Resize(width, height)),
        CrosstermEvent::Paste(text) => Some(Event::Paste(text)),
        _ => None,
    }
}

impl EventHandler {
    pub fn new(poll_rate: Duration) -> Self {
        Self::with_limits(poll_rate, TransportLimits::default())
    }

    pub fn with_limits(poll_rate: Duration, limits: TransportLimits) -> Self {
        assert!(!poll_rate.is_zero(), "Input poll interval must be nonzero");
        let (tx, rx) = event_channel(limits);
        let input = start_input(tx.clone(), poll_rate, |timeout| {
            if event::poll(timeout)? {
                event::read().map(Some)
            } else {
                Ok(None)
            }
        });
        Self {
            rx,
            tx,
            input: Some(input),
        }
    }

    pub fn sender(&self) -> EventSender {
        self.tx.clone()
    }

    pub async fn next(&mut self) -> Result<Event> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| crate::error::AppError::Terminal("Event channel closed".into()))
    }

    pub async fn shutdown(&mut self) {
        self.rx.close();
        if let Some(input) = self.input.take() {
            let _ = tokio::task::spawn_blocking(move || input.join()).await;
        }
    }
}

impl Drop for EventHandler {
    fn drop(&mut self) {
        self.rx.close();
    }
}

fn start_input(
    sender: EventSender,
    poll_rate: Duration,
    mut poll: impl FnMut(Duration) -> std::io::Result<Option<CrosstermEvent>> + Send + 'static,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !sender.is_closed() {
            match poll(poll_rate) {
                Ok(Some(input)) => {
                    if let Some(input) = forward_input(input) {
                        if sender.blocking_send(input) == Err(SendError::Closed) {
                            break;
                        }
                    }
                }
                Ok(None) => {} // clean idle produces no queued ticks
                Err(_) => {
                    let _ = sender
                        .blocking_send(Event::TransportRejected("Terminal input polling failed"));
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracketed_paste_input_is_not_dropped_or_dispatched_as_keys() {
        let event = forward_input(CrosstermEvent::Paste("q\n\x1b[A".to_string()));
        assert!(matches!(event, Some(Event::Paste(text)) if text == "q\n\x1b[A"));
    }

    #[tokio::test]
    async fn transport_full_queue_backpressures_literal_paste_without_dropping_it() {
        use std::future::poll_fn;
        use std::task::Poll;
        let (tx, mut rx) = event_channel(TransportLimits {
            slots: 1,
            ..TransportLimits::default()
        });
        tx.send(Event::Key(KeyEvent::new(
            crossterm::event::KeyCode::Char('x'),
            crossterm::event::KeyModifiers::NONE,
        )))
        .await
        .unwrap();
        let paste = tx.send(Event::Paste("q\n\x1b[A".into()));
        tokio::pin!(paste);
        poll_fn(|cx| {
            assert!(
                std::future::Future::poll(paste.as_mut(), cx).is_pending(),
                "a full runtime queue must backpressure"
            );
            Poll::Ready(())
        })
        .await;
        assert!(matches!(rx.recv().await, Some(Event::Key(_))));
        paste.await.unwrap();
        assert!(matches!(rx.recv().await, Some(Event::Paste(text)) if text == "q\n\x1b[A"));
    }

    fn key(value: char) -> Event {
        Event::Key(KeyEvent::new(
            crossterm::event::KeyCode::Char(value),
            crossterm::event::KeyModifiers::NONE,
        ))
    }

    #[tokio::test]
    async fn transport_byte_budget_backpressures_before_slot_capacity() {
        use std::task::Poll;
        let (tx, mut rx) = event_channel(TransportLimits {
            slots: 8,
            retained_bytes: 2 * size_of::<Event>(),
            event_bytes: size_of::<Event>(),
        });
        tx.send(key('a')).await.unwrap();
        tx.send(key('b')).await.unwrap();
        let third = tx.send(key('c'));
        tokio::pin!(third);
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(third.as_mut(), cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert!(
            matches!(rx.recv().await, Some(Event::Key(k)) if k.code == crossterm::event::KeyCode::Char('a'))
        );
        third.await.unwrap();
        assert!(
            matches!(rx.recv().await, Some(Event::Key(k)) if k.code == crossterm::event::KeyCode::Char('b'))
        );
        assert!(
            matches!(rx.recv().await, Some(Event::Key(k)) if k.code == crossterm::event::KeyCode::Char('c'))
        );
    }

    #[tokio::test]
    async fn transport_terminal_flood_reserves_admission_for_explicit_input() {
        use std::task::Poll;
        let (tx, mut rx) = event_channel(TransportLimits {
            slots: 8,
            ..Default::default()
        });
        for _ in 0..7 {
            tx.send(Event::TerminalOutput {
                session: 1,
                data: vec![1],
            })
            .await
            .unwrap();
        }
        let flooded = tx.send(Event::TerminalOutput {
            session: 1,
            data: vec![1],
        });
        tokio::pin!(flooded);
        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(flooded.as_mut(), cx).is_pending(),
                "reserve a slot for ordered explicit input"
            );
            Poll::Ready(())
        })
        .await;
        tx.send(key('z')).await.unwrap();
        for _ in 0..7 {
            assert!(matches!(
                rx.recv().await,
                Some(Event::TerminalOutput { .. })
            ));
        }
        assert!(
            matches!(rx.recv().await, Some(Event::Key(k)) if k.code == crossterm::event::KeyCode::Char('z'))
        );
        flooded.await.unwrap();
    }

    #[tokio::test]
    async fn transport_consumer_drop_wakes_async_and_blocking_producers() {
        use std::task::Poll;
        let (tx, rx) = event_channel(TransportLimits {
            slots: 1,
            ..Default::default()
        });
        tx.send(key('a')).await.unwrap();
        let blocked = tx.send(key('b'));
        tokio::pin!(blocked);
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(blocked.as_mut(), cx).is_pending());
            Poll::Ready(())
        })
        .await;
        let callback = tx.clone();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let worker = std::thread::spawn(move || {
            entered.send(()).unwrap();
            callback.blocking_send(key('c'))
        });
        ready.await.unwrap();
        drop(rx);
        assert_eq!(blocked.await, Err(SendError::Closed));
        assert_eq!(
            tokio::task::spawn_blocking(move || worker.join().unwrap())
                .await
                .unwrap(),
            Err(SendError::Closed)
        );
        assert!(tx.is_closed());
        assert_eq!(tx.channel.state.lock().unwrap().bytes, 0);
    }

    #[tokio::test]
    async fn transport_producer_stop_is_local_and_wakes_callback_without_closing_consumer() {
        let (tx, mut rx) = event_channel(TransportLimits {
            slots: 1,
            ..Default::default()
        });
        tx.send(key('a')).await.unwrap();
        let local = tx.producer();
        let callback = local.clone();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let worker = std::thread::spawn(move || {
            entered.send(()).unwrap();
            callback.blocking_send(key('b'))
        });
        ready.await.unwrap();
        local.stop();
        assert_eq!(
            tokio::task::spawn_blocking(move || worker.join().unwrap())
                .await
                .unwrap(),
            Err(SendError::Closed)
        );
        assert!(!tx.is_closed());
        rx.recv().await.unwrap();
        tx.send(key('c')).await.unwrap();
        assert!(
            matches!(rx.recv().await, Some(Event::Key(k)) if k.code == crossterm::event::KeyCode::Char('c'))
        );
    }

    #[tokio::test]
    async fn transport_oversized_paste_and_reserved_capacity_get_truthful_feedback() {
        let (tx, mut rx) = event_channel(Default::default());
        assert_eq!(
            tx.send(Event::Paste("x".repeat(1024 * 1024 + 1))).await,
            Err(SendError::TooLarge)
        );
        assert!(
            matches!(rx.recv().await, Some(Event::TransportRejected(message)) if message.contains("Paste"))
        );
        let (tx, mut rx) = event_channel(TransportLimits {
            event_bytes: size_of::<Event>() + 8,
            ..Default::default()
        });
        let mut text = String::with_capacity(1024);
        text.push('x');
        assert_eq!(
            tx.blocking_send(Event::ShowCopyableText(text)),
            Err(SendError::TooLarge)
        );
        assert!(
            matches!(rx.recv().await, Some(Event::TransportRejected(message)) if message.contains("allocation"))
        );
    }

    #[tokio::test]
    async fn transport_ordered_input_paste_operation_and_terminal_chunks_survive_flood() {
        let (tx, mut rx) = event_channel(TransportLimits {
            slots: 1,
            ..Default::default()
        });
        let producer = tokio::spawn(async move {
            tx.send(key('a')).await.unwrap();
            tx.send(Event::Paste("q\n\x1b[A".into())).await.unwrap();
            tx.send(Event::OperationComplete(OperationResult {
                success_count: 1,
                errors: vec![],
                created_paths: vec!["created".into()],
                source_paths: vec!["source".into()],
                dest_dir: "dest".into(),
                was_cut: false,
            }))
            .await
            .unwrap();
            for _ in 0..1000 {
                tx.send(Event::TerminalOutput {
                    session: 7,
                    data: b"\x1b[31mx\x1b[0m".to_vec(),
                })
                .await
                .unwrap();
            }
            tx.send(Event::TerminalClosed { session: 7 }).await.unwrap();
        });
        assert!(matches!(rx.recv().await, Some(Event::Key(_))));
        assert!(matches!(rx.recv().await, Some(Event::Paste(text)) if text == "q\n\x1b[A"));
        assert!(
            matches!(rx.recv().await, Some(Event::OperationComplete(result)) if result.created_paths == [PathBuf::from("created")])
        );
        for _ in 0..1000 {
            assert!(
                matches!(rx.recv().await, Some(Event::TerminalOutput { session: 7, data }) if data == b"\x1b[31mx\x1b[0m")
            );
            let state = rx.channel.state.lock().unwrap();
            assert!(
                state.queue.len() <= 1 && state.bytes <= TransportLimits::default().retained_bytes
            );
        }
        assert!(matches!(
            rx.recv().await,
            Some(Event::TerminalClosed { session: 7 })
        ));
        producer.await.unwrap();
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn transport_owned_input_shutdown_wakes_full_queue_and_joins_poller() {
        let (tx, mut rx) = event_channel(TransportLimits {
            slots: 1,
            ..Default::default()
        });
        tx.send(key('a')).await.unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let mut entered = Some(entered);
        let input = start_input(tx.clone(), Duration::from_millis(10), move |_| {
            if let Some(entered) = entered.take() {
                entered.send(()).unwrap();
            }
            Ok(Some(CrosstermEvent::Paste("literal".into())))
        });
        ready.await.unwrap();
        rx.close();
        let mut handler = EventHandler {
            rx,
            tx,
            input: Some(input),
        };
        handler.shutdown().await;
        assert!(handler.input.is_none());
        assert!(matches!(handler.next().await, Ok(Event::Key(_))));
        assert!(handler.next().await.is_err());
    }

    #[tokio::test]
    async fn transport_input_error_reports_once_and_idle_focus_events_are_not_keys() {
        assert!(forward_input(CrosstermEvent::FocusGained).is_none());
        assert!(matches!(
            forward_input(CrosstermEvent::Resize(4, 2)),
            Some(Event::Resize(4, 2))
        ));
        let (tx, mut rx) = event_channel(Default::default());
        let input = start_input(tx, Duration::from_millis(10), |_| {
            Err(std::io::Error::other("injected input error"))
        });
        assert!(matches!(rx.recv().await, Some(Event::TransportRejected(_))));
        tokio::task::spawn_blocking(move || input.join().unwrap())
            .await
            .unwrap();
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn transport_callback_lifetime_wait_is_woken_by_consumer_close() {
        let (tx, mut rx) = event_channel(Default::default());
        let worker = std::thread::spawn(move || tx.wait_closed());
        rx.close();
        tokio::task::spawn_blocking(move || worker.join().unwrap())
            .await
            .unwrap();
    }

    #[test]
    fn transport_payload_accounting_includes_nested_owned_capacities() {
        let snapshot = DirSnapshot {
            entries: vec![crate::fs::tree::SnapshotEntry {
                name: "name".into(),
                is_dir: false,
            }],
            skipped_count: 0,
            capped: false,
        };
        let events = [
            Event::DirScanComplete {
                path: "path".into(),
                snapshot,
            },
            Event::DirCountComplete {
                path: "path".into(),
                count: 1,
            },
            Event::DirSummaryUpdate {
                path: "path".into(),
                files: 1,
                dirs: 2,
                size: 3,
                done: true,
            },
            Event::ShallowDirSummary {
                path: "path".into(),
                lines: vec![
                    ratatui::text::Line::raw(String::from("owned")),
                    ratatui::text::Line::raw("borrowed"),
                ],
                total: 2,
            },
            Event::S3ListingComplete {
                s3_uri: "uri".into(),
                entries: vec![crate::s3::S3Entry {
                    name: "name".into(),
                    is_dir: false,
                    size: 0,
                    modified: "date".into(),
                }],
            },
            Event::S3HeadComplete {
                s3_uri: "uri".into(),
                content: Err("error".into()),
            },
            Event::WatcherInitFailed("error".into()),
            Event::ClipboardCopyComplete("copied".into()),
            Event::Progress(ProgressUpdate {
                current_file: "file".into(),
                current: 1,
                total: 1,
            }),
            Event::FsChange(vec!["path".into()]),
        ];
        for event in events {
            assert!(event.retained_bytes() > size_of::<Event>());
        }
    }
}
