mod app;
#[path = "background/app_jobs.rs"]
mod app_jobs;
// Generic core stays unchanged; concrete native jobs are a focused collaborator.
#[allow(dead_code)]
mod background;
mod commands;
mod components;
mod config;
mod editor;
mod error;
mod event;
mod fs;
mod git;
mod handler;
mod highlighting;
mod keymap;
mod preview_content;
// Bounded private recovery snapshots: the record shape, atomic store,
// retention, and restore/clear/disable surface. The startup discovery, the
// throttled capture loop and the recovery commands consume it live here.
mod recovery;
mod s3;
mod search;
mod session;
mod terminal;
mod text;
mod theme;
mod tui;
mod ui;
mod workspace;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;

use crate::app::App;
use crate::config::{
    AppConfig, GeneralConfig, PreviewConfig, TreeConfig, WatcherConfig, WatcherMode,
};
use crate::event::{Event, EventHandler};
use crate::fs::watcher::{FsWatcher, PollingWatcher};
use crate::tui::{install_panic_hook, Tui};

/// Main-loop scheduling seam: the runtime and fake-clock tests use this policy.
struct LoopPolicy {
    redraw: background::DirtyRedraw,
    bytes: Vec<u8>,
    session: Option<u64>,
    batch_bytes: usize,
}

impl LoopPolicy {
    fn new(now: Duration, redraw: background::RedrawLimits, batch_bytes: usize) -> Self {
        assert!(
            (1..=65536).contains(&batch_bytes),
            "Invalid terminal batch limit"
        );
        Self {
            redraw: background::DirtyRedraw::new(now, redraw).expect("redraw limits"),
            bytes: Vec::with_capacity(batch_bytes),
            session: None,
            batch_bytes,
        }
    }

    fn observed(&mut self, event: &Event) {
        if matches!(
            event,
            Event::TerminalInputComplete {
                outcome: terminal::pty::InputOutcome::Written,
                ..
            }
        ) {
            return;
        }
        self.redraw.dirty(!matches!(
            event,
            Event::TerminalOutput { .. }
                | Event::Progress(_)
                | Event::DirSummaryUpdate { done: false, .. }
        ));
    }

    fn output(&mut self, session: u64, bytes: &[u8], mut process: impl FnMut(u64, &[u8])) {
        if self.session != Some(session) {
            self.flush(&mut process);
        }
        self.session = Some(session);
        for chunk in bytes.chunks(self.batch_bytes) {
            if self.bytes.len() + chunk.len() > self.batch_bytes {
                self.flush(&mut process);
            }
            self.bytes.extend_from_slice(chunk);
        }
    }

    fn flush(&mut self, mut process: impl FnMut(u64, &[u8])) {
        if !self.bytes.is_empty() {
            process(self.session.expect("terminal session"), &self.bytes);
            self.bytes.clear();
        }
    }
}

fn next_wait(redraw: Option<Duration>, status: Option<Duration>) -> Option<Duration> {
    match (redraw, status) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The single production computation of the main loop's bounded wait for the
/// next event: the redraw deadline composed with the status-message deadline,
/// the deferred Git-refresh deadline (a Git refresh dropped by the coalescing
/// floor), and the throttled recovery-snapshot deadline.
///
/// `main()` reaches the loop only through [`wait_for_loop`], which calls this;
/// the test `deferred_git_refresh_folds_into_the_timer_wake_and_issues_once`
/// drives the same function, so the fold is directly red-covered. The guard test
/// `main_loop_waits_through_the_folding_helper_only` additionally pins that the
/// loop body has no wait computation of its own.
fn loop_wait(app: &App, now: std::time::Instant, redraw: Option<Duration>) -> Option<Duration> {
    let wait = next_wait(redraw, app.status_wait(now));
    // While recovery has unsaved work pending but throttled, schedule a bounded
    // wake so the last edit is captured even without further input. The snapshot
    // pass itself runs on that timer wake.
    let wait = next_wait(wait, app.recovery_snapshot_wait(now));
    // A Git refresh dropped by the coalescing floor wakes the loop at its
    // deadline so the change converges with no further event.
    next_wait(wait, app.git_refresh_wait(now))
}

fn process_terminal(app: &mut App, session: u64, data: &[u8]) {
    if app
        .terminal_state
        .pty
        .as_ref()
        .is_some_and(|pty| pty.session() == session)
    {
        app.terminal_state.emulator.process(data);
    }
}

fn process_input_completion(
    app: &mut App,
    session: u64,
    sequence: u64,
    outcome: terminal::pty::InputOutcome,
) {
    if app
        .terminal_state
        .pty
        .as_ref()
        .is_some_and(|pty| pty.session() == session)
    {
        if let terminal::pty::InputOutcome::Failed { written, reason } = outcome {
            app.set_status_message(format!(
                "Terminal input #{sequence} failed after {written} bytes: {reason}"
            ));
        }
    }
}

fn process_background(app: &mut App, policy: &mut LoopPolicy, delivery: app_jobs::Delivery) {
    let immediate = !matches!(
        delivery.result,
        Ok(app_jobs::NativeOutput::Progress(_) | app_jobs::NativeOutput::OperationProgress(_))
    );
    policy.flush(|session, bytes| process_terminal(app, session, bytes));
    app.apply_background(delivery);
    policy.redraw.dirty(immediate);
}

enum LoopWake {
    Event(Event),
    Background(app_jobs::Delivery),
    Timer,
}

/// The only way the main loop waits for its next wake. It computes the bounded
/// wait from `app` via [`loop_wait`] and awaits it.
///
/// Folding the wait computation in here (rather than taking a pre-computed
/// `wait` argument) removes the seam that let a caller bypass a term: the loop's
/// only waiting entry point derives the deadline from `app`. A test that drives
/// this function therefore covers the production fold directly, and the guard
/// test below pins that the loop body does not re-compute it.
async fn wait_for_loop(
    app: &mut App,
    next_event: impl std::future::Future<Output = error::Result<Event>>,
    redraw: Option<Duration>,
) -> error::Result<LoopWake> {
    let wait = loop_wait(app, std::time::Instant::now(), redraw);
    wait_for_work(app, next_event, wait).await
}

async fn wait_for_work(
    app: &mut App,
    next_event: impl std::future::Future<Output = error::Result<Event>>,
    wait: Option<Duration>,
) -> error::Result<LoopWake> {
    tokio::select! {
        biased;
        _ = async {
            if let Some(wait) = wait { tokio::time::sleep(wait).await; }
            else { std::future::pending::<()>().await; }
        } => Ok(LoopWake::Timer),
        completion = app.next_background() => match completion {
            Some(delivery) => Ok(LoopWake::Background(delivery)),
            None => Err(std::io::Error::other("Background result stream closed").into()),
        },
        event = next_event => event.map(LoopWake::Event),
    }
}

fn before_mouse(
    app: &mut App,
    event: &Event,
    render: impl FnOnce(&mut App) -> error::Result<()>,
) -> error::Result<()> {
    // Render the latest actual geometry/content/chrome before any cached hit.
    // A geometry-only update would leave menu/tab/gutter hits stale.
    if matches!(event, Event::Mouse(_)) {
        render(app)?;
    }
    Ok(())
}

#[cfg(test)]
mod transport_loop_tests {
    use super::*;

    #[tokio::test]
    async fn app_jobs_wired_main_drains_native_results_while_canonical_event_queue_is_full() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), b"x").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (tx, mut rx) = event::event_channel(event::TransportLimits {
            slots: 1,
            ..Default::default()
        });
        tx.send(Event::Paste("whole ordered input".into()))
            .await
            .unwrap();
        app.spawn_initial_load(&tx);
        // Main's actual selection seam: deliberately do NOT drain this full
        // canonical FIFO while awaiting the native completion. No Event relay.
        let wake = tokio::time::timeout(
            Duration::from_secs(2),
            wait_for_work(
                &mut app,
                std::future::pending::<error::Result<Event>>(),
                None,
            ),
        )
        .await;
        if let Ok(Ok(LoopWake::Background(delivery))) = wake {
            app.apply_background(delivery);
        } else {
            rx.close();
            app.shutdown_background().await;
            panic!("wired scheduler result must not wait for its own Event consumer");
        }
        app.shutdown_background().await;
        assert!(!app.tree_state.root.is_loading);
        assert_eq!(
            app.tree_state.root.children.as_ref().unwrap()[0].name,
            "file.txt"
        );
        assert!(matches!(rx.try_recv(), Ok(Event::Paste(text)) if text == "whole ordered input"));
        let timer = wait_for_work(
            &mut app,
            std::future::pending::<error::Result<Event>>(),
            Some(Duration::ZERO),
        )
        .await
        .unwrap();
        assert!(matches!(timer, LoopWake::Timer));
        let event = wait_for_work(&mut app, async { Ok(Event::Paste("literal".into())) }, None)
            .await
            .unwrap();
        assert!(matches!(event, LoopWake::Event(Event::Paste(text)) if text == "literal"));
    }

    /// P2-A at the main-loop seam: a Git refresh dropped by the coalescing floor
    /// folds into the production wait via `loop_wait`, and the bounded timer wake
    /// issues exactly one deferred follow-up refresh -- with no filesystem event
    /// and no further request.
    ///
    /// This drives the *production* fold function `loop_wait`, so deleting the
    /// `git_refresh_wait` term from `loop_wait` (what `main()` now calls) makes
    /// this test fail; it no longer re-composes the wait locally.
    ///
    /// The `App::new` status message (`status_wait` supplies a ~4 s deadline)
    /// would otherwise be the sub-750 ms term and mask the Git deadline, so it is
    /// expired first: the assertion is about the Git deadline, not a stray status
    /// message.
    #[tokio::test]
    async fn deferred_git_refresh_folds_into_the_timer_wake_and_issues_once() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        let mut app = App::new(repo.path(), AppConfig::default()).unwrap();
        let (tx, _rx) = event::event_channel(Default::default());
        app.event_tx = Some(tx);

        // P3: neutralize any status message so `status_wait` cannot supply the
        // sub-750 ms wait and confound the Git-deadline assertion. Force one to
        // exist, then expire it, so the test would fail loudly if a status
        // deadline (not the Git deadline) were driving the assertion.
        app.set_status_message("fold-sensitive test status".to_string());
        assert!(app.status_wait(std::time::Instant::now()).is_some());
        assert!(
            app.expire_status(std::time::Instant::now() + Duration::from_secs(5)),
            "the status message must be clearable for this assertion"
        );
        assert!(app.status_wait(std::time::Instant::now()).is_none());

        // Startup refresh runs; the immediate second request is coalesced and
        // records a deferred deadline.
        app.request_git_refresh();
        app.request_git_refresh();
        assert_eq!(app.git.issued(), 1);

        // The production wait computation (what `main()` calls) must include the
        // deferred Git deadline. This drives `loop_wait`, the exact expression
        // `wait_for_loop`/`main()` uses; removing the git term there makes both
        // the assertion and the wake below fail (no deadline => no timer wake).
        let wait = loop_wait(&app, std::time::Instant::now(), None);
        assert!(
            wait.is_some_and(|d| d <= Duration::from_millis(750)),
            "the production loop wait must fold in the deferred Git deadline: {wait:?}"
        );
        // Drive the very function the loop awaits, with no redraw deadline so the
        // only source of a timer wake is the folded Git deadline.
        let wake = wait_for_loop(
            &mut app,
            std::future::pending::<error::Result<Event>>(),
            None,
        )
        .await
        .unwrap();
        assert!(matches!(wake, LoopWake::Timer));

        // The timer branch issues exactly one deferred refresh.
        assert!(
            app.issue_deferred_git_refresh(std::time::Instant::now())
                .is_some(),
            "the timer wake must issue the deferred refresh"
        );
        assert_eq!(app.git.issued(), 2);
        // No repeat: a further timer wake issues nothing.
        assert!(app
            .issue_deferred_git_refresh(std::time::Instant::now())
            .is_none());
        assert_eq!(app.git.issued(), 2);
        app.shutdown_background().await;
    }

    /// Structural guard for P2-A: the production loop must wait only through the
    /// folding helper `wait_for_loop`, never by computing its own wait and
    /// calling the raw `wait_for_work`. Without this, deleting the fold from the
    /// loop (the reviewer's defeat) would be invisible to the behavioural test,
    /// which can only observe the helper it drives.
    #[test]
    fn main_loop_waits_through_the_folding_helper_only() {
        let source = include_str!("main.rs");
        // Locate the production loop by its unique tail marker (the loop is the
        // last thing before `Ok(())`/`.await;`); scanning from the end avoids the
        // test module's own source text, which contains the same identifiers.
        let tail_marker = "\n    .await;";
        let tail_start = source
            .rfind(tail_marker)
            .expect("the awaited loop result must exist");
        let production_tail = &source[..tail_start];
        let loop_start = production_tail
            .rfind("loop {")
            .expect("the production loop must exist");
        let loop_end = production_tail[loop_start..]
            .find("if app.should_quit {")
            .expect("the loop body must end at should_quit");
        let loop_body = &production_tail[loop_start..loop_start + loop_end];
        assert!(
            loop_body.contains("wait_for_loop("),
            "the production loop must await the folding helper"
        );
        assert!(
            !loop_body.contains("wait_for_work("),
            "the production loop must not call the raw wait helper"
        );
        assert!(
            !loop_body.contains("git_refresh_wait(")
                && !loop_body.contains("next_wait(")
                && !loop_body.contains("status_wait(")
                && !loop_body.contains("recovery_snapshot_wait("),
            "the production loop must not re-compute its wait outside loop_wait"
        );
    }

    /// P2-A: with nothing deferred, the Git wait is `None`, so the loop adds no
    /// idle wake-up and no per-frame spawn.
    #[tokio::test]
    async fn git_refresh_wait_is_none_when_nothing_was_deferred() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        let mut app = App::new(repo.path(), AppConfig::default()).unwrap();
        let (tx, _rx) = event::event_channel(Default::default());
        app.event_tx = Some(tx);

        assert!(app.git_refresh_wait(std::time::Instant::now()).is_none());
        app.request_git_refresh();
        // A single request issues immediately and defers nothing.
        assert_eq!(app.git.issued(), 1);
        assert!(app.git_refresh_wait(std::time::Instant::now()).is_none());
        assert!(app
            .issue_deferred_git_refresh(std::time::Instant::now())
            .is_none());
        assert_eq!(app.git.issued(), 1);
        app.shutdown_background().await;
    }

    /// Phase 6 Task 5 composite checkpoint: a bounded canonical transport under
    /// a terminal-output flood backpressures the producer without dropping
    /// explicit input, while native job results still drain through the real
    /// main-loop seam (never waiting on the full event queue).
    #[tokio::test]
    async fn checkpoint_bounded_transport_survives_flood_while_native_work_drains() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("visible.txt"), b"x").unwrap();
        let mut app = App::new(dir.path(), AppConfig::default()).unwrap();
        let (tx, mut rx) = event::event_channel(event::TransportLimits {
            slots: 2,
            ..Default::default()
        });
        tx.send(Event::Paste("checkpoint-ordered".into()))
            .await
            .unwrap();
        app.spawn_initial_load(&tx);
        // Flood terminal output; once the bounded queue is full the producer
        // must backpressure rather than drop or deadlock.
        let flood_tx = tx.clone();
        let flood = async move {
            for byte in 0..64u8 {
                if flood_tx
                    .send(Event::TerminalOutput {
                        session: 1,
                        data: vec![byte],
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        };
        tokio::pin!(flood);
        let mut applied = 0usize;
        let outcome = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                tokio::select! {
                    _ = &mut flood, if applied == 0 => {}
                    wake = wait_for_work(
                        &mut app,
                        std::future::pending::<error::Result<Event>>(),
                        None,
                    ) => {
                        if let Ok(LoopWake::Background(delivery)) = wake {
                            app.apply_background(delivery);
                            applied += 1;
                        }
                    }
                }
                if applied > 0 && !app.tree_state.root.is_loading {
                    break;
                }
            }
        })
        .await;
        assert!(
            outcome.is_ok(),
            "native drainage deadlocked behind the bounded terminal flood"
        );
        assert!(applied > 0);
        assert_eq!(
            app.tree_state.root.children.as_ref().unwrap()[0].name,
            "visible.txt"
        );
        assert!(
            matches!(rx.try_recv(), Ok(Event::Paste(text)) if text == "checkpoint-ordered"),
            "explicit ordered input must survive the terminal-output flood"
        );
        rx.close();
        app.shutdown_background().await;
    }

    #[test]
    fn transport_stdin_success_ack_is_clean_idle_but_failure_feedback_is_immediate() {
        let mut policy = LoopPolicy::new(Duration::ZERO, background::RedrawLimits::default(), 8192);
        policy.redraw.drawn(Duration::ZERO);
        policy.observed(&Event::TerminalInputComplete {
            session: 1,
            sequence: 1,
            outcome: terminal::pty::InputOutcome::Written,
        });
        assert_eq!(
            policy.redraw.wait(Duration::ZERO),
            None,
            "success ack has no changed pixels"
        );
        policy.observed(&Event::TerminalInputComplete {
            session: 1,
            sequence: 2,
            outcome: terminal::pty::InputOutcome::Failed {
                written: 0,
                reason: "injected",
            },
        });
        assert_eq!(policy.redraw.wait(Duration::ZERO), Some(Duration::ZERO));
    }

    #[test]
    fn transport_loop_batches_terminal_output_but_not_explicit_input() {
        let mut policy = LoopPolicy::new(Duration::ZERO, background::RedrawLimits::default(), 8192);
        policy.redraw.drawn(Duration::ZERO);
        let mut output = Vec::new();
        for bytes in [
            b"\x1b[".as_slice(),
            b"31mred".as_slice(),
            b"\x1b[0m".as_slice(),
        ] {
            let event = Event::TerminalOutput {
                session: 1,
                data: bytes.to_vec(),
            };
            policy.observed(&event);
            policy.output(1, bytes, |_, b| output.push(b.to_vec()));
        }
        assert_eq!(
            policy.redraw.wait(Duration::from_millis(1)),
            Some(Duration::from_millis(15))
        );
        assert!(output.is_empty(), "do not process/draw once per PTY chunk");
        policy.flush(|_, b| output.push(b.to_vec()));
        assert_eq!(output, [b"\x1b[31mred\x1b[0m".to_vec()]);
        policy.observed(&Event::Paste("literal".into()));
        assert_eq!(
            policy.redraw.wait(Duration::from_millis(1)),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn transport_loop_budget_idle_and_status_deadline_are_injected_not_periodic_draws() {
        let mut policy = LoopPolicy::new(
            Duration::ZERO,
            background::RedrawLimits {
                frame_interval: Duration::from_millis(10),
                event_budget: 3,
            },
            16,
        );
        policy.redraw.drawn(Duration::ZERO);
        assert_eq!(next_wait(policy.redraw.wait(Duration::ZERO), None), None);
        assert_eq!(
            next_wait(None, Some(Duration::from_secs(4))),
            Some(Duration::from_secs(4))
        );
        for _ in 0..2 {
            policy.observed(&Event::TerminalOutput {
                session: 1,
                data: vec![1],
            });
            assert_eq!(
                policy.redraw.wait(Duration::from_millis(1)),
                Some(Duration::from_millis(9))
            );
        }
        policy.observed(&Event::TerminalOutput {
            session: 1,
            data: vec![1],
        });
        assert_eq!(
            policy.redraw.wait(Duration::from_millis(1)),
            Some(Duration::ZERO)
        );
        assert_eq!(
            next_wait(
                Some(Duration::from_millis(9)),
                Some(Duration::from_millis(2))
            ),
            Some(Duration::from_millis(2))
        );
        policy.redraw.drawn(Duration::from_millis(1));
        assert_eq!(
            next_wait(policy.redraw.wait(Duration::from_millis(1)), None),
            None
        );
    }

    #[test]
    fn transport_loop_output_cap_and_session_changes_preserve_every_escape_byte() {
        let mut policy = LoopPolicy::new(Duration::ZERO, background::RedrawLimits::default(), 4);
        let mut chunks = Vec::new();
        policy.output(1, b"\x1b[31mA", |s, b| chunks.push((s, b.to_vec())));
        policy.output(2, b"\x1b[0mB", |s, b| chunks.push((s, b.to_vec())));
        policy.flush(|s, b| chunks.push((s, b.to_vec())));
        assert!(chunks.iter().all(|(_, b)| b.len() <= 4));
        assert_eq!(
            chunks
                .iter()
                .filter(|(s, _)| *s == 1)
                .flat_map(|(_, b)| b.iter().copied())
                .collect::<Vec<_>>(),
            b"\x1b[31mA"
        );
        assert_eq!(
            chunks
                .iter()
                .filter(|(s, _)| *s == 2)
                .flat_map(|(_, b)| b.iter().copied())
                .collect::<Vec<_>>(),
            b"\x1b[0mB"
        );
    }

    #[test]
    fn transport_before_mouse_refreshes_tiny_resize_menu_hits_without_losing_origin() {
        use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
        use ratatui::{backend::TestBackend, layout::Rect, Terminal};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("a.txt");
        std::fs::write(&path, b"original\nsecond").unwrap();
        let mut config = AppConfig::default();
        config.terminal.enabled = Some(false);
        let mut app = App::new(directory.path(), config).unwrap();
        app.open_document_path(&path, true);
        app.workspace.focus.panel = crate::app::FocusedPanel::Editor;
        let origin = app.workspace.documents.active_id();
        app.open_command_menu();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| ui::render(&mut app, f)).unwrap();
        let mouse = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        for (width, height) in [(12, 4), (1, 1), (80, 24)] {
            terminal.backend_mut().resize(width, height);
            terminal.resize(Rect::new(0, 0, width, height)).unwrap();
            before_mouse(&mut app, &mouse, |app| {
                terminal.draw(|f| ui::render(app, f))?;
                Ok(())
            })
            .unwrap();
            assert_eq!(app.workspace_area, Some(Rect::new(0, 0, width, height)));
            let menu = app.command_menu.as_ref().unwrap();
            assert_eq!(menu.origin.document, origin);
            assert_eq!(app.workspace.documents.active_id(), origin);
            assert_eq!(
                app.workspace
                    .documents
                    .active()
                    .unwrap()
                    .editor
                    .buffer
                    .join("\n"),
                "original\nsecond"
            );
            assert!(menu.area.right() <= width && menu.area.bottom() <= height);
        }
        before_mouse(&mut app, &Event::Resize(1, 1), |_| {
            panic!("nonmouse must not render here")
        })
        .unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn transport_genuine_terminal_restart_rejects_previous_session_bytes() {
        struct Cleanup(App);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                self.0.shutdown_terminal();
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let mut app = Cleanup(App::new(directory.path(), AppConfig::default()).unwrap());
        let (tx, _rx) = crate::event::event_channel(Default::default());
        let old = crate::terminal::pty::PtyProcess::spawn(
            "/bin/sh",
            directory.path(),
            24,
            80,
            tx.clone(),
        )
        .unwrap();
        let old_session = old.session();
        old.shutdown();
        let current =
            crate::terminal::pty::PtyProcess::spawn("/bin/sh", directory.path(), 24, 80, tx)
                .unwrap();
        let current_session = current.session();
        assert_ne!(old_session, current_session);
        app.0.terminal_state.pty = Some(current);
        app.0.set_status_message("current terminal".into());
        let failure = terminal::pty::InputOutcome::Failed {
            written: 3,
            reason: "injected failure",
        };
        process_input_completion(&mut app.0, old_session, 1, failure);
        assert_eq!(app.0.status_message.as_ref().unwrap().0, "current terminal");
        process_input_completion(
            &mut app.0,
            current_session,
            2,
            terminal::pty::InputOutcome::Written,
        );
        assert_eq!(app.0.status_message.as_ref().unwrap().0, "current terminal");
        process_input_completion(&mut app.0, current_session, 3, failure);
        assert_eq!(
            app.0.status_message.as_ref().unwrap().0,
            "Terminal input #3 failed after 3 bytes: injected failure"
        );
        process_terminal(&mut app.0, old_session, b"OLD");
        process_terminal(&mut app.0, current_session, b"NEW");
        let text: String = app.0.terminal_state.emulator.render_lines()[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(text.trim(), "NEW");
    }
}

/// A terminal-based file manager TUI.
#[derive(Parser, Debug)]
#[command(name = "fm", version, about)]
struct Cli {
    /// Root path to display (defaults to current directory)
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Path to config file
    #[arg(short = 'c', long = "config")]
    config: Option<PathBuf>,

    /// Explicit workspace key profile (never inferred from terminal/environment)
    #[arg(long, value_enum)]
    keymap_profile: Option<crate::keymap::KeymapProfile>,

    /// Disable preview panel
    #[arg(long)]
    no_preview: bool,

    /// Disable filesystem watcher (auto-refresh)
    #[arg(long)]
    no_watcher: bool,

    /// Use ASCII instead of Nerd Font icons
    #[arg(long)]
    no_icons: bool,

    /// Disable mouse support
    #[arg(long)]
    no_mouse: bool,

    /// Disable embedded terminal
    #[arg(long)]
    no_terminal: bool,

    /// Disable read-only Git indicators
    #[arg(long)]
    no_git: bool,

    /// Lines from top for large file preview
    #[arg(long)]
    head_lines: Option<usize>,

    /// Lines from bottom for large file preview
    #[arg(long)]
    tail_lines: Option<usize>,

    /// Max file size (bytes) for full preview
    #[arg(long)]
    max_preview: Option<u64>,

    /// Color theme: dark, light
    #[arg(long)]
    theme: Option<String>,

    /// AWS profile for S3 mode (used when PATH is an s3:// URI)
    #[arg(long = "aws-profile")]
    aws_profile: Option<String>,

    /// Number of lines to stream for S3 head preview (default: 100)
    #[arg(long)]
    s3_head_lines: Option<usize>,
}

impl Cli {
    /// Convert CLI flags into a partial `AppConfig` for the merge chain.
    /// Only flags that were explicitly set produce `Some` values.
    fn as_config_overrides(&self) -> AppConfig {
        AppConfig {
            layout: Default::default(),
            keymap: crate::keymap::KeymapConfig {
                profile: self.keymap_profile,
                ..Default::default()
            },
            general: GeneralConfig {
                default_path: None, // path is handled separately via positional arg
                show_hidden: None,
                confirm_delete: None,
                mouse: if self.no_mouse { Some(false) } else { None },
                max_entries_per_page: None,
                search_max_entries: None,
                snapshot_max_entries: None,
                max_editor_bytes: None,
                max_editor_lines: None,
                ..Default::default()
            },
            preview: PreviewConfig {
                max_full_preview_bytes: self.max_preview,
                head_lines: self.head_lines,
                tail_lines: self.tail_lines,
                default_view_mode: None,
                tab_width: None,
                line_wrap: None,
                syntax_theme: None,
                enabled: if self.no_preview { Some(false) } else { None },
                preview_timeout_ms: None,
                s3_head_lines: self.s3_head_lines,
            },
            tree: TreeConfig {
                sort_by: None,
                dirs_first: None,
                use_icons: if self.no_icons { Some(false) } else { None },
                scroll_lines: None,
            },
            watcher: WatcherConfig {
                enabled: if self.no_watcher { Some(false) } else { None },
                debounce_ms: None,
                auto_refresh: None,
                mode: None,
                poll_interval_ms: None,
            },
            terminal: crate::config::TerminalConfig {
                enabled: if self.no_terminal { Some(false) } else { None },
                default_shell: None,
                scrollback_lines: None,
            },
            session: crate::config::SessionConfig::default(),
            recovery: crate::config::RecoveryConfig::default(),
            git: crate::config::GitConfig {
                enabled: if self.no_git { Some(false) } else { None },
            },
            theme: crate::config::ThemeConfig {
                scheme: self.theme.clone(),
                custom: None,
            },
        }
    }
}

#[cfg(test)]
mod keymap_cli_tests {
    use super::*;
    #[test]
    fn keymap_cli_explicit_web_and_unknown_profile() {
        assert!(Cli::try_parse_from(["fm", "--keymap-profile", "web"]).is_ok());
        assert!(Cli::try_parse_from(["fm", "--keymap-profile", "unknown"]).is_err());
    }
    #[test]
    fn keymap_cli_profile_wins_toml_but_unspecified_cli_preserves_it() {
        use crate::keymap::{FocusContext, Keymap, KeymapProfile};
        let file: AppConfig = toml::from_str(
            r#"
[keymap]
profile = "web"
[[keymap.bindings]]
command = "document.save"
context = "editor"
keys = ["F9"]
"#,
        )
        .unwrap();
        let defaults = Cli::try_parse_from(["fm"]).unwrap().as_config_overrides();
        assert_eq!(
            file.clone().merge(&defaults).keymap.profile,
            Some(KeymapProfile::Web)
        );
        let explicit = Cli::try_parse_from(["fm", "--keymap-profile", "standard"])
            .unwrap()
            .as_config_overrides();
        let merged = file.merge(&explicit);
        assert_eq!(merged.keymap.profile, Some(KeymapProfile::Standard));
        let map = Keymap::compile(&merged.keymap).unwrap();
        assert_eq!(
            map.binding_labels(crate::commands::CommandId::Save, FocusContext::Editor),
            ["F9"]
        );
        assert_eq!(
            Cli::try_parse_from(["fm"])
                .unwrap()
                .as_config_overrides()
                .keymap
                .profile,
            None
        );
    }

    #[test]
    fn git_cli_flag_disables_indicators_without_touching_other_sources() {
        // No flag: the override leaves the option unset so file/default wins.
        let defaults = Cli::try_parse_from(["fm"]).unwrap().as_config_overrides();
        assert_eq!(defaults.git.enabled, None);
        assert!(defaults.git_enabled());

        // `--no-git` explicitly disables and wins the merge.
        let disabled = Cli::try_parse_from(["fm", "--no-git"])
            .unwrap()
            .as_config_overrides();
        assert_eq!(disabled.git.enabled, Some(false));
        let file: AppConfig = toml::from_str("[git]\nenabled = true\n").unwrap();
        assert!(!file.merge(&disabled).git_enabled());
    }
}

#[tokio::main]
async fn main() -> error::Result<()> {
    let cli = Cli::parse();

    // Detect S3 mode: PATH starts with "s3://"
    let path_str = cli.path.to_string_lossy();
    let s3_config = if path_str.starts_with("s3://") {
        let s3_path = s3::S3Path::parse(&path_str)
            .ok_or_else(|| error::AppError::InvalidPath(format!("Invalid S3 URI: {}", path_str)))?;

        // Validate AWS CLI is available
        s3::S3Backend::check_cli()
            .await
            .map_err(error::AppError::InvalidPath)?;

        Some(s3::S3Config {
            path: s3_path,
            profile: cli.aws_profile.clone(),
        })
    } else {
        None
    };

    // For S3 mode, use CWD as the local path (the tree is virtual)
    let path = if s3_config.is_some() {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        cli.path.canonicalize().map_err(|_| {
            error::AppError::InvalidPath(format!("{} does not exist", cli.path.display()))
        })?
    };

    // Load configuration: file sources + CLI overrides
    let cli_overrides = cli.as_config_overrides();
    let config = AppConfig::load_checked(cli.config.as_deref(), Some(&cli_overrides))
        .map_err(error::AppError::InvalidPath)?;

    install_panic_hook();

    let mut app = App::new(&path, config)?;

    // Initialize S3 mode if configured
    if let Some(ref s3_cfg) = s3_config {
        app.init_s3_mode(s3_cfg.clone());
    }

    // Session restore runs once before the event loop, so the input path never
    // performs state I/O. The payload size is validated before parsing and a
    // corrupt/unavailable record is non-fatal: it only surfaces a status line.
    let session_store = if app.config.session_enabled() && !app.is_s3_mode() {
        let (store, notice) =
            session::SessionStore::from_config_dir(app.config.session_state_dir());
        if let Some(notice) = notice {
            app.set_status_message(notice);
        }
        store
    } else {
        None
    };
    if let Some(store) = &session_store {
        if let Some(notice) = store.restore_into(&path, &mut app.workspace) {
            app.set_status_message(notice);
        }
    }

    // Recovery discovery is bounded (retention caps the retained count and each
    // record read is size-capped) and runs once before the event loop, so the
    // input path and render never perform snapshot I/O. S3 mode has no local
    // workspace to key records against.
    if !app.is_s3_mode() {
        let (store, notice) =
            recovery::RecoveryStore::from_config_dir(true, app.config.recovery_state_dir());
        match store {
            Some(store) => {
                if let Some(notice) = app.configure_recovery(store) {
                    app.set_status_message(notice);
                }
                // Offer restore/discard only when real unsaved work exists.
                app.open_recovery_prompt();
            }
            None => {
                if app.config.recovery_enabled() {
                    if let Some(notice) = notice {
                        app.set_status_message(notice);
                    }
                }
            }
        }
    }

    let mut tui = Tui::new(app.config.mouse_enabled())?;
    let mut events = EventHandler::new(Duration::from_millis(16));
    let event_tx = events.sender();
    app.event_tx = Some(event_tx.clone());
    // Production runs the prepared-only pipeline: the loop below drains
    // versioned preview/syntax work instead of rendering-time loads.
    app.prepared_pipeline = true;

    let (watcher_flag_tx, watcher_flag_rx) = std::sync::mpsc::sync_channel::<Arc<AtomicBool>>(1);
    let mut watcher_flag_rx = Some(watcher_flag_rx);
    let mut watcher_flag: Option<Arc<AtomicBool>> = None;

    // Initialize filesystem watcher in the background so startup stays responsive
    // even for very deep/large directory trees.
    // Watcher is disabled in S3 mode (no local filesystem to watch).
    let watcher_mode = app.config.watcher_mode();
    let poll_interval = Duration::from_millis(app.config.poll_interval_ms());
    let watcher_thread = if !app.config.watcher_enabled() || app.is_s3_mode() {
        app.watcher_active = false;
        None
    } else {
        let root = path.clone();
        let debounce = Duration::from_millis(app.config.debounce_ms());
        let watcher_tx = event_tx.clone();
        let watcher_flag_tx = watcher_flag_tx.clone();
        let ignore_patterns: Vec<String> = fs::watcher::DEFAULT_IGNORE_PATTERNS
            .iter()
            .map(|s| s.to_string())
            .collect();

        Some(std::thread::spawn(move || match watcher_mode {
            WatcherMode::Event => match FsWatcher::new(
                &root,
                debounce,
                ignore_patterns,
                fs::watcher::DEFAULT_FLOOD_THRESHOLD,
                watcher_tx.clone(),
            ) {
                Ok(watcher) => {
                    let _ = watcher_flag_tx.send(watcher.active_flag());
                    watcher_tx.wait_closed();
                    drop(watcher);
                }
                Err(e) => {
                    let _ = watcher_tx.blocking_send(Event::WatcherInitFailed(e.to_string()));
                }
            },
            WatcherMode::Polling => {
                let mut watcher = PollingWatcher::new(
                    &root,
                    poll_interval,
                    ignore_patterns,
                    fs::watcher::DEFAULT_FLOOD_THRESHOLD,
                    fs::watcher::DEFAULT_POLL_MAX_ENTRIES,
                    watcher_tx.clone(),
                );
                let _ = watcher_flag_tx.send(watcher.active_flag());
                // Scans run on this dedicated watcher thread, never the input
                // path, and sleep the configured interval between scans.
                watcher.run_blocking();
            }
        }))
    };

    drop(watcher_flag_tx);

    // Kick off async loading of root directory contents.
    // In S3 mode, this loads the S3 prefix listing instead.
    if app.is_s3_mode() {
        app.spawn_s3_initial_load(&event_tx);
    } else {
        app.spawn_initial_load(&event_tx);
    }

    // Read-only Git status refresh, generation-tagged so a stale result from an
    // earlier workspace or request can never overwrite newer state. Triggered
    // outside render through the backend's bounded background transport.
    app.request_git_refresh();

    let epoch = std::time::Instant::now();
    let mut policy = LoopPolicy::new(Duration::ZERO, background::RedrawLimits::default(), 65536);
    let loop_result: error::Result<()> = async {
        loop {
            let now = epoch.elapsed();
            if policy.redraw.wait(now) == Some(Duration::ZERO) {
                policy.flush(|session, bytes| process_terminal(&mut app, session, bytes));
                app.advance_syntax_preparation();
                tui.terminal_mut()
                    .draw(|frame| ui::render(&mut app, frame))?;
                policy.redraw.drawn(epoch.elapsed());
                // Queue-ready events must not starve async completion producers.
                tokio::task::yield_now().await;
            }
            let event =
                match wait_for_loop(&mut app, events.next(), policy.redraw.wait(epoch.elapsed()))
                    .await?
                {
                    LoopWake::Timer => {
                        if app.expire_status(std::time::Instant::now()) {
                            policy.redraw.dirty(true);
                        }
                        if let Some(warning) =
                            app.snapshot_dirty_documents(std::time::Instant::now())
                        {
                            app.set_status_message(warning);
                        }
                        if app
                            .issue_deferred_git_refresh(std::time::Instant::now())
                            .is_some()
                        {
                            policy.redraw.dirty(true);
                        }
                        continue;
                    }
                    LoopWake::Background(delivery) => {
                        process_background(&mut app, &mut policy, delivery);
                        use std::io::IsTerminal;
                        let term = std::env::var("TERM").unwrap_or_default();
                        let osc_available = std::io::stdout().is_terminal()
                            && ["xterm", "screen", "tmux", "rxvt"]
                                .iter()
                                .any(|prefix| term.starts_with(prefix));
                        app.present_copy_fallback(tui.terminal_mut().backend_mut(), osc_available);
                        continue;
                    }
                    LoopWake::Event(event) => event,
                };
            if !matches!(event, Event::TerminalOutput { .. }) {
                policy.flush(|session, bytes| process_terminal(&mut app, session, bytes));
            }
            before_mouse(&mut app, &event, |app| {
                tui.terminal_mut().draw(|frame| ui::render(app, frame))?;
                Ok(())
            })?;
            policy.observed(&event);
            match event {
                Event::Key(key) => handler::handle_key_event(&mut app, key, &event_tx),
                Event::Paste(text) => handler::handle_paste_event(&mut app, &text),
                Event::Mouse(mouse) => handler::handle_mouse_event(&mut app, mouse, &event_tx),
                Event::Resize(_, _) => {}
                Event::Progress(_) | Event::OperationComplete(_) => {}
                Event::FsChange(paths) => {
                    app.handle_fs_change(paths);
                    // Working-tree changes can alter indicators; re-query through
                    // the bounded background transport (never in render).
                    app.request_git_refresh();
                }
                Event::TerminalOutput { session, data } => {
                    policy.output(session, &data, |session, bytes| {
                        process_terminal(&mut app, session, bytes)
                    });
                }
                Event::TerminalClosed { session } => {
                    if app
                        .terminal_state
                        .pty
                        .as_ref()
                        .is_some_and(|pty| pty.session() == session)
                    {
                        app.terminal_state.exited = true;
                    }
                }
                Event::TerminalInputComplete {
                    session,
                    sequence,
                    outcome,
                } => {
                    process_input_completion(&mut app, session, sequence, outcome);
                }
                Event::TransportRejected(reason) => app.set_status_message(reason.into()),
                Event::DirScanComplete { path, snapshot } => {
                    app.handle_dir_scan_complete(&path, snapshot);
                }
                Event::DirCountComplete { path, count } => {
                    app.handle_dir_count_complete(&path, count);
                }
                // Untargeted legacy summary envelopes cannot identify a current
                // request. Native results/progress are drained directly above.
                Event::DirSummaryUpdate { .. } | Event::ShallowDirSummary { .. } => {}
                Event::ClipboardCopyComplete(_) | Event::ShowCopyableText(_) => {}
                // S3 completions now require typed pool/domain identities.
                Event::S3HeadComplete { .. } | Event::S3ListingComplete { .. } => {}
                Event::WatcherInitFailed(msg) => {
                    app.watcher_active = false;
                    app.set_status_message(format!("⚠ Watcher unavailable: {}", msg));
                }
                Event::GitState(refresh) => {
                    // Generation-tagged: stale results are refused in the state.
                    let _ = app.accept_git_refresh(refresh);
                }
            }

            // Throttled, bounded snapshot pass for dirty documents. Runs
            // outside render and outside the input handlers; the throttle keeps
            // it to at most one pass per configured interval.
            if let Some(warning) = app.snapshot_dirty_documents(std::time::Instant::now()) {
                app.set_status_message(warning);
            }

            app.restore_copy_mouse_capture(tui.terminal_mut().backend_mut());

            if watcher_flag.is_none() {
                if let Some(rx) = &watcher_flag_rx {
                    match rx.try_recv() {
                        Ok(flag) => {
                            watcher_flag = Some(flag);
                            watcher_flag_rx = None;
                        }
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            watcher_flag_rx = None;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    }
                }
            }

            if let Some(flag) = &watcher_flag {
                // Backends always forward events; the coordinator gates only the
                // tree refresh, so an open editor still observes external
                // document writes in manual-refresh mode.
                flag.store(true, Ordering::Relaxed);
            }

            if app.should_quit {
                break;
            }
        }

        Ok(())
    }
    .await;

    // Consumer close precedes any producer/child joins, waking full-queue senders.
    events.rx.close();
    app.cancel_token.store(true, Ordering::SeqCst);
    app.shutdown_terminal();
    app.shutdown_background().await;
    events.shutdown().await;
    if let Some(watcher) = watcher_thread {
        let _ = tokio::task::spawn_blocking(move || watcher.join()).await;
    }
    // Clean up S3 cache
    app.cleanup_s3();
    // Persist the session after the loop but before restoring the terminal, so a
    // state-directory failure is reported on stderr without aborting shutdown.
    if let Some(store) = &session_store {
        if let Some(warning) = store.persist_from(&path, &app.workspace) {
            eprintln!("fm: {warning}");
        }
    }
    // Bounded final snapshot pass (one pass, no unbounded flush loop). Its
    // failures are reported on stderr, never swallowed.
    if let Some(warning) = app.flush_recovery_on_shutdown() {
        eprintln!("fm: {warning}");
    }
    tui.restore()?;
    loop_result
}
