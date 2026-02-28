use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event as CrosstermEvent, KeyEvent, MouseEvent};
use tokio::sync::mpsc;

use crate::error::Result;

/// Progress update from an async file operation.
#[derive(Debug, Clone)]
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
    /// A periodic tick for rendering.
    Tick,
    /// Terminal resize event.
    #[allow(dead_code)]
    Resize(u16, u16),
    /// Progress update from an async file operation.
    Progress(ProgressUpdate),
    /// Async file operation completed.
    OperationComplete(OperationResult),
    /// Filesystem change detected by watcher.
    FsChange(Vec<PathBuf>),
}

/// Async event handler that polls crossterm events and forwards them via a channel.
pub struct EventHandler {
    rx: mpsc::UnboundedReceiver<Event>,
    tx: mpsc::UnboundedSender<Event>,
}

impl EventHandler {
    /// Create a new EventHandler with the given tick rate.
    pub fn new(tick_rate: Duration) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let event_tx = tx.clone();

        tokio::spawn(async move {
            loop {
                if event::poll(tick_rate).unwrap_or(false) {
                    match event::read() {
                        Ok(CrosstermEvent::Key(key)) => {
                            if event_tx.send(Event::Key(key)).is_err() {
                                break;
                            }
                        }
                        Ok(CrosstermEvent::Mouse(mouse)) => {
                            if event_tx.send(Event::Mouse(mouse)).is_err() {
                                break;
                            }
                        }
                        Ok(CrosstermEvent::Resize(w, h)) => {
                            if event_tx.send(Event::Resize(w, h)).is_err() {
                                break;
                            }
                        }
                        _ => {}
                    }
                } else if event_tx.send(Event::Tick).is_err() {
                    break;
                }
            }
        });

        Self { rx, tx }
    }

    /// Get a sender clone for async tasks to send progress/completion events.
    pub fn sender(&self) -> mpsc::UnboundedSender<Event> {
        self.tx.clone()
    }

    /// Receive the next event (blocks until available).
    pub async fn next(&mut self) -> Result<Event> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| crate::error::AppError::Terminal("Event channel closed".into()))
    }
}
