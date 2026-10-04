//! LSP client lifecycle over the bounded stdio transport.
//!
//! `Client` owns one server generation at a time: a spawned `LspTransport`,
//! the pending-request table, the negotiated position encoding, and the
//! document versions the server has seen. It enforces the safety contract —
//! byte-bounded frames (transport), monotonically increasing request IDs,
//! deadlines on every pending request, an explicit `MethodNotFound` reply to
//! every server-initiated request (no workspace edits or execute-commands are
//! ever auto-applied), a bounded shutdown (`shutdown`/`exit`, then kill), and
//! a restart budget so a crashing server cannot loop forever.
//!
//! Every emitted `ClientEvent` is generation-tagged: a response or
//! notification arriving after a restart is recognizably stale and must be
//! ignored by consumers (see `accepts`).

// Staged surface: Task 4 config/trust and Task 11 features consume this
// module; only unit tests touch it until then. Drop the allow on wiring.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::positions::PositionEncoding;
use super::transport::{Inbound, LspTransport};

/// The byte-level contract a server generation speaks. `LspTransport`
/// implements it for a real child; tests implement it over scripts.
pub trait LspIo: Send {
    /// Queue one encoded JSON-RPC body for transmission. Must never block
    /// indefinitely — bounded transports return `WouldBlock`.
    fn send(&mut self, body: &[u8]) -> io::Result<()>;
    /// Wait up to `timeout` for the next inbound item; `None` on timeout.
    fn poll(&mut self, timeout: Duration) -> Option<Inbound>;
    /// Bounded tail of the child's stderr (diagnostics only).
    fn stderr_tail(&self) -> Vec<String> {
        Vec::new()
    }
    /// Bounded teardown: may kill the child. Must always return.
    fn shutdown(&mut self);
}

impl LspIo for LspTransport {
    fn send(&mut self, body: &[u8]) -> io::Result<()> {
        LspTransport::send(self, body)
    }

    fn poll(&mut self, timeout: Duration) -> Option<Inbound> {
        LspTransport::recv_timeout(self, timeout)
    }

    fn stderr_tail(&self) -> Vec<String> {
        LspTransport::stderr_tail(self)
    }

    fn shutdown(&mut self) {
        LspTransport::shutdown(self);
    }
}

/// Lifecycle of one server generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientState {
    /// `initialize` sent, awaiting the result.
    Starting,
    /// `initialize`/`initialized` handshake completed.
    Ready,
    /// The transport ended (EOF, crash, or corrupt protocol).
    Dead,
    /// `shutdown`/`exit` completed at the client's request.
    Closed,
}

/// What a `pump` call observed. `generation` identifies which server process
/// produced it — a restart bumps the generation, and events carrying an older
/// one are stale results that consumers must drop.
#[derive(Debug)]
pub enum ClientEvent {
    /// Handshake completed for `generation`.
    Ready { generation: u64 },
    /// A response resolved a pending request. `result`/`error` are the raw
    /// JSON bodies from the server.
    Response {
        id: u64,
        generation: u64,
        /// `Ok(result_json)` or `Err(error_json)` from the server.
        outcome: Result<String, String>,
    },
    /// A server-to-client notification.
    Notification {
        generation: u64,
        method: String,
        params: String,
    },
    /// A pending request exceeded its deadline and was cancelled.
    Expired { id: u64, method: String },
    /// The transport ended without a graceful shutdown.
    ServerDied { generation: u64, reason: String },
}

/// Tunables — tests shrink the deadlines; defaults are production values.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    /// How long a request may stay pending before `$/cancelRequest` fires.
    pub request_timeout: Duration,
    /// How long `shutdown` waits for the server's reply before exiting anyway.
    pub shutdown_timeout: Duration,
    /// Maximum automatic restarts. A crash loop stops at the budget.
    pub restart_budget: u32,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(10),
            shutdown_timeout: Duration::from_secs(2),
            restart_budget: 3,
        }
    }
}

/// Errors the lifecycle surfaces. I/O failures pass through unwrapped so the
/// caller can show the server's own message.
#[derive(Debug)]
pub enum ClientError {
    Io(io::Error),
    /// Restart would exceed the configured budget.
    RestartBudgetExhausted,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "LSP transport error: {e}"),
            Self::RestartBudgetExhausted => write!(f, "LSP restart budget exhausted"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<io::Error> for ClientError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// The shutdown handshake report — did the server answer `shutdown`, and did
/// teardown happen within the deadline?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownReport {
    pub shutdown_replied: bool,
    pub exit_sent: bool,
}

struct PendingRequest {
    method: String,
    deadline: Instant,
}

/// One language-server session. Independent of `App` — feed it `poll` slices
/// and read `ClientEvent`s.
pub struct Client {
    io: Box<dyn LspIo>,
    generation: u64,
    next_id: u64,
    pending: HashMap<u64, PendingRequest>,
    state: ClientState,
    encoding: PositionEncoding,
    /// Highest document version issued per URI — responses bearing an older
    /// version are stale (the document moved on while the request was in
    /// flight).
    doc_versions: HashMap<String, i64>,
    /// Highest document version issued anywhere — backs `accepts`.
    latest_doc_version: i64,
    options: ClientOptions,
}

impl Client {
    /// Spawn `argv` (executable + args, no shell) in `cwd` and begin the
    /// `initialize` handshake for `root_uri`.
    pub fn spawn(
        argv: &[String],
        cwd: &Path,
        root_uri: &str,
        options: ClientOptions,
    ) -> Result<Self, ClientError> {
        let transport = LspTransport::spawn(argv, cwd)?;
        let mut client = Self::over(Box::new(transport), options);
        client.begin_initialize(root_uri)?;
        Ok(client)
    }

    /// Wrap any `LspIo` (test transports drive deterministic peers).
    /// The client starts `Starting`; call `begin_initialize`.
    pub fn over(io: Box<dyn LspIo>, options: ClientOptions) -> Self {
        Self {
            io,
            generation: 0,
            next_id: 1,
            pending: HashMap::new(),
            state: ClientState::Starting,
            encoding: PositionEncoding::default(),
            doc_versions: HashMap::new(),
            latest_doc_version: 0,
            options,
        }
    }

    pub fn state(&self) -> ClientState {
        self.state
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Position encoding negotiated at `initialize` (UTF-16 when the server
    /// does not declare one — the only protocol fallback).
    pub fn encoding(&self) -> PositionEncoding {
        self.encoding
    }

    /// Bounded tail of server stderr for status/diagnostics display.
    pub fn stderr_tail(&self) -> Vec<String> {
        self.io.stderr_tail()
    }

    /// Record the version this client issued for `uri`. Responses stamped
    /// with a lower version are stale and must be dropped.
    pub fn note_document_version(&mut self, uri: &str, version: i64) {
        self.doc_versions.insert(uri.to_string(), version);
        if version > self.latest_doc_version {
            self.latest_doc_version = version;
        }
    }

    /// Staleness predicate: only results from the *current* server generation
    /// at the *current* document version are live. `accepts` uses the global
    /// latest version; `accepts_document` the per-URI one.
    pub fn accepts(&self, server_generation: u64, document_version: i64) -> bool {
        server_generation == self.generation && document_version == self.latest_doc_version
    }

    pub fn accepts_document(
        &self,
        uri: &str,
        server_generation: u64,
        document_version: i64,
    ) -> bool {
        server_generation == self.generation
            && self
                .doc_versions
                .get(uri)
                .is_some_and(|v| *v == document_version)
    }

    /// Send `initialize` and mark the handshake pending.
    pub fn begin_initialize(&mut self, root_uri: &str) -> Result<u64, ClientError> {
        let params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "general": {
                    "positionEncodings": ["utf-8", "utf-16", "utf-32"],
                },
                "workspace": {
                    // Explicitly unsupported: the server must not drive file
                    // edits or command execution through this client.
                    "applyEdit": false,
                    "executeCommand": {"dynamicRegistration": false},
                },
                "textDocument": {
                    "synchronization": {"dynamicRegistration": false},
                    "completion": {"completionItem": {"snippetSupport": false}},
                    "hover": {},
                    "definition": {},
                    "references": {},
                    "documentSymbol": {},
                    "publishDiagnostics": {},
                },
            },
            "clientInfo": {"name": "fm"},
        });
        self.request("initialize", params)
    }

    /// Drive `poll` until `Ready`, `Dead`, or `deadline`. Returns all events.
    pub fn initialize_blocking(&mut self, deadline: Duration) -> Vec<ClientEvent> {
        let started = Instant::now();
        let mut events = Vec::new();
        while started.elapsed() < deadline && self.state == ClientState::Starting {
            events.extend(self.pump(Duration::from_millis(20)));
        }
        events
    }

    /// Issue a request. Returns the JSON-RPC id that will resolve it.
    pub fn request(&mut self, method: &str, params: Value) -> Result<u64, ClientError> {
        if matches!(self.state, ClientState::Dead | ClientState::Closed) {
            return Err(ClientError::Io(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "LSP client is not running",
            )));
        }
        let id = self.next_id;
        self.next_id += 1;
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.io.send(body.to_string().as_bytes())?;
        self.pending.insert(
            id,
            PendingRequest {
                method: method.to_string(),
                deadline: Instant::now() + self.options.request_timeout,
            },
        );
        Ok(id)
    }

    /// Send a notification (no id, no pending entry).
    pub fn notify(&mut self, method: &str, params: Value) -> Result<(), ClientError> {
        let body = json!({"jsonrpc": "2.0", "method": method, "params": params});
        self.io.send(body.to_string().as_bytes())?;
        Ok(())
    }

    /// Cancel a pending request: drop it and tell the server. A late reply
    /// for this id then finds no pending entry and is ignored as stale.
    pub fn cancel(&mut self, id: u64) -> Result<(), ClientError> {
        self.pending.remove(&id);
        self.notify("$/cancelRequest", json!({"id": id}))
    }

    /// One slice of inbound progress: polls the transport once (up to
    /// `timeout`), dispatches whatever arrived, and expires any pending
    /// request past its deadline.
    pub fn pump(&mut self, timeout: Duration) -> Vec<ClientEvent> {
        let mut events = Vec::new();
        match self.io.poll(timeout) {
            Some(Inbound::Message(body)) => {
                if let Some(event) = self.dispatch(&body) {
                    events.push(event);
                }
            }
            Some(Inbound::Closed) => {
                self.die("server closed stdout".to_string(), &mut events);
            }
            Some(Inbound::Corrupt(error)) => {
                self.die(format!("corrupt frame: {error}"), &mut events);
            }
            None => {}
        }
        self.expire_pending(&mut events);
        events
    }

    /// Run `shutdown` → `exit` → bounded teardown. Always returns; a server
    /// that ignores `exit` is killed by the transport's own deadline.
    pub fn shutdown(&mut self) -> ShutdownReport {
        if matches!(self.state, ClientState::Dead | ClientState::Closed) {
            self.io.shutdown();
            self.state = ClientState::Closed;
            return ShutdownReport {
                shutdown_replied: false,
                exit_sent: false,
            };
        }
        let mut replied = false;
        if let Ok(id) = self.request("shutdown", Value::Null) {
            let started = Instant::now();
            while started.elapsed() < self.options.shutdown_timeout {
                let events = self.pump(Duration::from_millis(20));
                if events
                    .iter()
                    .any(|e| matches!(e, ClientEvent::Response { id: r, .. } if *r == id))
                    || self.state != ClientState::Ready
                {
                    replied = self.state == ClientState::Ready
                        || events
                            .iter()
                            .any(|e| matches!(e, ClientEvent::Response { id: r, .. } if *r == id));
                    break;
                }
            }
        }
        let exit_sent = self.notify("exit", Value::Null).is_ok();
        self.io.shutdown();
        self.state = ClientState::Closed;
        ShutdownReport {
            shutdown_replied: replied,
            exit_sent,
        }
    }

    /// Replace the transport and bump the generation, invalidating every
    /// pending request and every outstanding event from the old generation.
    /// Bounded by `options.restart_budget`.
    pub fn restart_with(&mut self, io: Box<dyn LspIo>) -> Result<u64, ClientError> {
        if self.options.restart_budget == 0 {
            return Err(ClientError::RestartBudgetExhausted);
        }
        self.options.restart_budget -= 1;
        self.io.shutdown();
        self.io = io;
        self.generation += 1;
        self.next_id = 1;
        self.pending.clear();
        self.state = ClientState::Starting;
        Ok(self.generation)
    }

    /// Respawn the same argv/cwd and re-run the handshake.
    pub fn restart(
        &mut self,
        argv: &[String],
        cwd: &Path,
        root_uri: &str,
    ) -> Result<u64, ClientError> {
        let transport = LspTransport::spawn(argv, cwd)?;
        let generation = self.restart_with(Box::new(transport))?;
        self.begin_initialize(root_uri)?;
        Ok(generation)
    }

    fn die(&mut self, reason: String, events: &mut Vec<ClientEvent>) {
        self.pending.clear();
        if self.state != ClientState::Dead {
            self.state = ClientState::Dead;
            events.push(ClientEvent::ServerDied {
                generation: self.generation,
                reason,
            });
        }
    }

    fn expire_pending(&mut self, events: &mut Vec<ClientEvent>) {
        let now = Instant::now();
        let expired: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, p)| now >= p.deadline)
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            if let Some(pending) = self.pending.remove(&id) {
                // Tell the server so it can drop the work; a late reply is
                // then a no-op because the id is gone from `pending`.
                let _ = self.notify("$/cancelRequest", json!({"id": id}));
                events.push(ClientEvent::Expired {
                    id,
                    method: pending.method,
                });
            }
        }
    }

    /// Decode one complete frame body into at most one `ClientEvent`.
    fn dispatch(&mut self, body: &[u8]) -> Option<ClientEvent> {
        let message: Value = match serde_json::from_slice(body) {
            Ok(m) => m,
            Err(e) => {
                self.pending.clear();
                self.state = ClientState::Dead;
                return Some(ClientEvent::ServerDied {
                    generation: self.generation,
                    reason: format!("unparseable message: {e}"),
                });
            }
        };
        let has_method = message.get("method").is_some();
        let id = message.get("id").cloned();

        match (id, has_method) {
            // Server-initiated request. This client supports none — every one
            // gets an explicit MethodNotFound, never a silent workspace edit
            // or command execution.
            (Some(id), true) => {
                let reply = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32601,
                        "message": "client does not support server-initiated requests",
                    },
                });
                let _ = self.io.send(reply.to_string().as_bytes());
                Some(ClientEvent::Notification {
                    generation: self.generation,
                    method: "$/unsupportedServerRequest".to_string(),
                    params: message.to_string(),
                })
            }
            // Response: resolve the pending entry. Unknown ids are stale —
            // the request expired, was cancelled, or belonged to a dead
            // generation — and are dropped.
            (Some(id), false) => {
                let key = id.as_u64()?;
                let _pending = self.pending.remove(&key)?;
                if self.state == ClientState::Starting
                    && _pending.method == "initialize"
                    && message.get("error").is_none()
                {
                    self.finish_initialize(&message);
                    return Some(ClientEvent::Ready {
                        generation: self.generation,
                    });
                }
                let outcome = match (message.get("result"), message.get("error")) {
                    (_, Some(e)) => Err(e.to_string()),
                    (Some(r), None) => Ok(r.to_string()),
                    (None, None) => Ok("null".to_string()),
                };
                Some(ClientEvent::Response {
                    id: key,
                    generation: self.generation,
                    outcome,
                })
            }
            // Notification.
            (None, true) => Some(ClientEvent::Notification {
                generation: self.generation,
                method: message["method"].as_str().unwrap_or("").to_string(),
                params: message
                    .get("params")
                    .map_or_else(|| "null".to_string(), |p| p.to_string()),
            }),
            (None, false) => None,
        }
    }

    fn finish_initialize(&mut self, message: &Value) {
        let offered = message["result"]["capabilities"]["positionEncoding"].as_str();
        self.encoding = PositionEncoding::from_capability(offered);
        // `initialized` is fire-and-forget; a broken pipe here surfaces on the
        // next poll as ServerDied.
        let _ = self.notify("initialized", json!({}));
        self.state = ClientState::Ready;
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if self.state == ClientState::Closed {
            return;
        }
        // Dropping without `shutdown` still performs the bounded teardown —
        // no orphan server can outlive the client handle.
        self.io.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// Deterministic scripted peer: yields queued `Inbound`s and records
    /// every body the client wrote.
    struct ScriptedIo {
        inbound: VecDeque<Inbound>,
        sent: Arc<Mutex<Vec<String>>>,
        shutdowns: Arc<Mutex<u32>>,
    }

    /// (`ScriptedIo`, recorded client sends, recorded shutdown calls).
    type Scripted = (ScriptedIo, Arc<Mutex<Vec<String>>>, Arc<Mutex<u32>>);

    impl ScriptedIo {
        fn new(inbound: Vec<Inbound>) -> Scripted {
            let sent = Arc::new(Mutex::new(Vec::new()));
            let shutdowns = Arc::new(Mutex::new(0u32));
            (
                Self {
                    inbound: inbound.into(),
                    sent: sent.clone(),
                    shutdowns: shutdowns.clone(),
                },
                sent,
                shutdowns,
            )
        }
    }

    impl LspIo for ScriptedIo {
        fn send(&mut self, body: &[u8]) -> io::Result<()> {
            self.sent
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(body).into_owned());
            Ok(())
        }

        fn poll(&mut self, _timeout: Duration) -> Option<Inbound> {
            self.inbound.pop_front()
        }

        fn shutdown(&mut self) {
            *self.shutdowns.lock().unwrap() += 1;
        }
    }

    fn quick_options() -> ClientOptions {
        ClientOptions {
            request_timeout: Duration::from_millis(30),
            shutdown_timeout: Duration::from_millis(50),
            restart_budget: 2,
        }
    }

    fn response(id: u64, result: &str) -> Inbound {
        Inbound::Message(
            format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{result}}}").into_bytes(),
        )
    }

    fn initialize_result() -> Inbound {
        response(
            1,
            "{\"capabilities\":{\"positionEncoding\":\"utf-16\",\"textDocumentSync\":1}}",
        )
    }

    type ReadyClient = (Client, Arc<Mutex<Vec<String>>>, Arc<Mutex<u32>>);

    fn ready_client(inbound: Vec<Inbound>) -> ReadyClient {
        let (io, sent, shutdowns) = ScriptedIo::new(inbound);
        let mut client = Client::over(Box::new(io), quick_options());
        client.begin_initialize("file:///ws").unwrap();
        let events = client.initialize_blocking(Duration::from_secs(1));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClientEvent::Ready { .. })),
            "expected Ready, got {events:?}"
        );
        (client, sent, shutdowns)
    }

    #[test]
    fn client_initialize_negotiates_encoding_and_sends_initialized() {
        let (client, sent, _) = ready_client(vec![initialize_result()]);
        assert_eq!(client.state(), ClientState::Ready);
        assert_eq!(client.encoding(), PositionEncoding::Utf16);
        assert!(sent.lock().unwrap()[0].contains("\"method\":\"initialize\""));
        assert!(sent
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.contains("\"method\":\"initialized\"")));
    }

    #[test]
    fn client_accepts_current_generation_and_version_only() {
        let (mut client, _, _) = ready_client(vec![initialize_result()]);
        let generation = client.generation();
        client.note_document_version("file:///a.rs", 5);
        // Pinned contract from the plan.
        assert!(client.accepts(generation, 5));
        assert!(!client.accepts(generation.wrapping_sub(1), 5));
        assert!(!client.accepts(generation.wrapping_add(1), 5));
        // A response computed against version 4 is stale once 5 is issued.
        assert!(!client.accepts(generation, 4));
        assert!(client.accepts_document("file:///a.rs", generation, 5));
        assert!(!client.accepts_document("file:///a.rs", generation, 4));
        assert!(!client.accepts_document("file:///b.rs", generation, 5));
        // Restart bumps the generation: old-generation results die.
        let (io, _, _) = ScriptedIo::new(vec![]);
        let new_generation = client.restart_with(Box::new(io)).unwrap();
        assert_ne!(new_generation, generation);
        assert!(!client.accepts(generation, 5));
        assert!(client.accepts(new_generation, 5));
    }

    #[test]
    fn client_dispatches_response_and_notification() {
        let (mut client, _, _) = ready_client(vec![
            initialize_result(),
            response(2, "{\"ok\":true}"),
            Inbound::Message(
                br#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"u"}}"#
                    .to_vec(),
            ),
        ]);
        let id = client.request("textDocument/hover", json!({})).unwrap();
        assert_eq!(id, 2, "initialize consumed id 1");
        let events = client.pump(Duration::from_millis(10));
        let events: Vec<ClientEvent> = events
            .into_iter()
            .chain(client.pump(Duration::from_millis(10)))
            .collect();
        assert!(events.iter().any(|e| matches!(
            e,
            ClientEvent::Response { id: 2, generation: 0, outcome: Ok(r) } if r == "{\"ok\":true}"
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            ClientEvent::Notification { method, .. } if method == "textDocument/publishDiagnostics"
        )));
    }

    #[test]
    fn client_replies_method_not_found_to_every_server_request() {
        let (mut client, sent, _) = ready_client(vec![
            initialize_result(),
            Inbound::Message(
                br#"{"jsonrpc":"2.0","id":9001,"method":"workspace/applyEdit","params":{"edit":{}}}"#
                    .to_vec(),
            ),
        ]);
        let events = client.pump(Duration::from_millis(10));
        assert!(events.iter().any(|e| matches!(
            e,
            ClientEvent::Notification { method, .. } if method == "$/unsupportedServerRequest"
        )));
        let reply = sent.lock().unwrap().last().unwrap().clone();
        let reply: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["id"], 9001);
        assert_eq!(reply["error"]["code"], -32601);
        assert!(reply.get("method").is_none(), "a reply is never a request");
    }

    #[test]
    fn client_unknown_response_id_is_dropped_as_stale() {
        let (mut client, _, _) =
            ready_client(vec![initialize_result(), response(4242, "\"never asked\"")]);
        let events = client.pump(Duration::from_millis(10));
        assert!(
            events.is_empty(),
            "responses for never-pending ids must be ignored: {events:?}"
        );
    }

    #[test]
    fn client_expires_pending_and_cancels_server_side() {
        let (mut client, sent, _) = ready_client(vec![initialize_result()]);
        let id = client.request("textDocument/hover", json!({})).unwrap();
        std::thread::sleep(Duration::from_millis(60));
        let events = client.pump(Duration::from_millis(10));
        assert!(events.iter().any(|e| matches!(
            e,
            ClientEvent::Expired { id: i, method } if *i == id && method == "textDocument/hover"
        )));
        assert!(sent
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.contains("$/cancelRequest") && s.contains("\"id\":2")));
        // The late reply finds no pending entry — dropped, not delivered.
        let (io2, _, _) = ScriptedIo::new(vec![response(id, "\"late\"")]);
        client.io = Box::new(io2);
        assert!(client.pump(Duration::from_millis(10)).is_empty());
    }

    #[test]
    fn client_close_and_corrupt_mark_server_dead() {
        let (mut client, _, _) = ready_client(vec![initialize_result(), Inbound::Closed]);
        let events = client.pump(Duration::from_millis(10));
        assert!(events.iter().any(|e| matches!(
            e,
            ClientEvent::ServerDied { reason, .. } if reason.contains("closed")
        )));
        assert_eq!(client.state(), ClientState::Dead);
        assert!(
            client.request("x/y", json!({})).is_err(),
            "dead client refuses new work"
        );

        let (io, _, _) = ScriptedIo::new(vec![Inbound::Message(b"not json".to_vec())]);
        let mut client = Client::over(Box::new(io), quick_options());
        let events = client.pump(Duration::from_millis(10));
        assert!(events.iter().any(|e| matches!(
            e,
            ClientEvent::ServerDied { reason, .. } if reason.contains("unparseable")
        )));
    }

    #[test]
    fn client_restart_budget_limits_crash_loops() {
        let (mut client, _, _) = ready_client(vec![initialize_result()]);
        for expected in [1u64, 2] {
            let (io, _, _) = ScriptedIo::new(vec![]);
            let generation = client.restart_with(Box::new(io)).unwrap();
            assert_eq!(generation, expected);
        }
        let (io, _, _) = ScriptedIo::new(vec![]);
        assert!(matches!(
            client.restart_with(Box::new(io)),
            Err(ClientError::RestartBudgetExhausted)
        ));
    }

    #[test]
    fn client_shutdown_sends_shutdown_then_exit() {
        let (mut client, sent, shutdowns) = ready_client(vec![
            initialize_result(),
            response(2, "null"), // shutdown reply
        ]);
        let report = client.shutdown();
        assert!(report.shutdown_replied);
        assert!(report.exit_sent);
        assert_eq!(client.state(), ClientState::Closed);
        assert_eq!(*shutdowns.lock().unwrap(), 1);
        let all = sent.lock().unwrap();
        let shutdown_pos = all
            .iter()
            .position(|s| s.contains("\"method\":\"shutdown\""))
            .unwrap();
        let exit_pos = all
            .iter()
            .position(|s| s.contains("\"method\":\"exit\""))
            .unwrap();
        assert!(shutdown_pos < exit_pos, "exit follows shutdown reply");
    }

    #[test]
    fn client_shutdown_bounded_when_server_never_replies() {
        let (mut client, sent, _) = ready_client(vec![initialize_result()]);
        let started = Instant::now();
        let report = client.shutdown();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "shutdown must not wait on a silent server"
        );
        assert!(!report.shutdown_replied);
        assert!(report.exit_sent);
        assert!(sent
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.contains("\"method\":\"exit\"")));
    }

    // -- Real-child tests: the Python fake server over actual pipes. --

    #[cfg(unix)]
    fn fake_server(mode: &str, extra: &str) -> Vec<String> {
        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join("fake-lsp-server.py");
        let mut argv = vec![script.to_string_lossy().into_owned(), mode.to_string()];
        if !extra.is_empty() {
            argv.push(extra.to_string());
        }
        argv
    }

    #[cfg(unix)]
    fn spawned_fake(mode: &str, extra: &str) -> Client {
        let dir = tempfile::tempdir().unwrap();
        Client::spawn(
            &fake_server(mode, extra),
            dir.path(),
            "file:///ws",
            ClientOptions {
                request_timeout: Duration::from_secs(5),
                shutdown_timeout: Duration::from_secs(2),
                restart_budget: 1,
            },
        )
        .unwrap()
    }

    #[cfg(unix)]
    fn wait_for(
        client: &mut Client,
        deadline: Duration,
        want: impl Fn(&ClientEvent) -> bool,
    ) -> Vec<ClientEvent> {
        let started = Instant::now();
        let mut all = Vec::new();
        while started.elapsed() < deadline {
            let events = client.pump(Duration::from_millis(20));
            let done = events.iter().any(&want);
            all.extend(events);
            if done {
                break;
            }
        }
        all
    }

    #[cfg(unix)]
    #[test]
    fn fake_success_full_handshake_and_shutdown() {
        let mut client = spawned_fake("success", "");
        let events = client.initialize_blocking(Duration::from_secs(5));
        assert!(events
            .iter()
            .any(|e| matches!(e, ClientEvent::Ready { .. })));
        assert_eq!(client.encoding(), PositionEncoding::Utf16);
        let report = client.shutdown();
        assert!(report.shutdown_replied);
        assert!(report.exit_sent);
    }

    #[cfg(unix)]
    #[test]
    fn fake_unsupported_method_returns_error_result() {
        let mut client = spawned_fake("unsupported", "");
        client.initialize_blocking(Duration::from_secs(5));
        // Even unsupported-mode servers answer our `initialize`? No — this
        // mode errors every request, so the client never became Ready.
        assert_eq!(client.state(), ClientState::Starting);
        let id = client.request("textDocument/hover", json!({})).unwrap();
        let events = wait_for(
            &mut client,
            Duration::from_secs(5),
            |e| matches!(e, ClientEvent::Response { id: r, .. } if *r == id),
        );
        assert!(events.iter().any(|e| matches!(
            e,
            ClientEvent::Response { outcome: Err(e), .. } if e.contains("-32601")
        )));
        client.shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn fake_delayed_response_still_resolves() {
        let mut client = spawned_fake("delayed", "150");
        let events = client.initialize_blocking(Duration::from_secs(5));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ClientEvent::Ready { .. })),
            "a 150ms delay must not trip the 5s handshake deadline"
        );
        client.shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn fake_crash_marks_dead_and_cleanup_is_bounded() {
        let mut client = spawned_fake("crash", "1");
        let events = wait_for(&mut client, Duration::from_secs(5), |e| {
            matches!(e, ClientEvent::ServerDied { .. })
        });
        assert!(events
            .iter()
            .any(|e| matches!(e, ClientEvent::ServerDied { .. })));
        assert_eq!(client.state(), ClientState::Dead);
        let started = Instant::now();
        client.shutdown();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "cleanup after a crash must be bounded"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fake_malformed_output_kills_the_client() {
        let mut client = spawned_fake("malformed", "");
        let events = wait_for(&mut client, Duration::from_secs(5), |e| {
            matches!(e, ClientEvent::ServerDied { .. })
        });
        assert!(events.iter().any(|e| matches!(
            e,
            ClientEvent::ServerDied { reason, .. } if reason.contains("corrupt")
        )));
        client.shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn fake_oversized_output_rejected_by_frame_cap() {
        let mut client = spawned_fake("oversized", "");
        let events = wait_for(&mut client, Duration::from_secs(5), |e| {
            matches!(e, ClientEvent::ServerDied { .. })
        });
        assert!(events.iter().any(|e| matches!(
            e,
            ClientEvent::ServerDied { reason, .. } if reason.contains("corrupt")
        )));
        client.shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn fake_ignored_exit_still_terminates_bounded() {
        let mut client = spawned_fake("noexit", "");
        client.initialize_blocking(Duration::from_secs(5));
        let started = Instant::now();
        let report = client.shutdown();
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "a server ignoring `exit` is still torn down"
        );
        assert!(report.exit_sent);
    }

    #[cfg(unix)]
    #[test]
    fn fake_apply_edit_gets_explicit_unsupported_reply() {
        let mut client = spawned_fake("apply-edit", "");
        client.initialize_blocking(Duration::from_secs(5));
        // The server sends workspace/applyEdit, waits for our reply, then
        // reports it back as custom/serverSawReply with the error code.
        let events = wait_for(
            &mut client,
            Duration::from_secs(5),
            |e| matches!(e, ClientEvent::Notification { method, .. } if method == "custom/serverSawReply"),
        );
        let notification = events.iter().find_map(|e| match e {
            ClientEvent::Notification { method, params, .. }
                if method == "custom/serverSawReply" =>
            {
                Some(params.clone())
            }
            _ => None,
        });
        let params: Value =
            serde_json::from_str(&notification.expect("server must see our reply")).unwrap();
        assert_eq!(params["replied"], true);
        assert_eq!(params["code"], -32601, "explicitly refused, never applied");
        client.shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn fake_restart_respawns_and_bumps_generation() {
        let mut client = spawned_fake("success", "");
        client.initialize_blocking(Duration::from_secs(5));
        let first = client.generation();
        let dir = tempfile::tempdir().unwrap();
        let second = client
            .restart(&fake_server("success", ""), dir.path(), "file:///ws")
            .unwrap();
        assert_eq!(second, first + 1);
        let events = client.initialize_blocking(Duration::from_secs(5));
        assert!(events.iter().any(|e| matches!(
            e,
            ClientEvent::Ready { generation } if *generation == second
        )));
        // Results stamped with the old generation stay rejected.
        assert!(!client.accepts(first, 0));
        client.shutdown();
    }
}
