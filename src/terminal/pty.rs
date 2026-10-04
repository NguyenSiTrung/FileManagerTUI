//! PTY process management: spawning, bounded stdin, output and owned lifecycle.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::event::{Event, EventSender};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputOutcome {
    Written,
    Failed {
        written: usize,
        reason: &'static str,
    },
}

/// Includes the active packet until its completion has been delivered. Admission
/// is atomic/nonwaiting; Full never accepts a prefix or allocates a pending task.
#[derive(Clone, Copy)]
struct InputLimits {
    packets: usize,
    bytes: usize,
    packet_bytes: usize,
}
impl Default for InputLimits {
    fn default() -> Self {
        Self {
            packets: 64,
            bytes: 2 * 1024 * 1024,
            packet_bytes: 1024 * 1024,
        }
    }
}
struct InputPacket {
    sequence: u64,
    bytes: Box<[u8]>,
}
struct InputState {
    queue: VecDeque<InputPacket>,
    packets: usize,
    bytes: usize,
    next: u64,
    closed: bool,
}
struct InputQueue {
    state: Mutex<InputState>,
    ready: Condvar,
    limits: InputLimits,
}
impl InputQueue {
    fn new(limits: InputLimits) -> Arc<Self> {
        assert!(
            limits.packets > 0 && limits.packet_bytes > 0 && limits.packet_bytes <= limits.bytes
        );
        Arc::new(Self {
            state: Mutex::new(InputState {
                queue: VecDeque::new(),
                packets: 0,
                bytes: 0,
                next: 1,
                closed: false,
            }),
            ready: Condvar::new(),
            limits,
        })
    }
    fn admit(&self, data: &[u8]) -> std::io::Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Terminal input closed: no bytes accepted",
            ));
        }
        if data.len() > self.limits.packet_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Terminal input exceeds whole-packet limit: no bytes accepted",
            ));
        }
        if state.packets == self.limits.packets || data.len() > self.limits.bytes - state.bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "Terminal input queue full: no bytes accepted; retry after pending input",
            ));
        }
        let next = state.next.checked_add(1).ok_or_else(|| {
            std::io::Error::other("Terminal input sequence exhausted: no bytes accepted")
        })?;
        let packet = InputPacket {
            sequence: state.next,
            bytes: data.into(),
        };
        state.next = next;
        state.packets += 1;
        state.bytes += packet.bytes.len();
        state.queue.push_back(packet);
        self.ready.notify_one();
        Ok(())
    }
    fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.ready.notify_all();
    }
    fn closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }
    fn take(&self, output: &EventSender) -> Option<InputPacket> {
        let mut state = self.state.lock().unwrap();
        loop {
            if let Some(packet) = state.queue.pop_front() {
                return Some(packet);
            }
            if state.closed || output.is_closed() {
                return None;
            }
            // Consumer close is a different Condvar; finite polling owns idle
            // cancellation without an unbounded relay or detached close task.
            state = self
                .ready
                .wait_timeout(state, Duration::from_millis(50))
                .unwrap()
                .0;
        }
    }
    fn retire(&self, bytes: usize) {
        let mut state = self.state.lock().unwrap();
        state.packets -= 1;
        state.bytes -= bytes;
    }
}

struct InputWriter {
    queue: Arc<InputQueue>,
    output: EventSender,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl InputWriter {
    fn spawn(
        mut writer: Box<dyn Write + Send>,
        session: u64,
        output: EventSender,
        limits: InputLimits,
        mut ready: impl FnMut() -> std::io::Result<()> + Send + 'static,
    ) -> std::io::Result<Self> {
        let queue = InputQueue::new(limits);
        let worker_queue = queue.clone();
        let worker_output = output.clone();
        let handle = std::thread::Builder::new()
            .name("pty-stdin".into())
            .spawn(move || {
                while let Some(packet) = worker_queue.take(&worker_output) {
                    let mut written = 0;
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                        || -> std::io::Result<()> {
                            if worker_queue.closed() || worker_output.is_closed() {
                                return Err(std::io::ErrorKind::Interrupted.into());
                            }
                            while written < packet.bytes.len() {
                                if worker_queue.closed() || worker_output.is_closed() {
                                    return Err(std::io::ErrorKind::Interrupted.into());
                                }
                                ready()?;
                                if worker_queue.closed() || worker_output.is_closed() {
                                    return Err(std::io::ErrorKind::Interrupted.into());
                                }
                                match writer.write(
                                    &packet.bytes
                                        [written..(written + 4096).min(packet.bytes.len())],
                                ) {
                                    Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
                                    Ok(n) => written += n,
                                    Err(e)
                                        if matches!(
                                            e.kind(),
                                            std::io::ErrorKind::Interrupted
                                                | std::io::ErrorKind::WouldBlock
                                        ) =>
                                    {
                                        continue
                                    }
                                    Err(e) => return Err(e),
                                }
                            }
                            writer.flush()
                        },
                    ));
                    let outcome = match result {
                        Ok(Ok(())) => InputOutcome::Written,
                        other => {
                            let reason = if worker_queue.closed() || worker_output.is_closed() {
                                "session/transport closed; remainder cancelled"
                            } else if other.is_err() {
                                "PTY writer panicked; remainder cancelled"
                            } else {
                                "PTY I/O failed; remainder cancelled"
                            };
                            // Never replay a partly written packet or allow later
                            // packets to overtake it after a fatal I/O failure.
                            worker_queue.close();
                            InputOutcome::Failed { written, reason }
                        }
                    };
                    let _ = worker_output.blocking_send(Event::TerminalInputComplete {
                        session,
                        sequence: packet.sequence,
                        outcome,
                    });
                    let bytes = packet.bytes.len();
                    drop(packet); // Physical retention ends BEFORE budget release.
                    worker_queue.retire(bytes);
                }
                worker_queue.close();
            })?;
        Ok(Self {
            queue,
            output,
            handle: Mutex::new(Some(handle)),
        })
    }
    fn stop(&self) {
        self.queue.close();
        self.output.stop();
    }
    fn join(&self) {
        if let Some(handle) = self.handle.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
}

/// Exactly one owned input worker and one owned output reader per PTY session.
pub struct PtyProcess {
    writer: InputWriter,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    reader_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    output: EventSender,
    session: u64,
}

impl PtyProcess {
    pub fn spawn(
        shell: &str,
        cwd: &Path,
        rows: u16,
        cols: u16,
        output_tx: EventSender,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        if output_tx.max_event_bytes() < std::mem::size_of::<Event>() + 4096 {
            return Err(
                std::io::Error::other("PTY transport must admit a whole 4096-byte chunk").into(),
            );
        }
        let pair = native_pty_system().openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        // Do all fallible descriptor setup before starting our child.
        let writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let fd = native_nonblocking(pair.master.as_raw_fd())?;
        let mut cmd = CommandBuilder::new(shell);
        cmd.cwd(cwd);
        let child = Arc::new(Mutex::new(pair.slave.spawn_command(cmd)?));
        drop(pair.slave);
        let master = Arc::new(Mutex::new(pair.master));
        static SESSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let session = SESSION
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |n| n.checked_add(1),
            )
            .expect("PTY session exhausted");
        let input_output = output_tx.producer();
        let input_ready = input_output.clone();
        let writer_master = master.clone();
        let input = InputWriter::spawn(
            writer,
            session,
            input_output,
            InputLimits::default(),
            move || {
                let _keep_descriptor_alive = &writer_master;
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                {
                    native_write_ready(fd, &input_ready)
                }
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                {
                    if input_ready.is_closed() {
                        Err(std::io::ErrorKind::Interrupted.into())
                    } else {
                        Ok(())
                    }
                }
            },
        );
        let writer = match input {
            Ok(input) => input,
            Err(e) => {
                kill_owned_child(&child);
                return Err(e.into());
            }
        };
        let output = output_tx.producer();
        let reader_output = output.clone();
        let reader_master = master.clone();
        let reader = std::thread::Builder::new()
            .name("pty-stdout".into())
            .spawn(move || {
                let _keep_descriptor_alive = &reader_master;
                pump_output_ready(&mut reader, session, &reader_output, || {
                    #[cfg(any(target_os = "linux", target_os = "macos"))]
                    {
                        native_read_ready(Some(fd), &reader_output)
                    }
                    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                    {
                        !reader_output.is_closed()
                    }
                });
            });
        let reader = match reader {
            Ok(reader) => reader,
            Err(e) => {
                writer.stop();
                kill_owned_child(&child);
                writer.join();
                return Err(e.into());
            }
        };
        Ok(Self {
            writer,
            master,
            child,
            reader_handle: Mutex::new(Some(reader)),
            output,
            session,
        })
    }

    /// Admit a whole raw packet, never wait for OS I/O/output drainage. Ok means
    /// accepted, NOT already flushed; the session/sequence completion reports
    /// success or an explicit failure and known delivered prefix. Full/oversize/
    /// closed errors accept zero bytes. Queue+active bytes/packets are bounded.
    pub fn write(&self, data: &[u8]) -> std::io::Result<()> {
        if self.writer.output.is_closed() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Terminal transport closed: no bytes accepted",
            ));
        }
        self.writer.queue.admit(data)
    }
    pub fn resize(&self, rows: u16, cols: u16) -> std::io::Result<()> {
        self.master
            .lock()
            .map_err(|e| std::io::Error::other(e.to_string()))?
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| std::io::Error::other(e.to_string()))
    }
    pub fn is_alive(&self) -> bool {
        self.child
            .lock()
            .ok()
            .is_some_and(|mut child| matches!(child.try_wait(), Ok(None)))
    }
    pub fn session(&self) -> u64 {
        self.session
    }
    pub fn shutdown(&self) {
        self.output.stop();
        self.writer.stop();
        kill_owned_child(&self.child);
        self.writer.join();
        if let Some(reader) = self.reader_handle.lock().unwrap().take() {
            let _ = reader.join();
        }
    }
}
impl Drop for PtyProcess {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn kill_owned_child(child: &Mutex<Box<dyn portable_pty::Child + Send + Sync>>) {
    if let Ok(mut child) = child.lock() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn pump_output_ready(
    reader: &mut impl Read,
    session: u64,
    output: &EventSender,
    mut ready: impl FnMut() -> bool,
) {
    let mut buf = [0u8; 4096];
    while !output.is_closed() && ready() {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if output
                    .blocking_send(Event::TerminalOutput {
                        session,
                        data: buf[..n].to_vec(),
                    })
                    .is_err()
                {
                    return;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                ) =>
            {
                continue
            }
            Err(_) => break,
        }
    }
    let _ = output.blocking_send(Event::TerminalClosed { session });
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn native_nonblocking(fd: Option<std::os::fd::RawFd>) -> std::io::Result<std::os::fd::RawFd> {
    let fd = fd.ok_or_else(|| std::io::Error::other("native PTY lacks cancellable descriptor"))?;
    // SAFETY: borrowed descriptor owned by master throughout setup/workers.
    // O_NONBLOCK is shared by portable-pty's duplicated writer/reader, so read
    // WouldBlock must be retried as well. No ownership transfer or new handle.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(fd)
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn native_write_ready(fd: std::os::fd::RawFd, output: &EventSender) -> std::io::Result<()> {
    while !output.is_closed() {
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: initialized single pollfd, held-live nonblocking master.
        let ready = unsafe { libc::poll(&mut poll, 1, 50) };
        if ready > 0 {
            if output.is_closed() {
                break;
            }
            if poll.revents & libc::POLLOUT != 0 {
                return Ok(());
            }
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        if ready < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return Err(std::io::Error::last_os_error());
        }
    }
    Err(std::io::ErrorKind::Interrupted.into())
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn native_read_ready(fd: Option<std::os::fd::RawFd>, output: &EventSender) -> bool {
    let Some(fd) = fd else {
        return !output.is_closed();
    };
    while !output.is_closed() {
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialized pollfd, held-live master, finite timeout.
        let ready = unsafe { libc::poll(&mut poll, 1, 50) };
        if ready > 0 {
            return !output.is_closed();
        }
        if ready < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    struct TestGate(Arc<(Mutex<bool>, Condvar)>);
    impl TestGate {
        fn new() -> Self {
            Self(Arc::new((Mutex::new(false), Condvar::new())))
        }
        fn open(&self) {
            *self.0 .0.lock().unwrap() = true;
            self.0 .1.notify_all();
        }
    }
    impl Drop for TestGate {
        fn drop(&mut self) {
            self.open();
        }
    }

    fn input_state(writer: &InputWriter) -> (usize, usize) {
        let state = writer.queue.state.lock().unwrap();
        (state.packets, state.bytes)
    }

    #[tokio::test]
    async fn transport_stdin_pending_full_packet_and_byte_budgets_preserve_whole_key_paste_fifo() {
        struct Writer {
            entered: Option<std::sync::mpsc::SyncSender<()>>,
            gate: Arc<(Mutex<bool>, Condvar)>,
            bytes: Arc<Mutex<Vec<u8>>>,
        }
        impl Write for Writer {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                if let Some(tx) = self.entered.take() {
                    tx.send(()).unwrap();
                }
                let _open = self
                    .gate
                    .1
                    .wait_while(self.gate.0.lock().unwrap(), |open| !*open)
                    .unwrap();
                // Force actual partial OS writes, without splitting admission.
                let n = data.len().min(2);
                self.bytes.lock().unwrap().extend_from_slice(&data[..n]);
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for (packets, bytes, rejection) in [(3, 20, b"z".as_slice()), (8, 8, b"zzz".as_slice())] {
            let (tx, mut rx) = crate::event::event_channel(crate::event::TransportLimits {
                slots: 1,
                ..Default::default()
            });
            let captured = Arc::new(Mutex::new(Vec::new()));
            let gate = TestGate::new();
            let (entered, entered_rx) = std::sync::mpsc::sync_channel(1);
            let writer = InputWriter::spawn(
                Box::new(Writer {
                    entered: Some(entered),
                    gate: gate.0.clone(),
                    bytes: captured.clone(),
                }),
                23,
                tx,
                InputLimits {
                    packets,
                    bytes,
                    packet_bytes: 4,
                },
                || Ok(()),
            )
            .unwrap();
            writer.queue.admit(b"ab").unwrap();
            let reached = entered_rx.recv_timeout(Duration::from_secs(1));
            let key = writer.queue.admit(b"\x03");
            let paste = writer.queue.admit("é\n".as_bytes());
            let pending = input_state(&writer);
            let rejected = writer.queue.admit(rejection);
            let unchanged = input_state(&writer);
            let too_large = writer.queue.admit(b"12345");
            gate.open();
            let mut completions = Vec::new();
            for _ in 0..3 {
                completions.push(tokio::time::timeout(Duration::from_secs(1), rx.recv()).await);
            }
            writer.stop();
            writer.join();
            assert!(reached.is_ok());
            key.unwrap();
            paste.unwrap();
            assert_eq!(pending, (3, 6)); // active packet IS included
            assert_eq!(unchanged, pending);
            assert_eq!(rejected.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
            assert_eq!(
                too_large.unwrap_err().kind(),
                std::io::ErrorKind::InvalidInput
            );
            assert_eq!(input_state(&writer), (0, 0));
            assert_eq!(*captured.lock().unwrap(), b"ab\x03\xc3\xa9\n");
            for (index, event) in completions.into_iter().enumerate() {
                assert!(
                    matches!(event.unwrap().unwrap(), Event::TerminalInputComplete { session: 23, sequence, outcome: InputOutcome::Written } if sequence == index as u64 + 1)
                );
            }
        }
    }

    #[tokio::test]
    async fn transport_stdin_injected_error_panic_and_zero_write_finish_all_accepted_packets() {
        struct Writer {
            call: usize,
            mode: u8,
        }
        impl Write for Writer {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                self.call += 1;
                if self.call == 1 {
                    return Ok(1);
                }
                match self.mode {
                    0 => Err(std::io::ErrorKind::BrokenPipe.into()),
                    1 => panic!("injected PTY writer panic"),
                    _ => Ok(0),
                }
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for mode in 0..3 {
            let (tx, mut rx) = crate::event::event_channel(Default::default());
            let gate = TestGate::new();
            let worker_gate = gate.0.clone();
            let (entered, entered_rx) = std::sync::mpsc::sync_channel(1);
            let mut entered = Some(entered);
            let writer = InputWriter::spawn(
                Box::new(Writer { call: 0, mode }),
                31,
                tx,
                InputLimits::default(),
                move || {
                    if let Some(tx) = entered.take() {
                        tx.send(()).unwrap();
                    }
                    let _open = worker_gate
                        .1
                        .wait_while(worker_gate.0.lock().unwrap(), |open| !*open)
                        .unwrap();
                    Ok(())
                },
            )
            .unwrap();
            writer.queue.admit(b"first").unwrap();
            let reached = entered_rx.recv_timeout(Duration::from_secs(1));
            let queued = writer.queue.admit(b"second");
            gate.open();
            let first = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await;
            let second = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await;
            writer.stop();
            writer.join();
            assert!(reached.is_ok());
            queued.unwrap();
            assert!(
                matches!(first.unwrap().unwrap(), Event::TerminalInputComplete { session: 31, sequence: 1, outcome: InputOutcome::Failed { written: 1, reason } } if reason == if mode == 1 { "PTY writer panicked; remainder cancelled" } else { "PTY I/O failed; remainder cancelled" })
            );
            assert!(matches!(
                second.unwrap().unwrap(),
                Event::TerminalInputComplete {
                    session: 31,
                    sequence: 2,
                    outcome: InputOutcome::Failed { written: 0, .. }
                }
            ));
            assert_eq!(
                writer.queue.admit(b"retry").unwrap_err().kind(),
                std::io::ErrorKind::BrokenPipe
            );
            assert_eq!(input_state(&writer), (0, 0));
        }
    }

    #[tokio::test]
    async fn transport_stdin_completion_pressure_holds_retention_until_consumer_close() {
        struct Writer(std::sync::mpsc::SyncSender<()>);
        impl Write for Writer {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                Ok(data.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.0.send(()).unwrap();
                Ok(())
            }
        }
        let (tx, mut rx) = crate::event::event_channel(crate::event::TransportLimits {
            slots: 1,
            ..Default::default()
        });
        tx.send(Event::Paste("occupied".into())).await.unwrap();
        let (flushed, flushed_rx) = std::sync::mpsc::sync_channel(1);
        let writer = InputWriter::spawn(
            Box::new(Writer(flushed)),
            1,
            tx.producer(),
            InputLimits {
                packets: 1,
                bytes: 3,
                packet_bytes: 3,
            },
            || Ok(()),
        )
        .unwrap();
        writer.queue.admit(b"abc").unwrap();
        let reached = flushed_rx.recv_timeout(Duration::from_secs(1));
        let retained = input_state(&writer);
        let full = writer.queue.admit(b"x");
        rx.close(); // wakes completion sender, without stdin shutdown relay
        writer.join();
        assert!(reached.is_ok());
        assert_eq!(retained, (1, 3));
        assert_eq!(full.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        assert_eq!(input_state(&writer), (0, 0));
        assert!(writer.queue.closed());
        assert!(matches!(rx.recv().await, Some(Event::Paste(text)) if text == "occupied"));
        assert!(rx.recv().await.is_none());
    }

    fn pump_output(reader: &mut impl Read, session: u64, output: &EventSender) {
        pump_output_ready(reader, session, output, || true);
    }

    #[tokio::test]
    async fn transport_stdin_retry_interrupt_wouldblock_then_report_flush_failure_prefix() {
        struct Writer {
            calls: usize,
            bytes: Arc<Mutex<Vec<u8>>>,
        }
        impl Write for Writer {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.calls += 1;
                match self.calls {
                    1 => Err(std::io::ErrorKind::Interrupted.into()),
                    2 => Err(std::io::ErrorKind::WouldBlock.into()),
                    _ => {
                        self.bytes.lock().unwrap().push(data[0]);
                        Ok(1)
                    }
                }
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        let (tx, mut rx) = crate::event::event_channel(Default::default());
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer = InputWriter::spawn(
            Box::new(Writer {
                calls: 0,
                bytes: bytes.clone(),
            }),
            17,
            tx,
            InputLimits::default(),
            || Ok(()),
        )
        .unwrap();
        writer.queue.admit(b"\x03ab").unwrap();
        let completion = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await;
        writer.stop();
        writer.join();
        assert_eq!(*bytes.lock().unwrap(), b"\x03ab");
        assert!(matches!(
            completion.unwrap().unwrap(),
            Event::TerminalInputComplete {
                session: 17,
                sequence: 1,
                outcome: InputOutcome::Failed { written: 3, .. }
            }
        ));
        assert_eq!(input_state(&writer), (0, 0));
    }

    #[test]
    fn transport_stdin_sequence_exhaustion_rejects_before_any_retention() {
        let queue = InputQueue::new(InputLimits::default());
        queue.state.lock().unwrap().next = u64::MAX;
        assert_eq!(
            queue.admit(b"whole").unwrap_err().kind(),
            std::io::ErrorKind::Other
        );
        let state = queue.state.lock().unwrap();
        assert_eq!((state.packets, state.bytes), (0, 0));
        assert!(state.queue.is_empty());
    }

    #[tokio::test]
    async fn transport_stdin_close_at_injected_ready_barrier_never_starts_cancelled_packet() {
        let (tx, mut rx) = crate::event::event_channel(Default::default());
        let bytes = Arc::new(Mutex::new(Vec::new()));
        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl Write for Capture {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_gate = gate.clone();
        let mut entered_tx = Some(entered_tx);
        let writer = InputWriter::spawn(
            Box::new(Capture(bytes.clone())),
            7,
            tx,
            InputLimits::default(),
            move || {
                if let Some(tx) = entered_tx.take() {
                    tx.send(()).unwrap();
                }
                let (lock, wake) = &*worker_gate;
                let _open = wake
                    .wait_while(lock.lock().unwrap(), |open| !*open)
                    .unwrap();
                Ok(())
            },
        )
        .unwrap();
        writer.queue.admit(b"accepted").unwrap();
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        writer.queue.admit(b"queued").unwrap();
        writer.queue.admit(b"").unwrap();
        writer.queue.close();
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        let first = rx.recv().await.unwrap();
        let second = rx.recv().await.unwrap();
        let empty = rx.recv().await.unwrap();
        writer.join();
        assert!(
            bytes.lock().unwrap().is_empty(),
            "close must be checked after injected readiness, before writing"
        );
        for (event, expected_sequence) in [(first, 1), (second, 2), (empty, 3)] {
            assert!(
                matches!(event, Event::TerminalInputComplete { session: 7, sequence, outcome: InputOutcome::Failed { written: 0, .. } } if sequence == expected_sequence)
            );
        }
        let state = writer.queue.state.lock().unwrap();
        assert_eq!((state.packets, state.bytes), (0, 0));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn transport_stdin_native_ctrl_paste_fifo_full_duplex_with_one_output_slot() {
        let (tx, mut rx) = crate::event::event_channel(crate::event::TransportLimits {
            slots: 1,
            ..Default::default()
        });
        let pty = PtyProcess::spawn("/bin/cat", &env::temp_dir(), 24, 80, tx).unwrap();
        let fd = pty.master.lock().unwrap().as_raw_fd().unwrap();
        unsafe {
            let mut termios = std::mem::zeroed();
            assert_eq!(libc::tcgetattr(fd, &mut termios), 0);
            libc::cfmakeraw(&mut termios);
            assert_eq!(libc::tcsetattr(fd, libc::TCSANOW, &termios), 0);
        }
        let paste = b"literal\n".repeat(131072);
        let controls = b"\x03\x04\x1b[A\x1b[31m";
        let tail = "é\nliteral q not a command".as_bytes();
        let expected = [paste.as_slice(), controls, tail].concat();
        for packet in [paste.as_slice(), controls, tail] {
            pty.write(packet).unwrap();
        }
        let observed = tokio::time::timeout(Duration::from_secs(5), async {
            let mut bytes = Vec::new();
            let mut completions = Vec::new();
            while bytes.len() < expected.len() || completions.len() < 3 {
                match rx.recv().await {
                    Some(Event::TerminalOutput { session, data }) if session == pty.session() => {
                        bytes.extend(data)
                    }
                    Some(Event::TerminalInputComplete {
                        session,
                        sequence,
                        outcome,
                    }) if session == pty.session() => completions.push((sequence, outcome)),
                    _ => break,
                }
            }
            (bytes, completions)
        })
        .await;
        rx.close();
        pty.shutdown();
        let (actual, completions) = observed.unwrap();
        assert!(
            actual == expected,
            "cat full-duplex FIFO mismatch ({} versus {} bytes)",
            actual.len(),
            expected.len()
        );
        assert_eq!(
            completions,
            [
                (1, InputOutcome::Written),
                (2, InputOutcome::Written),
                (3, InputOutcome::Written)
            ]
        );
        assert!(!pty.is_alive());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn transport_stdin_native_shutdown_joins_both_workers_under_full_duplex_pressure() {
        let (tx, mut rx) = crate::event::event_channel(crate::event::TransportLimits {
            slots: 1,
            ..Default::default()
        });
        let pty = PtyProcess::spawn("/bin/cat", &env::temp_dir(), 24, 80, tx).unwrap();
        let paste = b"literal\n".repeat(131072);
        pty.write(&paste).unwrap();
        let started = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
        // A real output chunk demonstrates the native writer/reader are active;
        // with no more draining, output has only one slot and input cannot flush.
        let second = pty.write(&paste);
        rx.close();
        pty.shutdown(); // no detached kernel writer or Tokio blocking task
        assert!(matches!(
            started.unwrap(),
            Some(Event::TerminalOutput { .. })
        ));
        second.unwrap();
        assert!(!pty.is_alive());
        assert!(pty.reader_handle.lock().unwrap().is_none());
        assert!(pty.writer.handle.lock().unwrap().is_none());
        assert_eq!(input_state(&pty.writer), (0, 0));
        assert_eq!(
            pty.write(b"late").unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn transport_stdin_large_literal_paste_admission_never_waits_for_undrained_output() {
        let (tx, mut rx) = crate::event::event_channel(Default::default());
        let pty = Arc::new(PtyProcess::spawn("/bin/cat", &env::temp_dir(), 24, 80, tx).unwrap());
        let fd = pty.master.lock().unwrap().as_raw_fd().unwrap();
        // Raw mode makes cat an exact full-duplex byte oracle, including Ctrl
        // bytes; no terminal echo/canonical buffering changes the assertion.
        unsafe {
            let mut termios = std::mem::zeroed();
            assert_eq!(libc::tcgetattr(fd, &mut termios), 0);
            libc::cfmakeraw(&mut termios);
            assert_eq!(libc::tcsetattr(fd, libc::TCSANOW, &termios), 0);
        }
        let expected = b"literal\n".repeat(131072); // valid, whole 1 MiB Paste
        let write_bytes = expected.clone();
        let writer_pty = pty.clone();
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
        let caller = std::thread::spawn(move || {
            let result = writer_pty.write(&write_bytes);
            done_tx.send(result).unwrap();
        });
        // This caller models main before it can receive again. Timeouts guard
        // failures, not scheduling assertions; native output must fill first.
        let admitted_without_drain = done_rx.recv_timeout(std::time::Duration::from_secs(1));
        let drained = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut actual = Vec::new();
            let mut completed = None;
            while actual.len() < expected.len() || completed.is_none() {
                match rx.recv().await {
                    Some(Event::TerminalOutput { session, data }) if session == pty.session() => {
                        actual.extend(data)
                    }
                    Some(Event::TerminalInputComplete {
                        session,
                        sequence: 1,
                        outcome,
                    }) if session == pty.session() => completed = Some(outcome),
                    _ => break,
                }
            }
            (actual, completed)
        })
        .await;
        // Unconditionally release the old synchronous writer by draining before
        // cleanup. No detached writer, forgotten child or timeout-as-green.
        rx.close();
        pty.shutdown();
        caller.join().unwrap();
        assert!(
            admitted_without_drain.is_ok(),
            "valid 1 MiB admission must not wait for its own undrained output"
        );
        admitted_without_drain.unwrap().unwrap();
        let (actual, completed) = drained.unwrap();
        assert_eq!(completed, Some(InputOutcome::Written));
        assert!(
            actual == expected,
            "whole cat bytes differ (actual={}, expected={})",
            actual.len(),
            expected.len()
        );
        assert!(!pty.is_alive());
    }

    #[tokio::test]
    async fn transport_pty_eof_follows_all_ordered_bytes_with_a_lifecycle_event() {
        let (tx, mut rx) = crate::event::event_channel(crate::event::TransportLimits {
            slots: 1,
            ..Default::default()
        });
        let reader = tokio::task::spawn_blocking(move || {
            pump_output(&mut std::io::Cursor::new(b"\x1b[31mred\x1b[0m"), 41, &tx);
        });
        assert!(
            matches!(rx.recv().await, Some(Event::TerminalOutput { session: 41, data }) if data == b"\x1b[31mred\x1b[0m")
        );
        reader.await.unwrap();
        assert!(
            matches!(rx.try_recv(), Ok(Event::TerminalClosed { session: 41 })),
            "EOF needs ordered lifecycle delivery"
        );
    }

    #[tokio::test]
    async fn transport_real_reader_adapter_preserves_flooded_bytes_and_caps_chunks() {
        let bytes = [b"\x1b[31m".as_slice(), &[b'x'; 4096], b"\x1b[0m"]
            .concat()
            .repeat(8);
        let expected = bytes.clone();
        let (tx, mut rx) = crate::event::event_channel(crate::event::TransportLimits {
            slots: 1,
            ..Default::default()
        });
        let reader = tokio::task::spawn_blocking(move || {
            pump_output(&mut std::io::Cursor::new(bytes), 12, &tx)
        });
        let mut actual = Vec::new();
        loop {
            match rx.recv().await.unwrap() {
                Event::TerminalOutput { session: 12, data } => {
                    assert!(data.len() <= 4096);
                    actual.extend(data);
                }
                Event::TerminalClosed { session: 12 } => break,
                event => panic!("unexpected {event:?}"),
            }
        }
        reader.await.unwrap();
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn transport_reader_close_wakes_full_queue_without_killing_unrelated_processes() {
        struct Reader {
            calls: usize,
            second: Option<tokio::sync::oneshot::Sender<()>>,
        }
        impl Read for Reader {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                self.calls += 1;
                if self.calls == 2 {
                    self.second.take().unwrap().send(()).unwrap();
                }
                out[0] = self.calls as u8;
                Ok(1)
            }
        }
        let (tx, mut rx) = crate::event::event_channel(crate::event::TransportLimits {
            slots: 1,
            ..Default::default()
        });
        let (second, ready) = tokio::sync::oneshot::channel();
        let worker = tokio::task::spawn_blocking(move || {
            pump_output(
                &mut Reader {
                    calls: 0,
                    second: Some(second),
                },
                3,
                &tx,
            )
        });
        ready.await.unwrap(); // first chunk is queued; second read is complete
        rx.close();
        worker.await.unwrap();
        assert!(
            matches!(rx.recv().await, Some(Event::TerminalOutput { session: 3, data }) if data == [1])
        );
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn transport_reader_interrupt_retries_and_error_emits_lifecycle() {
        struct Reader(bool);
        impl Read for Reader {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                if !self.0 {
                    self.0 = true;
                    Err(std::io::ErrorKind::Interrupted.into())
                } else {
                    Err(std::io::Error::other("injected read failure"))
                }
            }
        }
        let (tx, mut rx) = crate::event::event_channel(Default::default());
        let worker = tokio::task::spawn_blocking(move || pump_output(&mut Reader(false), 9, &tx));
        assert!(matches!(
            rx.recv().await,
            Some(Event::TerminalClosed { session: 9 })
        ));
        worker.await.unwrap();
    }

    #[tokio::test]
    async fn transport_tiny_byte_limit_refuses_before_starting_any_shell() {
        let (tx, _rx) = crate::event::event_channel(crate::event::TransportLimits {
            slots: 1,
            retained_bytes: 1024,
            event_bytes: 1024,
        });
        assert!(
            PtyProcess::spawn("/nonexistent-no-shell-start", &env::temp_dir(), 24, 80, tx)
                .err()
                .unwrap()
                .to_string()
                .contains("4096")
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn transport_native_reader_stop_returns_while_owned_shell_is_still_alive() {
        let (tx, mut rx) = crate::event::event_channel(Default::default());
        let pty = PtyProcess::spawn("/bin/sh", &env::temp_dir(), 24, 80, tx).unwrap();
        // A real prompt confirms the reader has begun. No arbitrary sleep.
        tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        pty.output.stop();
        let reader = pty.reader_handle.lock().unwrap().take().unwrap();
        let mut reader = tokio::task::spawn_blocking(move || reader.join());
        let stopped = tokio::time::timeout(std::time::Duration::from_secs(1), &mut reader).await;
        let alive = pty.is_alive();
        // Always clean up our child, including the expected red timeout.
        pty.shutdown();
        if stopped.is_err() {
            reader.await.unwrap().unwrap();
        }
        assert!(
            stopped.is_ok(),
            "producer stop must wake idle native reader"
        );
        assert!(alive, "reader cancellation must not kill the owned shell");
    }

    #[tokio::test]
    async fn test_spawn_and_is_alive() {
        let (tx, _rx) = crate::event::event_channel(Default::default());
        let cwd = env::temp_dir();
        let pty = PtyProcess::spawn("/bin/sh", &cwd, 24, 80, tx);
        assert!(pty.is_ok(), "PTY should spawn successfully");
        let pty = pty.unwrap();
        assert!(pty.is_alive(), "PTY process should be alive after spawn");
        pty.shutdown();
    }

    #[tokio::test]
    async fn test_write_to_pty() {
        let (tx, _rx) = crate::event::event_channel(Default::default());
        let cwd = env::temp_dir();
        let pty = PtyProcess::spawn("/bin/sh", &cwd, 24, 80, tx).unwrap();
        let result = pty.write(b"echo hello\n");
        assert!(result.is_ok(), "Writing to PTY should succeed");
        pty.shutdown();
    }

    #[tokio::test]
    async fn test_shutdown() {
        let (tx, _rx) = crate::event::event_channel(Default::default());
        let cwd = env::temp_dir();
        let pty = PtyProcess::spawn("/bin/sh", &cwd, 24, 80, tx).unwrap();
        pty.shutdown();
        // shutdown waits for our owned child, so no timing sleep is needed.
        assert!(!pty.is_alive(), "PTY should not be alive after shutdown");
    }

    #[tokio::test]
    async fn test_resize() {
        let (tx, _rx) = crate::event::event_channel(Default::default());
        let cwd = env::temp_dir();
        let pty = PtyProcess::spawn("/bin/sh", &cwd, 24, 80, tx).unwrap();
        let result = pty.resize(40, 120);
        assert!(result.is_ok(), "Resize should succeed");
        pty.shutdown();
    }
}
