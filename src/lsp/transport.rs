//! Bounded stdio JSON-RPC framing for LSP servers (FR-10).
//!
//! LSP frames are `Content-Length: <bytes>\r\n\r\n` headers followed by the
//! raw body; Content-Length counts BYTES, not characters. This module owns
//! the byte-level contract — encoder, streaming decoder, message-size caps —
//! plus a spawned-child transport whose stderr is drained separately from
//! the protocol stream and whose queues are bounded so a stalled or hostile
//! server cannot grow memory or block the UI thread. It is independent of
//! `App`: the only inputs are argv, bytes, and channel senders.

// Staged surface: the Task 3 client lifecycle consumes this module; only
// unit tests touch it until then. Drop this allow when the client lands.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;

/// Cap on a single inbound frame's declared Content-Length (16 MiB covers
/// legitimately large workspace/symbol payloads while bounding hostile ones).
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
/// Cap on buffered partial-frame bytes while a header is incomplete.
pub const MAX_HEADER_BYTES: usize = 8 * 1024;
/// Bounded channel capacities — a flooded peer stalls at the bound.
pub const OUTBOUND_SLOTS: usize = 64;
pub const INBOUND_SLOTS: usize = 64;
/// Stderr line ring-buffer bound: diagnostics, not unbounded capture.
pub const STDERR_LINES: usize = 200;

/// Errors from the byte-level frame decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// Header block larger than [`MAX_HEADER_BYTES`] with no terminator.
    OversizedHeader,
    /// Declared Content-Length exceeds [`MAX_MESSAGE_BYTES`].
    OversizedMessage(usize),
    /// Missing/duplicate/unparseable Content-Length header.
    MalformedHeader(String),
    /// Input ended while a frame was incomplete.
    EofMidFrame { needed: usize, buffered: usize },
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OversizedHeader => write!(f, "LSP header exceeds {MAX_HEADER_BYTES} bytes"),
            Self::OversizedMessage(n) => write!(
                f,
                "LSP message declares {n} bytes (max {MAX_MESSAGE_BYTES})"
            ),
            Self::MalformedHeader(why) => write!(f, "malformed LSP header: {why}"),
            Self::EofMidFrame { needed, buffered } => {
                write!(
                    f,
                    "LSP stream ended mid-frame ({needed} bytes wanted, {buffered} held)"
                )
            }
        }
    }
}

impl std::error::Error for FrameError {}

/// Encode one JSON-RPC body into a wire frame. `Content-Length` is the byte
/// length of `body` — multi-byte UTF-8 content counts its bytes.
pub fn encode_frame(body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(body.len() + 32);
    frame.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
    frame.extend_from_slice(body);
    frame
}

/// Incremental decoder for the stdio frame format. Feed arbitrary byte
/// slices; complete message bodies come back in order. The buffered prefix
/// is bounded — an endless header or a huge Content-Length is an error, not
/// unbounded growth.
pub struct FrameDecoder {
    buf: Vec<u8>,
    max_message: usize,
    max_header: usize,
    /// Length of an already-announced body once its header was consumed —
    /// keeps oversize rejection intact even when the header and body arrive
    /// in separate `feed` calls.
    pending_body: Option<usize>,
    header_end: Option<usize>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::with_limits(MAX_MESSAGE_BYTES, MAX_HEADER_BYTES)
    }

    pub fn with_limits(max_message: usize, max_header: usize) -> Self {
        Self {
            buf: Vec::new(),
            max_message,
            max_header,
            pending_body: None,
            header_end: None,
        }
    }

    /// Feed bytes; return every complete frame body now available.
    pub fn feed(&mut self, data: &[u8]) -> Result<Vec<Vec<u8>>, FrameError> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        loop {
            if self.pending_body.is_none() {
                let header_end = match self.header_end {
                    Some(end) => end,
                    None => match find_subslice(&self.buf, b"\r\n\r\n") {
                        Some(end) => {
                            self.header_end = Some(end);
                            end
                        }
                        None => {
                            if self.buf.len() > self.max_header {
                                return Err(FrameError::OversizedHeader);
                            }
                            return Ok(out);
                        }
                    },
                };
                let length = parse_content_length(&self.buf[..header_end])?;
                if length > self.max_message {
                    return Err(FrameError::OversizedMessage(length));
                }
                self.buf.drain(..header_end + 4);
                self.header_end = None;
                self.pending_body = Some(length);
            }
            let want = self.pending_body.unwrap();
            if self.buf.len() < want {
                return Ok(out);
            }
            out.push(self.buf.drain(..want).collect());
            self.pending_body = None;
        }
    }

    /// Signal end-of-input. Clean only when the buffer ends exactly on a
    /// frame boundary; a partial tail is an error.
    pub fn finish(&mut self) -> Result<(), FrameError> {
        if self.buf.is_empty() && self.pending_body.is_none() {
            Ok(())
        } else {
            Err(FrameError::EofMidFrame {
                needed: self.pending_body.unwrap_or(0),
                buffered: self.buf.len(),
            })
        }
    }
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Parse `Content-Length` from a header block (lines ending `\r\n`,
/// terminated by the caller-found `\r\n\r\n`). Field name matching is
/// case-insensitive per the base protocol; other fields are ignored.
fn parse_content_length(header: &[u8]) -> Result<usize, FrameError> {
    let text = std::str::from_utf8(header)
        .map_err(|_| FrameError::MalformedHeader("header is not UTF-8".into()))?;
    let mut length = None;
    for line in text.split("\r\n") {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                if length.is_some() {
                    return Err(FrameError::MalformedHeader(
                        "duplicate Content-Length".into(),
                    ));
                }
                let parsed: usize = value.trim().parse().map_err(|_| {
                    FrameError::MalformedHeader(format!("bad Content-Length {value:?}"))
                })?;
                length = Some(parsed);
            }
        } else {
            return Err(FrameError::MalformedHeader(format!(
                "bad header line {line:?}"
            )));
        }
    }
    length.ok_or_else(|| FrameError::MalformedHeader("no Content-Length".into()))
}

/// One inbound transport observation for the client state machine.
#[derive(Debug)]
pub enum Inbound {
    /// A complete JSON-RPC body.
    Message(Vec<u8>),
    /// The child closed its stdout.
    Closed,
    /// Framing failed; the stream is unrecoverable.
    Corrupt(FrameError),
}

/// A spawned LSP server process with bounded inbound/outbound plumbing.
///
/// Spawned with argv directly through `std::process::Command` — never a
/// shell — so configured executable+arguments cannot interpolate. Stderr is
/// drained on its own thread into a bounded ring so a chatty server can
/// neither deadlock on a full pipe nor grow memory.
pub struct LspTransport {
    child: Child,
    /// Bounded outbound queue; `send` never blocks the caller. Dropped at
    /// shutdown so the writer's blocking `recv` ends and it can join.
    out_tx: Option<SyncSender<Vec<u8>>>,
    /// Bounded inbound frame stream read on the client side.
    inbound: Receiver<Inbound>,
    /// Drained stderr lines (bounded ring, newest kept).
    stderr_lines: std::sync::Arc<std::sync::Mutex<VecDeque<String>>>,
    writer: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    stderr: Option<JoinHandle<()>>,
}

impl LspTransport {
    /// Spawn `argv[0]` with `argv[1..]` as arguments — direct exec, no shell.
    /// Fails cleanly (`Err`) when the executable is missing so callers can
    /// keep editing without a server.
    pub fn spawn(argv: &[String], cwd: &std::path::Path) -> std::io::Result<Self> {
        let (program, args) = argv.split_first().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty LSP argv")
        })?;
        let mut child = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        // The writer thread owns stdin: the channel closing (all senders
        // dropped) ends the loop and drops the pipe → child sees clean EOF.
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("LSP child has no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("LSP child has no stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| std::io::Error::other("LSP child has no stderr"))?;

        let (out_tx, out_rx) = mpsc::sync_channel::<Vec<u8>>(OUTBOUND_SLOTS);
        let (in_tx, inbound) = mpsc::sync_channel::<Inbound>(INBOUND_SLOTS);
        let stderr_lines = std::sync::Arc::new(std::sync::Mutex::new(VecDeque::new()));

        let writer = std::thread::Builder::new()
            .name("lsp-stdin".into())
            .spawn(move || {
                while let Ok(body) = out_rx.recv() {
                    let frame = encode_frame(&body);
                    if stdin.write_all(&frame).is_err() {
                        return;
                    }
                }
            })
            .map_err(|e| std::io::Error::other(format!("writer spawn: {e}")))?;

        let reader_in = in_tx.clone();
        let reader = std::thread::Builder::new()
            .name("lsp-stdout".into())
            .spawn(move || {
                let mut reader = stdout;
                let mut decoder = FrameDecoder::new();
                let mut chunk = [0u8; 8192];
                loop {
                    match reader.read(&mut chunk) {
                        Ok(0) => {
                            let terminal = match decoder.finish() {
                                Ok(()) => Inbound::Closed,
                                Err(e) => Inbound::Corrupt(e),
                            };
                            let _ = reader_in.send(terminal);
                            return;
                        }
                        Ok(n) => match decoder.feed(&chunk[..n]) {
                            Ok(frames) => {
                                let mut ok = true;
                                for frame in frames {
                                    ok &= reader_in.send(Inbound::Message(frame)).is_ok();
                                }
                                if !ok {
                                    return;
                                }
                            }
                            Err(e) => {
                                let _ = reader_in.send(Inbound::Corrupt(e));
                                return;
                            }
                        },
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => {
                            let _ = reader_in.send(Inbound::Closed);
                            return;
                        }
                    }
                }
            })
            .map_err(|e| std::io::Error::other(format!("reader spawn: {e}")))?;

        let ring = stderr_lines.clone();
        let stderr = std::thread::Builder::new()
            .name("lsp-stderr".into())
            .spawn(move || {
                let mut lines = BufReader::new(stderr).lines();
                while let Some(Ok(line)) = lines.next() {
                    let mut ring = ring.lock().unwrap();
                    if ring.len() >= STDERR_LINES {
                        ring.pop_front();
                    }
                    ring.push_back(line);
                }
            })
            .map_err(|e| std::io::Error::other(format!("stderr spawn: {e}")))?;

        Ok(Self {
            child,
            out_tx: Some(out_tx),
            inbound,
            stderr_lines,
            writer: Some(writer),
            reader: Some(reader),
            stderr: Some(stderr),
        })
    }

    /// Queue one JSON-RPC body to the child. Never blocks: a full queue is a
    /// `WouldBlock` error, not a stall of the caller; a shut-down transport
    /// reports `BrokenPipe`.
    pub fn send(&self, body: &[u8]) -> std::io::Result<()> {
        let Some(tx) = &self.out_tx else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "LSP transport closed",
            ));
        };
        tx.try_send(body.to_vec())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::WouldBlock, e.to_string()))
    }

    /// Receive the next inbound observation (blocks until one arrives or
    /// every producer is gone).
    pub fn recv(&self) -> Option<Inbound> {
        self.inbound.recv().ok()
    }

    /// Receive the next inbound observation within `timeout`.
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Option<Inbound> {
        self.inbound.recv_timeout(timeout).ok()
    }

    /// Snapshot the drained stderr lines (bounded ring, oldest dropped).
    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr_lines.lock().unwrap().iter().cloned().collect()
    }

    /// Terminate the child (kill + reap) and join the transport threads.
    /// Bounded: `kill` is SIGKILL-equivalent, so `wait` cannot hang; dropping
    /// the queue sender ends the writer's `recv` so its join returns.
    pub fn shutdown(&mut self) {
        drop(self.out_tx.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
        for handle in [self.writer.take(), self.reader.take(), self.stderr.take()]
            .into_iter()
            .flatten()
        {
            let _ = handle.join();
        }
    }
}

impl Drop for LspTransport {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_matrix_frame_round_trip() {
        let body = br#"{"jsonrpc":"2.0","id":1,"result":null}"#;
        let framed = encode_frame(body);
        assert_eq!(decode_one(&framed).unwrap(), body);
    }

    fn decode_one(framed: &[u8]) -> Result<Vec<u8>, FrameError> {
        let mut dec = FrameDecoder::new();
        let mut out = dec.feed(framed)?;
        assert_eq!(out.len(), 1, "expected exactly one frame");
        dec.finish()?;
        Ok(out.remove(0))
    }

    #[test]
    fn header_and_body_survive_byte_at_a_time_and_coalesced_feeds() {
        let a = encode_frame(b"first");
        let b = encode_frame(br#"{"jsonrpc":"2.0","method":"x"}"#);
        let mut wire = a.clone();
        wire.extend_from_slice(&b);
        // Coalesced: both frames in one feed.
        let mut dec = FrameDecoder::new();
        let out = dec.feed(&wire).unwrap();
        assert_eq!(
            out,
            vec![
                b"first".to_vec(),
                br#"{"jsonrpc":"2.0","method":"x"}"#.to_vec()
            ]
        );
        assert!(dec.finish().is_ok());
        // Fragmented: every split point must decode identically.
        for split in 1..wire.len() {
            let mut dec = FrameDecoder::new();
            let mut out = dec.feed(&wire[..split]).unwrap();
            out.extend(dec.feed(&wire[split..]).unwrap());
            assert_eq!(out.len(), 2, "split {split} lost a frame");
        }
        // Byte-at-a-time extremes.
        let mut dec = FrameDecoder::new();
        let mut out = vec![];
        for byte in &wire {
            out.extend(dec.feed(&[*byte]).unwrap());
        }
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn content_length_counts_bytes_for_unicode_bodies() {
        let body = "{\"label\":\"中文字符🦀\"}".as_bytes();
        let framed = encode_frame(body);
        let header = std::str::from_utf8(&framed[..find_subslice(&framed, b"\r\n\r\n").unwrap()])
            .unwrap()
            .to_string();
        assert!(header.contains(&format!("Content-Length: {}", body.len())));
        assert_eq!(decode_one(&framed).unwrap(), body);
    }

    #[test]
    fn malformed_headers_are_rejected() {
        for bad in [
            b"Content-Length: nope\r\n\r\n{}" as &[u8],
            b"Content-Type: x\r\n\r\n{}" as &[u8],
            b"Content-Length: 1\r\nContent-Length: 2\r\n\r\n{}" as &[u8],
            b"garbage-line\r\n\r\n{}" as &[u8],
        ] {
            let mut dec = FrameDecoder::new();
            assert!(
                matches!(dec.feed(bad), Err(FrameError::MalformedHeader(_))),
                "{bad:?} must be malformed"
            );
        }
    }

    #[test]
    fn oversized_message_and_header_are_bounded() {
        // Declared length beyond the cap is rejected before any body bytes.
        let mut dec = FrameDecoder::with_limits(8, MAX_HEADER_BYTES);
        let err = dec
            .feed(b"Content-Length: 16\r\n\r\n0123456789012345")
            .unwrap_err();
        assert_eq!(err, FrameError::OversizedMessage(16));
        // Header that never terminates grows the buffer only to the bound.
        let mut dec = FrameDecoder::with_limits(MAX_MESSAGE_BYTES, 16);
        let mut input = vec![b'X'; 17];
        assert_eq!(dec.feed(&input).unwrap_err(), FrameError::OversizedHeader);
        // A header split across feeds still hits the bound.
        let mut dec = FrameDecoder::with_limits(MAX_MESSAGE_BYTES, 16);
        dec.feed(&[b'X'; 10]).unwrap();
        input.extend_from_slice(&[b'Y'; 7]);
        assert_eq!(
            dec.feed(&input[10..]).unwrap_err(),
            FrameError::OversizedHeader
        );
    }

    #[test]
    fn eof_is_clean_at_boundaries_and_corrupt_mid_frame() {
        let mut dec = FrameDecoder::new();
        dec.feed(&encode_frame(b"done")).unwrap();
        assert!(dec.finish().is_ok());

        let mut dec = FrameDecoder::new();
        dec.feed(b"Content-Length: 5\r\n\r\nab").unwrap();
        assert_eq!(
            dec.finish().unwrap_err(),
            FrameError::EofMidFrame {
                needed: 5,
                buffered: 2
            }
        );
    }

    /// Spawn the argv directly — the test binary runs `sh -c` style runners
    /// without any shell interpolation by pointing at the script file.
    #[cfg(unix)]
    fn script_runner(dir: &tempfile::TempDir, name: &str, body: &str) -> Vec<String> {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.path().join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        vec![path.to_str().unwrap().to_string()]
    }

    #[cfg(unix)]
    fn collect_until(
        transport: &LspTransport,
        deadline: std::time::Duration,
        done: impl Fn(&Vec<u8>) -> bool,
    ) -> Vec<u8> {
        let start = std::time::Instant::now();
        let mut bodies = Vec::new();
        while start.elapsed() < deadline {
            match transport.recv_timeout(std::time::Duration::from_millis(50)) {
                Some(Inbound::Message(body)) => {
                    if done(&body) {
                        return body;
                    }
                    bodies.extend(body);
                }
                Some(_) => return bodies,
                None => {}
            }
        }
        bodies
    }

    #[cfg(unix)]
    #[test]
    fn transport_echo_roundtrip_through_real_child() {
        let dir = tempfile::tempdir().unwrap();
        // `cat` echoes the entire frame back; the reader's decoder then hands
        // the body to us — a full encode→pipe→decode loop through a real
        // child spawned by argv alone (no shell interpolation).
        let argv = script_runner(&dir, "echo", "#!/bin/sh\nexec cat\n");
        let mut transport = LspTransport::spawn(&argv, dir.path()).unwrap();
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        transport.send(body).unwrap();
        let got = collect_until(&transport, std::time::Duration::from_secs(5), |b| b == body);
        assert_eq!(
            got,
            body.to_vec(),
            "child must echo the framed body verbatim"
        );
        transport.shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn transport_stderr_is_drained_separately_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let argv = script_runner(
            &dir,
            "noisy-stderr",
            "#!/bin/sh\ni=0\nwhile [ $i -lt 400 ]; do echo \"stderr-line-$i\" >&2; i=$((i+1)); done\nsleep 0.2\nexit 0\n",
        );
        let mut transport = LspTransport::spawn(&argv, dir.path()).unwrap();
        // Wait for the child to exit and stderr to drain.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while transport.stderr_tail().len() < STDERR_LINES && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let tail = transport.stderr_tail();
        assert_eq!(tail.len(), STDERR_LINES, "stderr ring keeps the bound");
        assert_eq!(tail.last().unwrap(), "stderr-line-399");
        assert_eq!(tail.first().unwrap(), "stderr-line-200");
        // Closing the protocol side surfaces as Closed, not a hang.
        let closed = matches!(
            transport.recv_timeout(std::time::Duration::from_secs(5)),
            Some(Inbound::Closed)
        );
        assert!(closed, "child exit must surface as Inbound::Closed");
        transport.shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn transport_send_is_bounded_and_never_blocks() {
        let dir = tempfile::tempdir().unwrap();
        // Child that never reads stdin → the pipe fills → our queue hits its
        // bound and reports WouldBlock instead of blocking the caller.
        let argv = script_runner(&dir, "stall", "#!/bin/sh\nexec sleep 60\n");
        let mut transport = LspTransport::spawn(&argv, dir.path()).unwrap();
        let mut admitted = 0usize;
        let mut rejected = None;
        // 4 KiB bodies: the OS pipe holds ~64 KiB, so a never-reading child
        // fills it in ~16 frames and our bounded queue is what actually caps
        // in-flight packets — rejection is deterministic, not timing luck.
        let body = vec![b'x'; 4096];
        for _ in 0..(OUTBOUND_SLOTS * 4) {
            match transport.send(&body) {
                Ok(()) => admitted += 1,
                Err(e) => {
                    rejected = Some(e);
                    break;
                }
            }
        }
        // The bound that matters: `send` eventually reports WouldBlock rather
        // than blocking forever. How many it admitted first depends on how
        // fast the writer drains into the OS pipe — the queue itself stays
        // capped at OUTBOUND_SLOTS in-flight packets.
        let error = rejected.expect("a never-reading child must bound the send queue");
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert!(
            admitted >= OUTBOUND_SLOTS,
            "admitted {admitted} under the queue bound"
        );
        transport.shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn transport_spawn_failure_is_a_clean_error() {
        let dir = tempfile::tempdir().unwrap();
        let argv = vec!["/nonexistent/definitely-not-a-server".to_string()];
        assert!(LspTransport::spawn(&argv, dir.path()).is_err());
        let empty: Vec<String> = vec![];
        assert!(LspTransport::spawn(&empty, dir.path()).is_err());
    }
}
