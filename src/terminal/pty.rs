//! PTY process management: spawning, I/O, resize, and lifecycle.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc;

/// A PTY child process wrapping a system shell.
pub struct PtyProcess {
    /// Writer to send input to the shell.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// The master PTY handle (needed for resize).
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    /// Child process handle.
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    /// Handle for the reader task (dropped on shutdown).
    _reader_handle: tokio::task::JoinHandle<()>,
}

impl PtyProcess {
    /// Spawn a new PTY shell process.
    ///
    /// - `shell`: path to the shell executable (e.g. `/bin/bash`)
    /// - `cwd`: working directory for the shell
    /// - `rows`, `cols`: initial terminal size
    /// - `output_tx`: channel to send PTY output bytes to the main event loop
    pub fn spawn(
        shell: &str,
        cwd: &Path,
        rows: u16,
        cols: u16,
        output_tx: mpsc::UnboundedSender<Vec<u8>>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let pty_system = native_pty_system();

        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(shell);
        cmd.cwd(cwd);

        let child = pair.slave.spawn_command(cmd)?;

        // Get writer and reader from master
        let writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;

        let master: Box<dyn MasterPty + Send> = pair.master;

        let writer = Arc::new(Mutex::new(writer));
        let master = Arc::new(Mutex::new(master));
        let child = Arc::new(Mutex::new(child));

        // Spawn async reader task
        let reader_handle = tokio::task::spawn_blocking(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        // EOF — shell exited
                        break;
                    }
                    Ok(n) => {
                        if output_tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
        });

        Ok(Self {
            writer,
            master,
            child,
            _reader_handle: reader_handle,
        })
    }

    /// Write raw bytes to the PTY stdin.
    pub fn write(&self, data: &[u8]) -> std::io::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        writer.write_all(data)?;
        writer.flush()
    }

    /// Resize the PTY to new dimensions.
    pub fn resize(&self, rows: u16, cols: u16) -> std::io::Result<()> {
        let master = self
            .master
            .lock()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    /// Check if the child process is still running.
    pub fn is_alive(&self) -> bool {
        if let Ok(mut child) = self.child.lock() {
            // try_wait returns Ok(Some(status)) if exited, Ok(None) if still running
            match child.try_wait() {
                Ok(Some(_)) => false,
                Ok(None) => true,
                Err(_) => false,
            }
        } else {
            false
        }
    }

    /// Shut down the PTY process: kill + wait.
    pub fn shutdown(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_spawn_and_is_alive() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let cwd = env::temp_dir();
        let pty = PtyProcess::spawn("/bin/sh", &cwd, 24, 80, tx);
        assert!(pty.is_ok(), "PTY should spawn successfully");
        let pty = pty.unwrap();
        assert!(pty.is_alive(), "PTY process should be alive after spawn");
        pty.shutdown();
    }

    #[tokio::test]
    async fn test_write_to_pty() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let cwd = env::temp_dir();
        let pty = PtyProcess::spawn("/bin/sh", &cwd, 24, 80, tx).unwrap();
        let result = pty.write(b"echo hello\n");
        assert!(result.is_ok(), "Writing to PTY should succeed");
        pty.shutdown();
    }

    #[tokio::test]
    async fn test_shutdown() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let cwd = env::temp_dir();
        let pty = PtyProcess::spawn("/bin/sh", &cwd, 24, 80, tx).unwrap();
        pty.shutdown();
        // After shutdown, is_alive should return false (may need brief wait)
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!pty.is_alive(), "PTY should not be alive after shutdown");
    }

    #[tokio::test]
    async fn test_resize() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let cwd = env::temp_dir();
        let pty = PtyProcess::spawn("/bin/sh", &cwd, 24, 80, tx).unwrap();
        let result = pty.resize(40, 120);
        assert!(result.is_ok(), "Resize should succeed");
        pty.shutdown();
    }
}
