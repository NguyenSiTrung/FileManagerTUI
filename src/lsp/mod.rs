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
pub mod features;
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
    /// Documents this session has (re-)opened, mirroring the server's view.
    docs: features::SyncedDocuments,
    /// Negotiated sync surface; `default` (no sync) until `Ready` lands.
    sync: features::TextSync,
    /// Feature capabilities from initialize.result; empty until `Ready`.
    features: features::ServerFeatures,
}

/// One open editor document known to the manager. The reconcile pass
/// re-derives `language`/`uri` from the current path each poll, so a rename
/// reads as close-on-old + open-on-new without a dedicated hook.
struct TrackedDoc {
    language: String,
    uri: String,
    /// `content_revision` last reflected (or queued) toward the server.
    revision: u64,
}

/// Correlation context for an in-flight feature request — everything the
/// UI needs to route and validate the eventual response.
#[derive(Debug)]
pub struct FeatureRequest {
    pub language: String,
    /// The JSON-RPC method issued ("textDocument/completion", ...).
    pub method: String,
    /// Document the request was issued for.
    pub document: crate::workspace::documents::DocumentId,
    /// URI the request addressed (rename guard on delivery).
    pub uri: String,
    /// `content_revision` at request time — the staleness token.
    pub revision: u64,
}

/// A resolved feature request, queued for the app to consume. Raw result
/// JSON travels untouched — parsing happens in the app, against the
/// document's *current* text.
#[derive(Debug)]
pub enum FeatureResult {
    /// Server returned a result body (which may be `null`).
    Ready {
        req: FeatureRequest,
        result: serde_json::Value,
    },
    /// Server error, malformed JSON, or a manager-side failure.
    Error {
        req: FeatureRequest,
        message: String,
    },
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
    /// Editor documents currently under sync, keyed by stable document id.
    tracked: HashMap<crate::workspace::documents::DocumentId, TrackedDoc>,
    /// In-flight feature requests by JSON-RPC id (manager-allocated, so
    /// responses correlate without a second channel).
    pending_features: HashMap<u64, FeatureRequest>,
    /// Next manager-allocated request id.
    next_request_id: u64,
    /// Resolved requests awaiting app consumption.
    feature_results: VecDeque<FeatureResult>,
}

impl LspManager {
    pub fn new() -> Self {
        Self {
            next_request_id: 1,
            ..Self::default()
        }
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
                        docs: features::SyncedDocuments::default(),
                        sync: features::TextSync::default(),
                        features: features::ServerFeatures::default(),
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
                        docs: features::SyncedDocuments::default(),
                        sync: features::TextSync::default(),
                        features: features::ServerFeatures::default(),
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
                        docs: features::SyncedDocuments::default(),
                        sync: features::TextSync::default(),
                        features: features::ServerFeatures::default(),
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
    pub fn send(&self, language: &str, body: &[u8]) -> bool {
        self.sessions
            .get(language)
            .and_then(|s| s.cmd_tx.as_ref())
            .is_some_and(|tx| tx.try_send(LspCommand::Send(body.to_vec())).is_ok())
    }

    /// Capabilities advertised by a `Ready` session — capability fallback
    /// checks read this before issuing a request.
    /// A live session is Ready or Degraded (degraded = a request timed out
    /// while the process stayed alive — it can still serve requests).
    fn session_live(&self, language: &str) -> Option<&LspSession> {
        let session = self.sessions.get(language)?;
        match session.status {
            SessionStatus::Ready { .. } | SessionStatus::Degraded(_) => Some(session),
            _ => None,
        }
    }

    pub fn ready_features(&self, language: &str) -> Option<features::ServerFeatures> {
        self.session_live(language).map(|s| s.features)
    }

    /// Negotiated position encoding of a `Ready` session — callers convert
    /// cursor/positions through it.
    pub fn ready_encoding(&self, language: &str) -> Option<positions::PositionEncoding> {
        self.session_live(language).map(|s| match s.status {
            SessionStatus::Ready { encoding } => encoding,
            _ => positions::PositionEncoding::default(),
        })
    }

    /// Current tracked record for a document (uri/revision) when under sync.
    pub fn tracked_revision(
        &self,
        id: crate::workspace::documents::DocumentId,
    ) -> Option<(String, u64)> {
        self.tracked.get(&id).map(|t| (t.uri.clone(), t.revision))
    }

    /// Issue a JSON-RPC request through a `Ready` session. Returns the id
    /// that will resolve it; the request context lands in
    /// `pending_features` for correlation. None when the session is not
    /// Ready or the queue cannot accept the body.
    pub fn request_feature(
        &mut self,
        language: &str,
        method: &str,
        params: serde_json::Value,
        document: crate::workspace::documents::DocumentId,
        uri: String,
        revision: u64,
    ) -> Option<u64> {
        self.session_live(language)?;
        let session = self.sessions.get(language)?;
        let id = self.next_request_id;
        self.next_request_id += 1;
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let tx = session.cmd_tx.as_ref()?;
        tx.try_send(LspCommand::Send(body.to_string().into_bytes()))
            .ok()?;
        self.pending_features.insert(
            id,
            FeatureRequest {
                language: language.to_string(),
                method: method.to_string(),
                document,
                uri,
                revision,
            },
        );
        Some(id)
    }

    /// Drain resolved feature results; the app consumes them in its
    /// per-iteration pass.
    pub fn take_feature_results(&mut self) -> Vec<FeatureResult> {
        self.feature_results.drain(..).collect()
    }

    /// Test-only: inject a Ready session with a live command channel so
    /// app-level tests can drive the request/response path.
    #[cfg(test)]
    pub(crate) fn insert_ready_session(
        &mut self,
        language: &str,
        features: features::ServerFeatures,
    ) -> mpsc::Receiver<LspCommand> {
        let (cmd_tx, cmd_rx) = mpsc::sync_channel::<LspCommand>(64);
        self.sessions.insert(
            language.to_string(),
            LspSession {
                resolved: ResolvedServer {
                    spec: config::ServerSpec {
                        language: language.to_string(),
                        argv: vec!["fake".to_string()],
                        root_markers: vec![],
                        source: config::ConfigSource::Global,
                    },
                    root: std::env::temp_dir(),
                    executable: None,
                },
                cmd_tx: Some(cmd_tx),
                status: SessionStatus::Ready {
                    encoding: positions::PositionEncoding::Utf16,
                },
                generation: 0,
                docs: features::SyncedDocuments::default(),
                sync: features::TextSync::default(),
                features,
            },
        );
        cmd_rx
    }

    /// Restart a dead/degraded session within its restart budget.
    pub fn restart(&mut self, language: &str) -> Option<String> {
        let session = self.sessions.get_mut(language)?;
        let tx = session.cmd_tx.as_ref()?;
        tx.try_send(LspCommand::Restart).ok()?;
        session.status = SessionStatus::Starting;
        Some(format!("LSP {language}: restarting"))
    }

    // ── Document sync (Phase 11): didOpen/didChange/didSave/didClose ────────

    /// Wrap params as a JSON-RPC notification and queue it to the session's
    /// pump thread. Returns false when the session cannot be reached.
    fn notify(&self, language: &str, method: &str, params: serde_json::Value) -> bool {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.send(language, body.to_string().as_bytes())
    }

    /// The negotiated sync surface of a session that can accept `did*`
    /// messages right now (Ready + open/close advertised).
    fn ready_sync(
        &self,
        language: &str,
    ) -> Option<(features::TextSync, positions::PositionEncoding)> {
        let session = self.sessions.get(language)?;
        match session.status {
            SessionStatus::Ready { encoding } if session.sync.open_close => {
                Some((session.sync, encoding))
            }
            _ => None,
        }
    }

    /// Send `didOpen` for `uri` when the session is ready; records the
    /// mirror either way so a later `Ready` re-open diffs correctly.
    fn try_open(&mut self, language: &str, uri: &str, text: &str) -> bool {
        if self.ready_sync(language).is_none() {
            return false;
        }
        let session = self.sessions.get_mut(language).unwrap();
        let params = session.docs.did_open(uri, language, text);
        self.notify(language, "textDocument/didOpen", params)
    }

    /// Send `didChange` (or silently advance the mirror for `SyncKind::None`)
    /// and report whether the revision was reflected toward the server.
    fn try_change(&mut self, language: &str, uri: &str, text: &str) -> bool {
        let Some((sync, encoding)) = self.ready_sync(language) else {
            return false;
        };
        let session = self.sessions.get_mut(language).unwrap();
        match session.docs.did_change(uri, text, sync, encoding) {
            Some(params) => self.notify(language, "textDocument/didChange", params),
            // `None` = sync-disabled or identical text: mirrored, nothing owed.
            None => true,
        }
    }

    /// Drop `id`'s tracked record, sending `didClose` only when the owning
    /// session is still live enough to hear it (a dead server needs none).
    fn close_tracked(&mut self, id: crate::workspace::documents::DocumentId) {
        let Some(tracked) = self.tracked.remove(&id) else {
            return;
        };
        let Some(session) = self.sessions.get_mut(&tracked.language) else {
            return;
        };
        let live = matches!(session.status, SessionStatus::Ready { .. });
        if let Some(params) = session.docs.did_close(&tracked.uri) {
            if live {
                self.notify(&tracked.language, "textDocument/didClose", params);
            }
        }
    }

    /// Reconcile the open document set against the tracked table — the
    /// single driver for didOpen/didChange/didClose. `docs` yields
    /// `(id, path, content_revision)` for every open editor document;
    /// `text_of` fetches text lazily, only for docs that need a send.
    ///
    /// Open covers docs that appeared before the handshake or before trust
    /// was granted; a changed `uri`/language (rename) reads as close-old +
    /// open-new; ids absent from `docs` are closed. Read-only S3/binary
    /// previews never reach the document store, so they never appear here.
    pub fn sync_documents<'a, I, F>(&mut self, config: &config::LspConfig, docs: I, mut text_of: F)
    where
        I: Iterator<Item = (crate::workspace::documents::DocumentId, &'a Path, u64)>,
        F: FnMut(crate::workspace::documents::DocumentId) -> Option<String>,
    {
        if !config.enabled() {
            return;
        }
        let mut seen = std::collections::HashSet::new();
        for (id, path, revision) in docs {
            seen.insert(id);
            let uri = features::uri_for_path(path);
            let language = config::language_for_path(path, &config.languages);
            // A rename or a lost language mapping closes under the OLD
            // identity before the new one is considered.
            if let Some(tracked) = self.tracked.get(&id) {
                if Some(&tracked.language) != language.as_ref() || tracked.uri != uri {
                    self.close_tracked(id);
                }
            }
            let Some(language) = language else {
                continue;
            };
            self.tracked.entry(id).or_insert(TrackedDoc {
                language: language.clone(),
                uri: uri.clone(),
                revision,
            });
            let opened = self
                .sessions
                .get(&language)
                .is_some_and(|s| s.docs.is_open(&uri));
            if !opened {
                if let Some(text) = text_of(id) {
                    if self.try_open(&language, &uri, &text) {
                        self.tracked.get_mut(&id).unwrap().revision = revision;
                    }
                }
                continue;
            }
            if self.tracked[&id].revision == revision {
                continue;
            }
            let Some(text) = text_of(id) else {
                continue;
            };
            if self.try_change(&language, &uri, &text) {
                self.tracked.get_mut(&id).unwrap().revision = revision;
            }
        }
        let gone: Vec<_> = self
            .tracked
            .keys()
            .filter(|id| !seen.contains(id))
            .copied()
            .collect();
        for id in gone {
            self.close_tracked(id);
        }
    }

    /// `didSave` for a successfully saved document — only when the session
    /// advertised save support and the document is actually open there.
    #[cfg(test)]
    pub(crate) fn tracked_len(&self) -> usize {
        self.tracked.len()
    }

    pub fn document_saved(&mut self, id: crate::workspace::documents::DocumentId) {
        let Some(tracked) = self.tracked.get(&id) else {
            return;
        };
        let language = tracked.language.clone();
        let uri = tracked.uri.clone();
        let Some(session) = self.sessions.get(&language) else {
            return;
        };
        if !session.sync.save
            || !session.docs.is_open(&uri)
            || !matches!(session.status, SessionStatus::Ready { .. })
        {
            return;
        }
        if let Some(params) = session.docs.did_save(&uri, session.sync.save_include_text) {
            self.notify(&language, "textDocument/didSave", params);
        }
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
                sync,
                features,
            } => {
                session.generation = generation;
                session.sync = sync;
                session.features = features;
                session.status = SessionStatus::Ready { encoding };
                // A restart's new generation owns an empty document table —
                // re-open every doc the mirror still holds (same versions).
                for params in session.docs.reopen_params(language) {
                    let body = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/didOpen",
                        "params": params,
                    });
                    if let Some(tx) = session.cmd_tx.as_ref() {
                        let _ = tx.try_send(LspCommand::Send(body.to_string().into_bytes()));
                    }
                }
                Some(format!("LSP {language}: ready ({})", encoding.as_str()))
            }
            ClientEvent::ServerDied { generation, reason } => {
                session.generation = generation;
                session.status = SessionStatus::Dead(reason.clone());
                session.cmd_tx = None;
                // In-flight requests die with the session — their ids will
                // never resolve; the status note carries the failure.
                self.pending_features.retain(|_, r| r.language != language);
                Some(format!("LSP {language}: stopped ({reason})"))
            }
            ClientEvent::Expired { id, method, .. } => {
                if let Some(req) = self.pending_features.remove(&id) {
                    self.feature_results.push_back(FeatureResult::Error {
                        req,
                        message: format!("{method} timed out"),
                    });
                }
                session.status = SessionStatus::Degraded(format!("{method} timed out"));
                None
            }
            ClientEvent::Response {
                id,
                generation: _,
                outcome,
            } => {
                // Unknown ids are stale (expired, cancelled, pre-restart) —
                // the client already dropped them from its own table.
                if let Some(req) = self.pending_features.remove(&id) {
                    let result = match outcome {
                        Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
                            Ok(result) => FeatureResult::Ready { req, result },
                            Err(_) => FeatureResult::Error {
                                req,
                                message: "malformed response body".to_string(),
                            },
                        },
                        Err(raw) => FeatureResult::Error {
                            req,
                            message: features::sanitize_server_text(&raw, 240),
                        },
                    };
                    self.feature_results.push_back(result);
                }
                None
            }
            ClientEvent::Notification { .. } => {
                // Diagnostics land in Phase 11 Task 3; other notifications
                // need no session-table transition.
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
                docs: features::SyncedDocuments::default(),
                sync: features::TextSync::default(),
                features: features::ServerFeatures::default(),
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

    let root_uri = features::uri_for_path(&cwd);
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
                        match message.get("id") {
                            // Manager-allocated id: honor it so the response
                            // resolves the manager's pending registry.
                            Some(id) if id.is_u64() => {
                                let _ = client.request_with_id(
                                    id.as_u64().unwrap_or_default(),
                                    &method,
                                    message["params"].clone(),
                                );
                            }
                            // A request with a non-integer id still allocates.
                            Some(_) => {
                                let _ = client.request(&method, message["params"].clone());
                            }
                            None => {
                                let _ = client.notify(&method, message["params"].clone());
                            }
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
                docs: features::SyncedDocuments::default(),
                sync: features::TextSync::default(),
                features: features::ServerFeatures::default(),
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
                docs: features::SyncedDocuments::default(),
                sync: features::TextSync::default(),
                features: features::ServerFeatures::default(),
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
                    sync: features::TextSync::default(),
                    features: features::ServerFeatures::default(),
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
        #[rustfmt::skip]
        let Ok(LspCommand::Shutdown) = cmd_rx.try_recv() else { unreachable!("expected Shutdown for the stale session") };
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

    // ── Document sync (Phase 11 Task 1) ────────────────────────────────────

    /// Drive one reconcile pass the way `App::sync_lsp_documents` does.
    fn drive(
        manager: &mut LspManager,
        config: &LspConfig,
        store: &crate::workspace::documents::DocumentStore,
    ) {
        let docs: Vec<_> = store
            .iter()
            .map(|d| (d.id(), d.path().to_path_buf(), d.editor.content_revision()))
            .collect();
        manager.sync_documents(
            config,
            docs.iter().map(|(id, p, r)| (*id, p.as_path(), *r)),
            |id| store.get(id).map(|d| d.text()),
        );
    }

    /// Decode queued `Send` bodies into (method, params) pairs.
    fn drain_sends(rx: &mpsc::Receiver<LspCommand>) -> Vec<(String, serde_json::Value)> {
        let mut out = Vec::new();
        while let Ok(LspCommand::Send(body)) = rx.try_recv() {
            let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
            out.push((
                v["method"].as_str().unwrap().to_string(),
                v["params"].clone(),
            ));
        }
        out
    }

    fn ready_session(
        manager: &mut LspManager,
        language: &str,
        sync: features::TextSync,
    ) -> mpsc::Receiver<LspCommand> {
        let (cmd_tx, cmd_rx) = mpsc::sync_channel::<LspCommand>(64);
        let (language, mut session) = fake_session(language, &["ra"]);
        session.cmd_tx = Some(cmd_tx);
        session.status = SessionStatus::Ready {
            encoding: positions::PositionEncoding::Utf16,
        };
        session.sync = sync;
        manager.sessions.insert(language, session);
        cmd_rx
    }

    const FULL_SYNC: features::TextSync = features::TextSync {
        open_close: true,
        change: features::TextSyncKind::Full,
        save: true,
        save_include_text: true,
    };

    #[test]
    fn sync_opens_changes_saves_and_closes_against_channel() {
        use crate::workspace::documents::{DocumentStore, OpenDisposition};
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn a() {}\n").unwrap();
        let mut manager = LspManager::new();
        let cmd_rx = ready_session(&mut manager, "rust", FULL_SYNC);
        let config = LspConfig::default();
        let mut store = DocumentStore::new();
        let a = store.open(&file, OpenDisposition::Pinned).unwrap();

        // First poll: didOpen with the file's text.
        drive(&mut manager, &config, &store);
        let sent = drain_sends(&cmd_rx);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "textDocument/didOpen");
        assert_eq!(sent[0].1["textDocument"]["version"], 1);
        assert_eq!(sent[0].1["textDocument"]["languageId"], "rust");

        // No revision change → no resend (opened docs are not duplicated).
        drive(&mut manager, &config, &store);
        assert!(drain_sends(&cmd_rx).is_empty());

        // Edit bumps the revision → didChange (full text under Full sync).
        store.get_mut(a).unwrap().editor.insert_text("x").unwrap();
        drive(&mut manager, &config, &store);
        let sent = drain_sends(&cmd_rx);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "textDocument/didChange");
        assert_eq!(sent[0].1["textDocument"]["version"], 2);
        assert!(sent[0].1["contentChanges"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with('x'));

        // Save flows only when the session advertised it (FULL_SYNC did).
        manager.document_saved(a);
        let sent = drain_sends(&cmd_rx);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "textDocument/didSave");
        assert!(sent[0].1["textDocument"]["text"]
            .as_str()
            .unwrap()
            .starts_with('x'));

        // Closing the document emits didClose and drops the tracked record.
        store.discard_and_close(a).unwrap();
        drive(&mut manager, &config, &store);
        let sent = drain_sends(&cmd_rx);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "textDocument/didClose");
        assert!(manager.tracked.is_empty());
        assert!(!manager.sessions["rust"]
            .docs
            .is_open(&features::uri_for_path(
                store.get(a).map(|d| d.path()).unwrap_or(&file)
            )));
    }

    #[test]
    fn sync_skips_disabled_unmapped_missing_text_and_not_ready_sends() {
        use crate::workspace::documents::{DocumentStore, OpenDisposition};
        let dir = tempfile::tempdir().unwrap();
        let file_rs = dir.path().join("a.rs");
        let file_xyz = dir.path().join("a.unknownext");
        std::fs::write(&file_rs, "fn a() {}\n").unwrap();
        std::fs::write(&file_xyz, "??\n").unwrap();
        let mut manager = LspManager::new();
        let cmd_rx = ready_session(&mut manager, "rust", FULL_SYNC);
        let mut store = DocumentStore::new();
        let a = store.open(&file_rs, OpenDisposition::Pinned).unwrap();
        let u = store.open(&file_xyz, OpenDisposition::Pinned).unwrap();
        // Disabled config: reconcile is a no-op — nothing tracked or sent.
        let mut config = LspConfig {
            enabled: Some(false),
            ..LspConfig::default()
        };
        drive(&mut manager, &config, &store);
        assert!(manager.tracked.is_empty());
        assert!(drain_sends(&cmd_rx).is_empty());
        config.enabled = None;

        // Unknown extension is skipped before tracking; .rs opens normally.
        drive(&mut manager, &config, &store);
        assert!(manager.tracked.contains_key(&a));
        assert!(!manager.tracked.contains_key(&u));
        assert_eq!(drain_sends(&cmd_rx).len(), 1);

        // Missing text: a poll item the text provider cannot produce stays
        // tracked but unopened, while a sibling with a real diff still sends.
        let b_file = dir.path().join("b.rs");
        std::fs::write(&b_file, "fn b() {}\n").unwrap();
        let b = store.open(&b_file, OpenDisposition::Pinned).unwrap();
        let a_path = store.get(a).unwrap().path().to_path_buf();
        let b_path = store.get(b).unwrap().path().to_path_buf();
        let b_uri = features::uri_for_path(&b_path);
        store.get_mut(a).unwrap().editor.insert_text("x").unwrap();
        let a_rev = store.get(a).unwrap().editor.content_revision();
        manager.sync_documents(
            &config,
            [(a, a_path.as_path(), a_rev), (b, b_path.as_path(), 0u64)].into_iter(),
            |id| {
                if id == b {
                    None
                } else {
                    store.get(id).map(|d| d.text())
                }
            },
        );
        assert!(manager.tracked.contains_key(&b));
        assert!(!manager.sessions["rust"].docs.is_open(&b_uri));
        let sent = drain_sends(&cmd_rx);
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(sent[0].0, "textDocument/didChange");

        // The change path defers too: `a` owes a diff but its text is
        // unavailable, so the revision stays unsynced; `b` opens normally.
        store.get_mut(a).unwrap().editor.insert_text("y").unwrap();
        let a_rev = store.get(a).unwrap().editor.content_revision();
        let stale = manager.tracked[&a].revision;
        manager.sync_documents(
            &config,
            [(a, a_path.as_path(), a_rev), (b, b_path.as_path(), 0u64)].into_iter(),
            |id| {
                if id == a {
                    None
                } else {
                    store.get(id).map(|d| d.text())
                }
            },
        );
        assert_eq!(manager.tracked[&a].revision, stale);
        let sent = drain_sends(&cmd_rx);
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(sent[0].0, "textDocument/didOpen");
        assert_eq!(sent[0].1["textDocument"]["uri"], b_uri);
        store.discard_and_close(b).unwrap();

        // Session gone not-ready: a change diffs but cannot leave.
        manager.sessions.get_mut("rust").unwrap().status = SessionStatus::Starting;
        let rev2 = store.get(a).unwrap().editor.content_revision() + 1;
        store.get_mut(a).unwrap().editor.insert_text("y").unwrap();
        let rev3 = store.get(a).unwrap().editor.content_revision();
        assert_eq!(rev3, rev2);
        manager.sync_documents(&config, [(a, a_path.as_path(), rev3)].into_iter(), |id| {
            store.get(id).map(|d| d.text())
        });
        assert!(drain_sends(&cmd_rx).is_empty());

        // SyncKind::None mirrors the change without emitting a notification.
        let session = manager.sessions.get_mut("rust").unwrap();
        session.status = SessionStatus::Ready {
            encoding: positions::PositionEncoding::Utf16,
        };
        session.sync = features::TextSync {
            open_close: true,
            change: features::TextSyncKind::None,
            save: false,
            save_include_text: false,
        };
        store.get_mut(a).unwrap().editor.insert_text("z").unwrap();
        drive(&mut manager, &config, &store);
        assert!(drain_sends(&cmd_rx).is_empty());

        // close_tracked early returns: unknown id; session present but the
        // uri was never opened (didClose stays silent); missing session.
        manager.close_tracked(u);
        let u_uri = features::uri_for_path(&file_xyz);
        manager.tracked.insert(
            u,
            TrackedDoc {
                language: "rust".into(),
                uri: u_uri.clone(),
                revision: 0,
            },
        );
        manager.close_tracked(u);
        assert!(!manager.tracked.contains_key(&u));
        assert!(drain_sends(&cmd_rx).is_empty());
        manager.sessions.remove("rust");
        manager.close_tracked(a);
        assert!(!manager.tracked.contains_key(&a));

        // document_saved early returns: untracked id; then tracked-but-dead.
        manager.document_saved(u);
        let mut manager2 = LspManager::new();
        let _rx2 = ready_session(&mut manager2, "rust", FULL_SYNC);
        manager2.sessions.get_mut("rust").unwrap().status = SessionStatus::Starting;
        let a2 = store.get(a).unwrap().id();
        manager2.tracked.insert(
            a2,
            TrackedDoc {
                language: "rust".into(),
                uri: features::uri_for_path(store.get(a).unwrap().path()),
                revision: 0,
            },
        );
        manager2.document_saved(a2);
        assert!(drain_sends(&_rx2).is_empty());

        // And with the owning session itself removed.
        manager2.sessions.remove("rust");
        manager2.document_saved(a2);
        assert!(drain_sends(&_rx2).is_empty());
    }

    #[test]
    fn sync_defers_open_until_ready_and_skips_unsupported() {
        use crate::workspace::documents::{DocumentStore, OpenDisposition};
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn a() {}\n").unwrap();
        let mut manager = LspManager::new();
        let mut store = DocumentStore::new();
        let a = store.open(&file, OpenDisposition::Pinned).unwrap();
        let config = LspConfig::default();

        // Session still Starting: tracked but nothing sent, no open.
        let (cmd_tx, cmd_rx) = mpsc::sync_channel::<LspCommand>(64);
        let (language, mut session) = fake_session("rust", &["ra"]);
        session.cmd_tx = Some(cmd_tx);
        manager.sessions.insert(language, session);
        drive(&mut manager, &config, &store);
        assert!(drain_sends(&cmd_rx).is_empty());
        assert_eq!(manager.tracked.len(), 1);

        // Ready + no open_close support → still nothing.
        let session = manager.sessions.get_mut("rust").unwrap();
        session.status = SessionStatus::Ready {
            encoding: positions::PositionEncoding::Utf16,
        };
        session.sync = features::TextSync::default();
        drive(&mut manager, &config, &store);
        assert!(drain_sends(&cmd_rx).is_empty());

        // Upgrading sync to Full opens the already-tracked document.
        manager.sessions.get_mut("rust").unwrap().sync = FULL_SYNC;
        drive(&mut manager, &config, &store);
        let sent = drain_sends(&cmd_rx);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "textDocument/didOpen");

        // didSave is gated by the advertised capability.
        manager.sessions.get_mut("rust").unwrap().sync.save = false;
        manager.document_saved(a);
        assert!(drain_sends(&cmd_rx).is_empty());
    }

    #[test]
    fn sync_rename_closes_old_uri_and_dead_session_sends_nothing() {
        use crate::workspace::documents::{DocumentStore, OpenDisposition};
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        let renamed = dir.path().join("renamed.rs");
        std::fs::write(&file, "fn a() {}\n").unwrap();
        let mut manager = LspManager::new();
        let cmd_rx = ready_session(&mut manager, "rust", FULL_SYNC);
        let config = LspConfig::default();
        let mut store = DocumentStore::new();
        let a = store.open(&file, OpenDisposition::Pinned).unwrap();
        drive(&mut manager, &config, &store);
        assert_eq!(drain_sends(&cmd_rx).len(), 1);

        // Rename on disk + store commit: poll observes the new path and
        // emits didClose(old uri) + didOpen(new uri) in order.
        std::fs::rename(&file, &renamed).unwrap();
        let canon = std::fs::canonicalize(&renamed).unwrap();
        store.commit_rename(vec![(a, canon.clone())]);
        drive(&mut manager, &config, &store);
        let sent = drain_sends(&cmd_rx);
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[0].0, "textDocument/didClose");
        assert_eq!(
            sent[0].1["textDocument"]["uri"],
            features::uri_for_path(&file)
        );
        assert_eq!(sent[1].0, "textDocument/didOpen");
        assert_eq!(
            sent[1].1["textDocument"]["uri"],
            features::uri_for_path(&canon)
        );

        // A dead session sends nothing — didClose is swallowed, not queued.
        store.discard_and_close(a).unwrap();
        manager.sessions.get_mut("rust").unwrap().status = SessionStatus::Dead("x".into());
        drive(&mut manager, &config, &store);
        assert!(drain_sends(&cmd_rx).is_empty());
        assert!(manager.tracked.is_empty());
    }

    /// The pinned scenario: a real fake-server transcript over two unsaved
    /// documents exercising edit/paste/undo, save, close, rename, external
    /// reload, and a server restart — versions monotonic per generation,
    /// server-side mirror equal to the active document text.
    #[test]
    fn document_sync_transcript_matches_active_documents_end_to_end() {
        use crate::workspace::documents::{DocumentStore, OpenDisposition};
        use std::time::Instant;
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join("fake-lsp-server.py");
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("transcript.json");
        let a_path = dir.path().join("a.rs");
        let b_path = dir.path().join("b.rs");
        std::fs::write(&a_path, "fn a() {}\n").unwrap();
        std::fs::write(&b_path, "fn b() {}\n").unwrap();

        let mut manager = LspManager::new();
        let (tx, mut rx) = crate::event::event_channel(crate::event::TransportLimits::default());
        let resolved = ResolvedServer {
            spec: config::ServerSpec {
                language: "rust".to_string(),
                argv: vec![
                    script.to_string_lossy().into_owned(),
                    "sync".to_string(),
                    transcript.to_string_lossy().into_owned(),
                ],
                root_markers: vec![],
                source: ConfigSource::Global,
            },
            root: dir.path().to_path_buf(),
            executable: None,
        };
        manager.spawn_session(resolved, &tx);

        // Route Lsp events into the manager exactly like main.rs does.
        let wait_ready =
            |manager: &mut LspManager, rx: &mut crate::event::EventReceiver, n: u32| {
                let deadline = Instant::now() + Duration::from_secs(15);
                let mut seen = 0u32;
                while Instant::now() < deadline && seen < n {
                    match rx.try_recv() {
                        Ok(crate::event::Event::Lsp {
                            language,
                            generation,
                            event,
                        }) => {
                            let is_ready = matches!(event, ClientEvent::Ready { .. });
                            manager.handle_event(&language, generation, event);
                            if is_ready {
                                seen += 1;
                            }
                        }
                        _ => {
                            std::thread::sleep(Duration::from_millis(25));
                        }
                    }
                }
                seen
            };
        assert_eq!(wait_ready(&mut manager, &mut rx, 1), 1);

        let config = LspConfig::default();
        let mut store = DocumentStore::new();
        let a = store.open(&a_path, OpenDisposition::Pinned).unwrap();
        let b = store.open(&b_path, OpenDisposition::Pinned).unwrap();

        // Two documents, both edited in memory (unsaved — text never touches
        // disk; the server learns it only through didOpen/didChange).
        drive(&mut manager, &config, &store);
        store.get_mut(a).unwrap().editor.insert_text("aaa").unwrap();
        store
            .get_mut(b)
            .unwrap()
            .editor
            .insert_text("multi\nline")
            .unwrap();
        drive(&mut manager, &config, &store);
        store.get_mut(a).unwrap().editor.undo();
        drive(&mut manager, &config, &store);

        // Save a → didSave; rename a → close+open; reload it from disk.
        store.get_mut(a).unwrap().editor.save().unwrap();
        manager.document_saved(a);
        let a2_path = dir.path().join("a2.rs");
        std::fs::rename(&a_path, &a2_path).unwrap();
        let canon = std::fs::canonicalize(&a2_path).unwrap();
        store.commit_rename(vec![(a, canon.clone())]);
        drive(&mut manager, &config, &store);
        std::fs::write(&a2_path, "fn a() { /* externally reloaded */ }\n").unwrap();
        store.reload(a).unwrap();
        drive(&mut manager, &config, &store);

        // Close b → didClose. Then restart: the new generation re-opens a.
        store.discard_and_close(b).unwrap();
        drive(&mut manager, &config, &store);
        // Wait for the last in-flight notification before killing the fake:
        // transport shutdown discards queued writes (kill is not a flush).
        {
            let dl = Instant::now() + Duration::from_secs(10);
            while Instant::now() < dl {
                let closes = std::fs::read_to_string(&transcript)
                    .ok()
                    .map(|s| s.matches("textDocument/didClose").count())
                    .unwrap_or(0);
                if closes >= 2 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        manager.restart("rust");
        assert!(wait_ready(&mut manager, &mut rx, 1) >= 1);
        drive(&mut manager, &config, &store);

        // The fake persists its transcript after every message (kill-safe).
        // Poll until the restart's second didOpen lands — it is ordered
        // strictly after every earlier send, so it proves all arrived.
        let a2_uri = features::uri_for_path(&canon);
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut report = None;
        loop {
            if Instant::now() >= deadline || report.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
            let parsed = std::fs::read_to_string(&transcript)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
            let opens = parsed
                .as_ref()
                .and_then(|candidate| candidate["log"].as_array())
                .map(|log| {
                    log.iter()
                        .filter(|e| {
                            e["method"] == "textDocument/didOpen"
                                && e["uri"] == serde_json::json!(a2_uri)
                        })
                        .count()
                })
                .unwrap_or(0);
            if opens >= 2 {
                report = parsed;
            }
        }
        let report = report.expect("transcript never showed the restart re-open");
        manager.shutdown_all();

        // Per-URI, per-generation versions strictly increase.
        let log = report["log"].as_array().unwrap();
        let a_uri = features::uri_for_path(&a_path);
        let b_uri = features::uri_for_path(&b_path);
        for uri in [&a_uri, &a2_uri, &b_uri] {
            let mut sent_versions: Vec<i64> = Vec::new();
            let mut generational: Vec<Vec<i64>> = vec![Vec::new()];
            for entry in log.iter().filter(|e| e["uri"] == *uri) {
                let method = entry["method"].as_str().unwrap();
                let version = entry["version"].as_i64();
                if method == "textDocument/didOpen" {
                    generational.push(Vec::new());
                }
                if let Some(v) = version {
                    sent_versions.push(v);
                    generational.last_mut().unwrap().push(v);
                }
            }
            for window in sent_versions.windows(2) {
                let _ = window;
            }
            for segment in &generational {
                assert!(
                    segment.windows(2).all(|v| v[0] < v[1]),
                    "versions for {uri} must strictly increase per generation: {generational:?}"
                );
            }
            assert!(
                sent_versions.windows(2).all(|v| v[0] <= v[1]),
                "versions for {uri} never rewind: {sent_versions:?}"
            );
        }

        // Server-side mirror equals the live document text — the pinned
        // assert_eq!(server_text, active_document_text).
        let server_text = report["docs"][&a2_uri].as_str().unwrap().to_string();
        let active_document_text = store.get(a).unwrap().text();
        assert_eq!(server_text, active_document_text);
        // b was closed → gone from the server's table; a's old uri too.
        assert!(report["docs"].get(&b_uri).is_none());
        assert!(report["docs"].get(&a_uri).is_none());
        // The transcript exercised every required surface.
        let methods: Vec<&str> = log.iter().map(|e| e["method"].as_str().unwrap()).collect();
        for want in [
            "textDocument/didOpen",
            "textDocument/didChange",
            "textDocument/didSave",
            "textDocument/didClose",
        ] {
            assert!(methods.contains(&want), "missing {want} in {methods:?}");
        }
        // Post-restart re-open: a2 opened exactly twice (once per
        // generation), never more — no duplicated events.
        let a2_opens = methods
            .iter()
            .zip(log.iter())
            .filter(|(m, e)| **m == "textDocument/didOpen" && e["uri"] == a2_uri)
            .count();
        assert_eq!(a2_opens, 2, "reopen after restart must not duplicate");
    }

    // ── Language-feature routing (Phase 11 Task 2) ────────────────────────

    #[test]
    fn request_feature_routes_and_resolves_via_pending() {
        use crate::workspace::documents::{DocumentStore, OpenDisposition};
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.rs");
        std::fs::write(&file, "fn f() {}\n").unwrap();
        let mut store = DocumentStore::new();
        let doc = store.open(&file, OpenDisposition::Pinned).unwrap();

        let mut manager = LspManager::new();
        let rx = ready_session(&mut manager, "rust", FULL_SYNC);
        manager.sessions.get_mut("rust").unwrap().features = features::ServerFeatures {
            completion: true,
            hover: true,
            ..Default::default()
        };
        assert_eq!(
            manager.ready_features("rust"),
            Some(features::ServerFeatures {
                completion: true,
                hover: true,
                ..Default::default()
            })
        );
        assert_eq!(
            manager.ready_encoding("rust"),
            Some(positions::PositionEncoding::Utf16)
        );

        // Request → Send body carries the manager-allocated id verbatim so
        // the response can resolve the pending entry.
        let id = manager
            .request_feature(
                "rust",
                "textDocument/completion",
                serde_json::json!({"x": 1}),
                doc,
                "file:///f.rs".to_string(),
                7,
            )
            .unwrap();
        #[rustfmt::skip]
        let Ok(LspCommand::Send(body)) = rx.try_recv() else { unreachable!("expected Send") };
        let body = serde_json::from_slice::<serde_json::Value>(&body).unwrap();
        assert_eq!(body["id"], id);
        assert_eq!(body["method"], "textDocument/completion");
        assert_eq!(body["params"], serde_json::json!({"x": 1}));

        // Response resolves the pending entry into a Ready result; unknown
        // ids drop silently.
        assert!(manager
            .handle_event(
                "rust",
                0,
                ClientEvent::Response {
                    id,
                    generation: 0,
                    outcome: Ok("{\"items\": []}".to_string()),
                },
            )
            .is_none());
        manager.handle_event(
            "rust",
            0,
            ClientEvent::Response {
                id: 9999,
                generation: 0,
                outcome: Ok("null".to_string()),
            },
        );
        let results = manager.take_feature_results();
        assert_eq!(results.len(), 1);
        #[rustfmt::skip]
        let FeatureResult::Ready { req, result } = &results[0] else { unreachable!("expected Ready") };
        assert_eq!(req.revision, 7);
        assert_eq!(req.uri, "file:///f.rs");
        assert_eq!(result["items"], serde_json::json!([]));
        assert!(manager.take_feature_results().is_empty());

        // Error outcome → FeatureResult::Error, sanitized.
        let id = manager
            .request_feature(
                "rust",
                "textDocument/hover",
                serde_json::json!({}),
                doc,
                "file:///f.rs".to_string(),
                8,
            )
            .unwrap();
        let _ = rx.try_recv();
        manager.handle_event(
            "rust",
            0,
            ClientEvent::Response {
                id,
                generation: 0,
                outcome: Err("internal failure".to_string()),
            },
        );
        #[rustfmt::skip]
        let FeatureResult::Error { req, message } = manager.take_feature_results().remove(0) else { unreachable!("expected Error") };
        assert_eq!(req.method, "textDocument/hover");
        assert_eq!(message, "internal failure");

        // Expired request → pending dropped + Error result + session degrades.
        let id = manager
            .request_feature(
                "rust",
                "textDocument/hover",
                serde_json::json!({}),
                doc,
                "file:///f.rs".to_string(),
                8,
            )
            .unwrap();
        let _ = rx.try_recv();
        manager.handle_event(
            "rust",
            0,
            ClientEvent::Expired {
                id,
                method: "textDocument/hover".to_string(),
            },
        );
        #[rustfmt::skip]
        let FeatureResult::Error { message, .. } = manager.take_feature_results().remove(0) else { unreachable!("expected Error") };
        assert!(message.contains("timed out"));
        assert!(matches!(
            manager.session_status("rust"),
            Some(SessionStatus::Degraded(_))
        ));

        // ServerDied drops that session's in-flight requests; another
        // session's pending entries survive.
        let _id = manager
            .request_feature(
                "rust",
                "textDocument/hover",
                serde_json::json!({}),
                doc,
                "file:///f.rs".to_string(),
                9,
            )
            .unwrap();
        let _ = rx.try_recv();
        manager.handle_event(
            "rust",
            1,
            ClientEvent::ServerDied {
                generation: 1,
                reason: "boom".to_string(),
            },
        );
        assert!(manager.take_feature_results().is_empty());
        assert!(manager
            .request_feature(
                "rust",
                "textDocument/hover",
                serde_json::json!({}),
                doc,
                "file:///f.rs".to_string(),
                10,
            )
            .is_none()); // dead session cannot take requests
    }

    /// The pinned scenario: a real fake server answers a completion request
    /// end-to-end — initialize advertises the provider, the request crosses
    /// stdio, and the response resolves through the pending registry.
    #[test]
    fn feature_request_round_trips_against_fake_server() {
        use std::time::Instant;
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join("fake-lsp-server.py");
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("transcript.json");
        let opts = serde_json::json!({
            "capabilities": {
                "completionProvider": {"triggerCharacters": ["."]},
                "hoverProvider": true,
            },
            "features": {
                "textDocument/completion": [
                    {"label": "complete_me", "kind": 3, "detail": "fn complete_me()"},
                    {"label": "companion", "insertText": "companion()"},
                ],
                "textDocument/hover": {"contents": {"kind": "plaintext",
                    "value": "hover text"}},
            },
        });
        let mut manager = LspManager::new();
        let (tx, mut rx) = crate::event::event_channel(crate::event::TransportLimits::default());
        let resolved = ResolvedServer {
            spec: config::ServerSpec {
                language: "rust".to_string(),
                argv: vec![
                    script.to_string_lossy().into_owned(),
                    "sync".to_string(),
                    transcript.to_string_lossy().into_owned(),
                    opts.to_string(),
                ],
                root_markers: vec![],
                source: ConfigSource::Global,
            },
            root: dir.path().to_path_buf(),
            executable: None,
        };
        manager.spawn_session(resolved, &tx);

        // Ready, then a completion request for a synced document.
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut is_ready = false;
        while !is_ready && Instant::now() <= deadline {
            match rx.try_recv() {
                Ok(crate::event::Event::Lsp {
                    language,
                    generation,
                    event,
                }) => {
                    is_ready = matches!(event, ClientEvent::Ready { .. });
                    manager.handle_event(&language, generation, event);
                }
                _ => std::thread::sleep(Duration::from_millis(25)),
            }
        }
        assert!(is_ready);
        assert_eq!(
            manager.ready_features("rust"),
            Some(features::ServerFeatures {
                completion: true,
                hover: true,
                ..Default::default()
            })
        );

        use crate::workspace::documents::{DocumentStore, OpenDisposition};
        let path = dir.path().join("f.rs");
        std::fs::write(&path, "let c = comp\n").unwrap();
        let mut store = DocumentStore::new();
        let doc = store.open(&path, OpenDisposition::Pinned).unwrap();
        drive(&mut manager, &LspConfig::default(), &store);
        let uri = features::uri_for_path(&std::fs::canonicalize(&path).unwrap());
        let req_id = manager
            .request_feature(
                "rust",
                "textDocument/completion",
                serde_json::json!({"textDocument": {"uri": uri}}),
                doc,
                uri.clone(),
                0,
            )
            .unwrap();
        assert!(req_id > 0);

        // The response arrives as an Lsp event → FeatureResult::Ready.
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut found = None;
        while found.is_none() && Instant::now() <= deadline {
            match rx.try_recv() {
                Ok(crate::event::Event::Lsp {
                    language,
                    generation,
                    event,
                }) => {
                    manager.handle_event(&language, generation, event);
                    let results = manager.take_feature_results();
                    if !results.is_empty() {
                        found = Some(results);
                    }
                }
                _ => std::thread::sleep(Duration::from_millis(25)),
            }
        }
        assert!(found.is_some());
        let results = found.unwrap();
        #[rustfmt::skip]
        let FeatureResult::Ready { req, result } = &results[0] else { unreachable!("expected Ready") };
        assert_eq!(req.document, doc);
        assert_eq!(req.method, "textDocument/completion");
        let document = store.get(doc).unwrap();
        let items = features::parse_completion(
            result,
            &document.editor.buffer,
            positions::PositionEncoding::Utf16,
        )
        .unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "complete_me");
        assert_eq!(items[0].kind, Some(3));

        // The transcript shows the request crossed the pipe verbatim.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut ready = false;
        while !ready && Instant::now() <= deadline {
            std::thread::sleep(Duration::from_millis(40));
            ready = std::fs::read_to_string(&transcript)
                .is_ok_and(|r| r.contains("textDocument/completion"));
        }
        assert!(ready);
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&transcript).unwrap()).unwrap();
        let methods: Vec<_> = report["log"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["method"].as_str().unwrap())
            .collect();
        assert!(methods.contains(&"textDocument/completion"));

        // A Send whose JSON-RPC id is a string still forwards through the
        // client's own allocation (covers the non-u64 Send arm): the server
        // sees a normal request and the transcript logs it.
        let cmd_tx = manager
            .sessions
            .get("rust")
            .unwrap()
            .cmd_tx
            .clone()
            .unwrap();
        cmd_tx
            .try_send(LspCommand::Send(
                br#"{"jsonrpc":"2.0","id":"s-1","method":"workspace/symbol","params":{"query":"x"}}"#
                    .to_vec(),
            ))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut ready = false;
        while !ready && Instant::now() <= deadline {
            std::thread::sleep(Duration::from_millis(40));
            ready =
                std::fs::read_to_string(&transcript).is_ok_and(|r| r.contains("workspace/symbol"));
        }
        assert!(ready);

        // A Degraded session still answers encoding/feature lookups with
        // safe defaults — the live gate keeps requests flowing.
        manager.sessions.get_mut("rust").unwrap().status =
            SessionStatus::Degraded("probe".to_string());
        assert_eq!(
            manager.ready_encoding("rust"),
            Some(positions::PositionEncoding::default())
        );
        assert!(manager.ready_features("rust").is_some());

        manager.shutdown_all();
    }
}
