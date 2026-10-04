//! Installed-server LSP client (FR-10).
//!
//! LSP is optional: unsupported files, missing servers, and startup/crash
//! errors never block editing. `positions` adapts position encodings,
//! `transport` bounds the byte-level frames, `client` drives the
//! request/restart/shutdown lifecycle over one server generation at a time,
//! and `config` maps languages to argv plus the project-trust gate.
//!
//! `LspManager` (this file) owns the live sessions for `App`: one pump
//! thread per server generation, generation-tagged `Event::Lsp` results, a
//! bounded command channel into each session, and the trust queue that
//! serializes interactive project-argv approvals.

pub mod client;
pub mod config;
pub mod positions;
pub mod transport;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, SyncSender};
use std::time::Duration;

use client::{Client, ClientError, ClientEvent, ClientOptions};
use config::{ResolvedServer, SpawnDecision, TrustStore};

/// Control messages into a running server session's pump thread. Bounded
/// like the transport itself — a flooded queue rejects, never grows.
#[derive(Debug)]
pub enum LspCommand {
    /// Graceful shutdown + bounded teardown, then the thread exits.
    Shutdown,
    /// Respawn the same argv (restart budget applies in the client).
    Restart,
    /// Send one encoded JSON-RPC body (notification or request).
    /// Written by Phase 11's document-to-request mapping.
    #[allow(dead_code)]
    Send(Vec<u8>),
}

/// What the UI shows for one server session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Starting,
    Ready {
        encoding: positions::PositionEncoding,
    },
    /// Running but degraded (e.g. a request expired).
    Degraded(String),
    Dead(String),
    /// Project argv awaiting (or refused) explicit trust.
    /// Reserved for status displays; the trust queue carries the payload.
    #[allow(dead_code)]
    NeedsTrust,
    Denied(&'static str),
    MissingExecutable(String),
}

impl SessionStatus {
    fn as_word(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready { .. } => "ready",
            Self::Degraded(_) => "degraded",
            Self::Dead(_) => "dead",
            Self::NeedsTrust => "needs trust",
            Self::Denied(_) => "denied",
            Self::MissingExecutable(_) => "missing",
        }
    }
}

struct LspSession {
    resolved: ResolvedServer,
    /// Into the pump thread; `None` once the session is dead/shut down.
    cmd_tx: Option<SyncSender<LspCommand>>,
    status: SessionStatus,
    /// Highest event generation seen — guards stale-event replay.
    generation: u64,
}

/// A project-local argv awaiting (or refused) interactive approval.
#[derive(Debug, Clone)]
pub struct PendingTrust {
    pub resolved: ResolvedServer,
    /// File whose language mapping triggered the resolution. Kept for
    /// Phase 11's document-scoped dispatch after approval.
    #[allow(dead_code)]
    pub for_path: PathBuf,
}

/// Runtime LSP state for one `App`. Not `Send` by itself — sessions own
/// their pump threads and communicate via bounded channels.
#[derive(Default)]
pub struct LspManager {
    sessions: HashMap<String, LspSession>,
    trust: TrustStore,
    /// Project argv waiting for the interactive dialog, one at a time.
    pending_trust: VecDeque<PendingTrust>,
}

impl LspManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed durable grants from a *global/CLI* layer only (project-local
    /// `[[lsp.trust]]` entries are already stripped at load).
    pub fn apply_config_trust(&mut self, global: &config::LspConfig) {
        self.trust.apply_config_grants(&global.trust);
    }

    /// One-line status per live/attempted session for status bars.
    pub fn status_summary(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .sessions
            .values()
            .map(|s| {
                let detail = match &s.status {
                    SessionStatus::Ready { encoding } => encoding.as_str().to_string(),
                    SessionStatus::Degraded(d) | SessionStatus::Dead(d) => d.clone(),
                    SessionStatus::Denied(d) => d.to_string(),
                    SessionStatus::MissingExecutable(e) => format!("{e} not found"),
                    _ => String::new(),
                };
                config::status_line(Some(&s.resolved), s.status.as_word(), &detail)
            })
            .collect();
        for pending in &self.pending_trust {
            lines.push(config::status_line(
                Some(&pending.resolved),
                "needs trust",
                "awaiting approval",
            ));
        }
        lines.sort();
        lines
    }

    /// The next project argv needing the interactive trust dialog.
    pub fn next_pending_trust(&self) -> Option<&PendingTrust> {
        self.pending_trust.front()
    }

    /// Is `language` already live (any status that spawned or will)?
    #[allow(dead_code)] // Consumed by Phase 11 capability checks.
    pub fn has_session(&self, language: &str) -> bool {
        self.sessions.contains_key(language)
    }

    /// Session status for the current-file indicator.
    pub fn session_status(&self, language: &str) -> Option<&SessionStatus> {
        self.sessions.get(language).map(|s| &s.status)
    }

    /// Resolve `path` → language → spec and start or gate its server.
    ///
    /// `interactive` = a UI exists to answer a trust prompt. Returns a short
    /// status string when something user-visible changed (spawned, gated,
    /// missing) or `None` for silent no-ops (no language, no config).
    pub fn maybe_start_for_path(
        &mut self,
        path: &Path,
        global: &config::LspConfig,
        local: &config::LspConfig,
        interactive: bool,
        event_tx: &crate::event::EventSender,
    ) -> Option<String> {
        if !global.enabled() {
            return None;
        }
        let language = config::language_for_path(path, &global.languages)?;
        let resolved = config::resolve_server(&language, path, global, local)?;
        // Trust binds to the exact argv: a queued prompt or session for an
        // argv the config no longer names is stale and re-gates below.
        if let Some(pos) = self
            .pending_trust
            .iter()
            .position(|p| p.resolved.spec.language == language)
        {
            if self.pending_trust[pos].resolved.spec.argv == resolved.spec.argv {
                return None;
            }
            self.pending_trust.remove(pos);
        }
        if let Some(session) = self.sessions.get_mut(&language) {
            if session.resolved.spec.argv == resolved.spec.argv {
                return None;
            }
            // The command changed: boundedly shut the stale server down on
            // its own pump thread, then re-gate the new argv fresh.
            if let Some(tx) = session.cmd_tx.take() {
                let _ = tx.try_send(LspCommand::Shutdown);
            }
            self.sessions.remove(&language);
        }
        match resolved.decide(&self.trust, interactive) {
            SpawnDecision::Allowed => {
                let status = self.spawn_session(resolved, event_tx);
                Some(status)
            }
            SpawnDecision::NeedsTrust => {
                let language = resolved.spec.language.clone();
                self.pending_trust.push_back(PendingTrust {
                    resolved,
                    for_path: path.to_path_buf(),
                });
                Some(format!(
                    "LSP {language}: project server needs trust approval"
                ))
            }
            SpawnDecision::MissingExecutable(program) => {
                let language = resolved.spec.language.clone();
                self.sessions.insert(
                    language.clone(),
                    LspSession {
                        resolved,
                        cmd_tx: None,
                        status: SessionStatus::MissingExecutable(program.clone()),
                        generation: 0,
                    },
                );
                Some(format!(
                    "LSP {language}: `{program}` not found — LSP off for this file"
                ))
            }
            SpawnDecision::DeniedHeadless => {
                let language = resolved.spec.language.clone();
                self.sessions.insert(
                    language.clone(),
                    LspSession {
                        resolved,
                        cmd_tx: None,
                        status: SessionStatus::Denied("headless start"),
                        generation: 0,
                    },
                );
                Some(format!(
                    "LSP {language}: project server needs trust (headless — skipped)"
                ))
            }
            SpawnDecision::DeniedByUser => {
                let language = resolved.spec.language.clone();
                self.sessions.insert(
                    language.clone(),
                    LspSession {
                        resolved,
                        cmd_tx: None,
                        status: SessionStatus::Denied("refused"),
                        generation: 0,
                    },
                );
                Some(format!("LSP {language}: refused earlier this session"))
            }
            SpawnDecision::Disabled(reason) => Some(format!("LSP {language}: disabled ({reason})")),
        }
    }

    /// Approve the front of the trust queue: bind (root, argv) for this
    /// session and spawn. Returns the status string.
    pub fn approve_next_trust(&mut self, event_tx: &crate::event::EventSender) -> Option<String> {
        let pending = self.pending_trust.pop_front()?;
        self.trust
            .grant(&pending.resolved.root, &pending.resolved.spec.argv);
        Some(self.spawn_session(pending.resolved, event_tx))
    }

    /// Refuse the front of the trust queue; the (root, argv) pair is denied
    /// for the rest of the session.
    pub fn deny_next_trust(&mut self) -> Option<String> {
        let pending = self.pending_trust.pop_front()?;
        self.trust
            .deny(&pending.resolved.root, &pending.resolved.spec.argv);
        Some(format!(
            "LSP {}: refused — server will not start this session",
            pending.resolved.spec.language
        ))
    }

    /// Send an already-encoded JSON body to a live session's server.
    /// Returns false when the session is absent/dead — the UI degrades.
    #[allow(dead_code)] // Consumed by Phase 11 feature requests (didOpen, …).
    pub fn send(&self, language: &str, body: &[u8]) -> bool {
        self.sessions
            .get(language)
            .and_then(|s| s.cmd_tx.as_ref())
            .is_some_and(|tx| tx.try_send(LspCommand::Send(body.to_vec())).is_ok())
    }

    /// Restart a dead/degraded session within its restart budget.
    pub fn restart(&mut self, language: &str) -> Option<String> {
        let session = self.sessions.get_mut(language)?;
        let tx = session.cmd_tx.as_ref()?;
        tx.try_send(LspCommand::Restart).ok()?;
        session.status = SessionStatus::Starting;
        Some(format!("LSP {language}: restarting"))
    }

    /// Route one generation-tagged event into the session table; returns a
    /// status string for the few transitions the user should see.
    pub fn handle_event(
        &mut self,
        language: &str,
        generation: u64,
        event: ClientEvent,
    ) -> Option<String> {
        let session = self.sessions.get_mut(language)?;
        if generation < session.generation {
            return None; // stale event from a previous server process
        }
        match event {
            ClientEvent::Ready {
                generation,
                encoding,
            } => {
                session.generation = generation;
                session.status = SessionStatus::Ready { encoding };
                Some(format!("LSP {language}: ready ({})", encoding.as_str()))
            }
            ClientEvent::ServerDied { generation, reason } => {
                session.generation = generation;
                session.status = SessionStatus::Dead(reason.clone());
                session.cmd_tx = None;
                Some(format!("LSP {language}: stopped ({reason})"))
            }
            ClientEvent::Expired { method, .. } => {
                session.status = SessionStatus::Degraded(format!("{method} timed out"));
                None
            }
            ClientEvent::Response { .. } | ClientEvent::Notification { .. } => {
                // Feature payloads are consumed by Phase 11 wiring; the
                // session table only tracks lifecycle transitions here.
                None
            }
        }
    }

    /// Graceful bounded teardown for every live session; dead ones are
    /// already gone. Called from app shutdown, after the event loop.
    pub fn shutdown_all(&mut self) {
        for session in self.sessions.values_mut() {
            if let Some(tx) = session.cmd_tx.take() {
                let _ = tx.try_send(LspCommand::Shutdown);
            }
        }
        self.sessions.clear();
        self.pending_trust.clear();
    }

    fn spawn_session(
        &mut self,
        resolved: ResolvedServer,
        event_tx: &crate::event::EventSender,
    ) -> String {
        let language = resolved.spec.language.clone();
        let (cmd_tx, cmd_rx) = mpsc::sync_channel::<LspCommand>(64);
        let argv = resolved.spec.argv.clone();
        let cwd = resolved.root.clone();
        let tx = event_tx.clone();
        let thread_language = language.clone();
        std::thread::Builder::new()
            .name(format!("lsp-{language}"))
            .spawn(move || run_server(argv, cwd, thread_language, tx, cmd_rx))
            .ok();
        self.sessions.insert(
            language.clone(),
            LspSession {
                resolved,
                cmd_tx: Some(cmd_tx),
                status: SessionStatus::Starting,
                generation: 0,
            },
        );
        format!("LSP {language}: starting")
    }
}

/// The pump thread: owns the `Client`, forwards every event as
/// `Event::Lsp { language, generation, event }`, applies control commands,
/// and always exits through the bounded client shutdown.
fn run_server(
    argv: Vec<String>,
    cwd: PathBuf,
    language: String,
    event_tx: crate::event::EventSender,
    cmd_rx: mpsc::Receiver<LspCommand>,
) {
    let emit = |generation: u64, event: ClientEvent| {
        let _ = event_tx.blocking_send(crate::event::Event::Lsp {
            language: language.clone(),
            generation,
            event,
        });
    };

    let root_uri = format!("file://{}", cwd.display());
    let mut client = match Client::spawn(&argv, &cwd, &root_uri, ClientOptions::default()) {
        Ok(c) => c,
        Err(ClientError::Io(e)) => {
            emit(
                0,
                ClientEvent::ServerDied {
                    generation: 0,
                    reason: e.to_string(),
                },
            );
            return;
        }
        Err(_) => return,
    };
    for event in client.initialize_blocking(Duration::from_secs(10)) {
        emit(client.generation(), event);
    }

    loop {
        if !matches!(
            client.state(),
            client::ClientState::Ready | client::ClientState::Starting
        ) {
            break;
        }
        // Control first: shutdown/restart/sends never wait behind the poll.
        while let Ok(command) = cmd_rx.try_recv() {
            match command {
                LspCommand::Shutdown => {
                    client.shutdown();
                    return;
                }
                LspCommand::Restart => match client.restart(&argv, &cwd, &root_uri) {
                    Ok(_) => {
                        for event in client.initialize_blocking(Duration::from_secs(10)) {
                            emit(client.generation(), event);
                        }
                    }
                    Err(ClientError::RestartBudgetExhausted) => {
                        emit(
                            client.generation(),
                            ClientEvent::ServerDied {
                                generation: client.generation(),
                                reason: "restart budget exhausted".to_string(),
                            },
                        );
                        return;
                    }
                    Err(ClientError::Io(e)) => {
                        emit(
                            client.generation(),
                            ClientEvent::ServerDied {
                                generation: client.generation(),
                                reason: e.to_string(),
                            },
                        );
                        return;
                    }
                },
                LspCommand::Send(body) => {
                    // Best-effort forward; a dead transport reports through
                    // the next poll as ServerDied.
                    if let Ok(message) = serde_json::from_slice::<serde_json::Value>(&body) {
                        let method = message["method"].as_str().unwrap_or_default().to_string();
                        if message.get("id").is_some() {
                            let _ = client.request(&method, message["params"].clone());
                        } else {
                            let _ = client.notify(&method, message["params"].clone());
                        }
                    }
                }
            }
        }
        for event in client.pump(Duration::from_millis(20)) {
            emit(client.generation(), event);
        }
    }
    // Loop exit = client died; teardown is bounded inside client.shutdown.
    client.shutdown();
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::{ConfigSource, LspConfig, ServerEntry};

    fn local_config_with(argv: &[&str]) -> LspConfig {
        LspConfig {
            servers: [(
                "rust".to_string(),
                ServerEntry {
                    argv: argv.iter().map(|s| s.to_string()).collect(),
                    root_markers: vec!["Cargo.toml".to_string()],
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        }
    }

    fn event_channel() -> crate::event::EventSender {
        crate::event::event_channel(crate::event::TransportLimits::default()).0
    }

    #[test]
    fn project_argv_needs_trust_then_spawns_after_approval() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();

        // Project-local config names a real executable so only trust gates it.
        let local = local_config_with(&["/bin/true"]);
        let global = LspConfig::default();
        let mut manager = LspManager::new();
        let tx = event_channel();
        // Headless: refused without ever asking.
        let note = manager
            .maybe_start_for_path(&file, &global, &local, false, &tx)
            .unwrap();
        assert!(note.contains("trust"), "{note}");
        assert_eq!(
            manager.session_status("rust"),
            Some(&SessionStatus::Denied("headless start"))
        );
    }

    #[test]
    fn trust_dialog_queue_approves_and_denies_one_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "").unwrap();
        let local = local_config_with(&["/bin/true"]);
        let global = LspConfig::default();
        let mut manager = LspManager::new();
        let tx = event_channel();

        manager
            .maybe_start_for_path(&file, &global, &local, true, &tx)
            .unwrap();
        let pending = manager.next_pending_trust().unwrap();
        assert_eq!(pending.resolved.spec.source, ConfigSource::ProjectLocal);
        assert_eq!(pending.resolved.spec.argv, vec!["/bin/true"]);

        // Deny → same (root, argv) never re-asks.
        let note = manager.deny_next_trust().unwrap();
        assert!(note.contains("refused"));
        assert!(manager.next_pending_trust().is_none());
        let note = manager
            .maybe_start_for_path(&file, &global, &local, true, &tx)
            .unwrap();
        assert!(note.contains("refused"), "{note}");
    }

    #[cfg(unix)]
    #[test]
    fn approved_project_server_spawns_and_reports_status() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "").unwrap();
        // `/bin/true` exits immediately — proves the pipe works end to end
        // and the session reports its death rather than hanging.
        let local = local_config_with(&["/bin/true"]);
        let global = LspConfig::default();
        let mut manager = LspManager::new();
        let (tx, _rx) = crate::event::event_channel(crate::event::TransportLimits::default());

        manager
            .maybe_start_for_path(&file, &global, &local, true, &tx)
            .unwrap();
        let note = manager.approve_next_trust(&tx).unwrap();
        assert!(note.contains("starting"), "{note}");
        assert!(matches!(
            manager.session_status("rust"),
            Some(SessionStatus::Starting)
        ));
        manager.shutdown_all();
    }

    #[test]
    fn missing_executable_is_status_not_blocker() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "").unwrap();
        // Global config → trusted, but argv[0] does not exist.
        let global = local_config_with(&["definitely-not-installed-fm-lsp"]);
        let mut manager = LspManager::new();
        let tx = event_channel();
        let note = manager
            .maybe_start_for_path(&file, &global, &LspConfig::default(), true, &tx)
            .unwrap();
        assert!(note.contains("not found"), "{note}");
        assert!(matches!(
            manager.session_status("rust"),
            Some(SessionStatus::MissingExecutable(_))
        ));
        // Editing is unaffected; summary renders the state.
        assert!(manager
            .status_summary()
            .iter()
            .any(|l| l.contains("missing")));
    }

    #[test]
    fn lsp_disabled_config_is_a_silent_noop() {
        let global = LspConfig {
            enabled: Some(false),
            ..Default::default()
        };
        let mut manager = LspManager::new();
        let tx = event_channel();
        assert!(manager
            .maybe_start_for_path(
                Path::new("/tmp/a.rs"),
                &global,
                &LspConfig::default(),
                true,
                &tx
            )
            .is_none());
    }

    #[test]
    fn stale_generation_events_are_dropped() {
        let mut manager = LspManager::new();
        let dir = tempfile::tempdir().unwrap();
        let resolved = ResolvedServer {
            spec: config::ServerSpec {
                language: "rust".to_string(),
                argv: vec!["/bin/true".to_string()],
                root_markers: vec![],
                source: ConfigSource::Global,
            },
            root: dir.path().to_path_buf(),
            executable: Some(PathBuf::from("/bin/true")),
        };
        manager.sessions.insert(
            "rust".to_string(),
            LspSession {
                resolved,
                cmd_tx: None,
                status: SessionStatus::Starting,
                generation: 3,
            },
        );
        // Events tagged older than the session's generation are ignored.
        assert!(manager
            .handle_event(
                "rust",
                2,
                ClientEvent::ServerDied {
                    generation: 2,
                    reason: "old".to_string()
                }
            )
            .is_none());
        assert!(matches!(
            manager.session_status("rust"),
            Some(SessionStatus::Starting)
        ));
        // Same-or-newer generation applies.
        assert!(manager
            .handle_event(
                "rust",
                4,
                ClientEvent::ServerDied {
                    generation: 4,
                    reason: "stopped".to_string()
                }
            )
            .is_some());
        assert!(matches!(
            manager.session_status("rust"),
            Some(SessionStatus::Dead(_))
        ));
    }

    #[test]
    fn modified_project_argv_regates_instead_of_reusing_stale_decision() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        let global = LspConfig::default();
        let mut manager = LspManager::new();
        let tx = event_channel();

        // Refuse argv A; the same command is remembered as refused.
        let local_a = local_config_with(&["/bin/true"]);
        manager
            .maybe_start_for_path(&file, &global, &local_a, true, &tx)
            .unwrap();
        manager.deny_next_trust();
        let note = manager
            .maybe_start_for_path(&file, &global, &local_a, true, &tx)
            .unwrap();
        assert!(note.contains("refused"), "{note}");

        // The project file then names a *different* command — the denial
        // binds to argv A, so argv B prompts fresh instead of inheriting.
        let local_b = local_config_with(&["/bin/false"]);
        manager
            .maybe_start_for_path(&file, &global, &local_b, true, &tx)
            .unwrap();
        let pending = manager.next_pending_trust().unwrap();
        assert_eq!(pending.resolved.spec.argv, vec!["/bin/false"]);

        // A queued prompt for an argv that changes before approval is
        // replaced by the newly configured command.
        let local_c = local_config_with(&["/bin/cat"]);
        manager
            .maybe_start_for_path(&file, &global, &local_c, true, &tx)
            .unwrap();
        assert_eq!(manager.pending_trust.len(), 1);
        assert_eq!(
            manager.next_pending_trust().unwrap().resolved.spec.argv,
            vec!["/bin/cat"]
        );
    }

    /// Fabricate a session entry without spawning — private fields are
    /// visible to this module's tests.
    fn fake_session(language: &str, argv: &[&str]) -> (String, LspSession) {
        (
            language.to_string(),
            LspSession {
                resolved: ResolvedServer {
                    spec: config::ServerSpec {
                        language: language.to_string(),
                        argv: argv.iter().map(|s| s.to_string()).collect(),
                        root_markers: vec![],
                        source: ConfigSource::Global,
                    },
                    root: std::env::temp_dir(),
                    executable: None,
                },
                cmd_tx: None,
                status: SessionStatus::Starting,
                generation: 0,
            },
        )
    }

    #[test]
    fn status_summary_lists_sessions_and_pending_by_word() {
        let mut manager = LspManager::new();
        let (language, mut session) = fake_session("rust", &["ra"]);
        session.status = SessionStatus::Ready {
            encoding: positions::PositionEncoding::Utf16,
        };
        manager.sessions.insert(language, session);
        let (language, mut session) = fake_session("python", &["pylsp"]);
        session.status = SessionStatus::Dead("crash".to_string());
        manager.sessions.insert(language, session);
        let (language, session) = fake_session("shell", &["bashls"]);
        manager.sessions.insert(language, session);
        let (language, mut session) = fake_session("go2", &["gopls"]);
        session.status = SessionStatus::Degraded("hover timed out".to_string());
        manager.sessions.insert(language, session);
        let (language, mut session) = fake_session("zig", &["zls"]);
        session.status = SessionStatus::Denied("refused");
        manager.sessions.insert(language, session);
        let (language, mut session) = fake_session("d", &["serve-d"]);
        session.status = SessionStatus::MissingExecutable("serve-d".to_string());
        manager.sessions.insert(language, session);
        let (language, mut session) = fake_session("c", &["clangd"]);
        session.status = SessionStatus::NeedsTrust;
        manager.sessions.insert(language, session);
        manager.pending_trust.push_back(PendingTrust {
            resolved: ResolvedServer {
                spec: config::ServerSpec {
                    language: "go".to_string(),
                    argv: vec!["gopls".to_string()],
                    root_markers: vec![],
                    source: ConfigSource::ProjectLocal,
                },
                root: PathBuf::from("/w"),
                executable: None,
            },
            for_path: PathBuf::from("/w/main.go"),
        });
        assert!(manager.has_session("rust"));
        assert!(!manager.has_session("go"));

        let lines = manager.status_summary();
        assert_eq!(lines.len(), 8);
        assert!(lines.iter().any(|l| l.contains("utf-16")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("crash")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("starting")), "{lines:?}");
        assert!(
            lines.iter().any(|l| l.contains("awaiting approval")),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("degraded")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("needs trust")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("refused")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("missing")), "{lines:?}");
    }

    #[test]
    fn handle_event_covers_ready_expired_and_feature_payloads() {
        let mut manager = LspManager::new();
        let (language, session) = fake_session("rust", &["ra"]);
        manager.sessions.insert(language, session);
        manager.sessions.get_mut("rust").unwrap().generation = 1;

        let note = manager
            .handle_event(
                "rust",
                2,
                ClientEvent::Ready {
                    generation: 2,
                    encoding: positions::PositionEncoding::Utf8,
                },
            )
            .unwrap();
        assert!(note.contains("ready"), "{note}");
        assert!(matches!(
            manager.session_status("rust"),
            Some(SessionStatus::Ready { .. })
        ));

        assert!(manager
            .handle_event(
                "rust",
                2,
                ClientEvent::Expired {
                    id: 1,
                    method: "textDocument/hover".to_string()
                }
            )
            .is_none());
        assert!(matches!(
            manager.session_status("rust"),
            Some(SessionStatus::Degraded(_))
        ));

        // Feature payloads stay lifecycle-silent until Phase 11 wiring.
        for event in [
            ClientEvent::Response {
                id: 1,
                generation: 2,
                outcome: Ok("{}".to_string()),
            },
            ClientEvent::Notification {
                generation: 2,
                method: "m".to_string(),
                params: "{}".to_string(),
            },
        ] {
            assert!(manager.handle_event("rust", 2, event).is_none());
        }
        // Unknown language is a no-op.
        assert!(manager
            .handle_event(
                "nope",
                1,
                ClientEvent::ServerDied {
                    generation: 1,
                    reason: "x".to_string()
                }
            )
            .is_none());
    }

    #[test]
    fn apply_config_trust_and_disabled_and_send_gate_paths() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        let mut manager = LspManager::new();
        let tx = event_channel();

        // Global-config grants seed the session store.
        manager.apply_config_trust(&LspConfig {
            trust: vec![config::TrustGrantEntry {
                root: dir.path().to_string_lossy().into_owned(),
                argv: vec!["ra".to_string()],
            }],
            ..Default::default()
        });
        assert!(manager.trust.allows(dir.path(), &["ra".to_string()]));

        // An invalid argv is Disabled — never reaches trust or spawn.
        let local = local_config_with(&[""]);
        let note = manager
            .maybe_start_for_path(&file, &LspConfig::default(), &local, true, &tx)
            .unwrap();
        assert!(note.contains("disabled"), "{note}");
        assert!(manager.next_pending_trust().is_none());

        // Send is a soft gate: missing session → false, live channel → true.
        assert!(!manager.send("rust", b"{}"));
        let (cmd_tx, _cmd_rx) = mpsc::sync_channel::<LspCommand>(4);
        let (language, mut session) = fake_session("rust", &["ra"]);
        session.cmd_tx = Some(cmd_tx);
        manager.sessions.insert(language, session);
        assert!(manager.send("rust", b"{}"));
        assert!(manager.restart("rust").is_some());
        assert!(manager.restart("nope").is_none());
    }

    #[test]
    fn stale_argv_session_is_shut_down_before_regating() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        let mut manager = LspManager::new();
        let tx = event_channel();

        // A *live* session (channel open) whose configured argv changed:
        // the pump thread gets a bounded Shutdown before the new gate.
        let (cmd_tx, cmd_rx) = mpsc::sync_channel::<LspCommand>(4);
        let (language, mut session) = fake_session("rust", &["/bin/true"]);
        session.cmd_tx = Some(cmd_tx);
        manager.sessions.insert(language, session);
        let local = local_config_with(&["/bin/false"]);
        manager
            .maybe_start_for_path(&file, &LspConfig::default(), &local, true, &tx)
            .unwrap();
        match cmd_rx.try_recv() {
            Ok(LspCommand::Shutdown) => {}
            other => panic!("expected Shutdown for the stale session, got {other:?}"),
        }
        assert_eq!(
            manager.next_pending_trust().unwrap().resolved.spec.argv,
            vec!["/bin/false"]
        );

        // Identical argv is a pure no-op (no re-prompt, no removal).
        manager.deny_next_trust();
        let local = local_config_with(&["/bin/false"]);
        let note = manager
            .maybe_start_for_path(&file, &LspConfig::default(), &local, true, &tx)
            .unwrap();
        assert!(note.contains("refused"), "{note}");
        // Queued identical prompt is also a no-op.
        let local = local_config_with(&["/bin/cat"]);
        manager
            .maybe_start_for_path(&file, &LspConfig::default(), &local, true, &tx)
            .unwrap();
        assert_eq!(manager.pending_trust.len(), 1);
        assert!(manager
            .maybe_start_for_path(&file, &LspConfig::default(), &local, true, &tx)
            .is_none());
        assert_eq!(manager.pending_trust.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn live_session_send_restart_and_shutdown_drive_the_pump_loop() {
        use std::time::Instant;
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join("fake-lsp-server.py");
        let mut manager = LspManager::new();
        let (tx, mut rx) = crate::event::event_channel(crate::event::TransportLimits::default());
        let resolved = ResolvedServer {
            spec: config::ServerSpec {
                language: "fakelang".to_string(),
                argv: vec![script.to_string_lossy().into_owned(), "success".to_string()],
                root_markers: vec![],
                source: ConfigSource::Global,
            },
            root: std::env::temp_dir(),
            executable: None,
        };
        manager.spawn_session(resolved, &tx);

        let wait_ready = |rx: &mut crate::event::EventReceiver, n: u32| {
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut seen = 0u32;
            while Instant::now() < deadline && seen < n {
                match rx.try_recv() {
                    Ok(crate::event::Event::Lsp {
                        event: ClientEvent::Ready { .. },
                        ..
                    }) => seen += 1,
                    Ok(_) => {}
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Err(_) => break,
                }
            }
            seen
        };
        assert!(wait_ready(&mut rx, 1) >= 1, "initial handshake");

        // Notification, request, and undecodable bodies all traverse Send.
        assert!(manager.send(
            "fakelang",
            br#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{}}"#
        ));
        assert!(manager.send(
            "fakelang",
            br#"{"jsonrpc":"2.0","id":7,"method":"textDocument/hover","params":{}}"#
        ));
        assert!(manager.send("fakelang", b"not json"));
        assert!(!manager.send("nope", b"{}"));

        assert_eq!(
            manager.restart("fakelang"),
            Some("LSP fakelang: restarting".to_string())
        );
        assert_eq!(wait_ready(&mut rx, 1), 1, "ready after restart");

        let wait_dead = |rx: &mut crate::event::EventReceiver, language: &str| {
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline {
                match rx.try_recv() {
                    Ok(crate::event::Event::Lsp {
                        language: l,
                        event: ClientEvent::ServerDied { reason, .. },
                        ..
                    }) if l == language => return Some(reason),
                    Ok(_) => {}
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Err(_) => break,
                }
            }
            None
        };

        // Restart budget: three restarts is the default budget — the fourth
        // exhausts it, emits ServerDied, and the pump thread exits.
        let resolved = ResolvedServer {
            spec: config::ServerSpec {
                language: "fakebudget".to_string(),
                argv: vec![script.to_string_lossy().into_owned(), "success".to_string()],
                root_markers: vec![],
                source: ConfigSource::Global,
            },
            root: std::env::temp_dir(),
            executable: None,
        };
        manager.spawn_session(resolved, &tx);
        assert_eq!(wait_ready(&mut rx, 1), 1);
        {
            let cmd = manager
                .sessions
                .get("fakebudget")
                .and_then(|s| s.cmd_tx.clone())
                .unwrap();
            for _ in 0..4 {
                let _ = cmd.try_send(LspCommand::Restart);
            }
        }
        let reason =
            wait_dead(&mut rx, "fakebudget").expect("budget exhaustion must emit ServerDied");
        assert!(!reason.is_empty());

        // Io arm: restart where the workspace root vanished fails the spawn.
        let root_dir = tempfile::tempdir().unwrap();
        let root = root_dir.path().to_path_buf();
        let resolved = ResolvedServer {
            spec: config::ServerSpec {
                language: "fakegone".to_string(),
                argv: vec![script.to_string_lossy().into_owned(), "success".to_string()],
                root_markers: vec![],
                source: ConfigSource::Global,
            },
            root,
            executable: None,
        };
        manager.spawn_session(resolved, &tx);
        assert_eq!(wait_ready(&mut rx, 1), 1);
        drop(root_dir);
        {
            let cmd = manager
                .sessions
                .get("fakegone")
                .and_then(|s| s.cmd_tx.clone())
                .unwrap();
            let _ = cmd.try_send(LspCommand::Restart);
        }
        let reason = wait_dead(&mut rx, "fakegone").expect("vanished cwd must emit ServerDied");
        assert!(!reason.is_empty());

        manager.shutdown_all();
        assert!(manager.sessions.is_empty());
    }
}
