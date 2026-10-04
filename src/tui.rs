use std::io::{self, Stdout, Write};

use crossterm::{
    cursor::Show,
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::error::Result;

fn enable_screen(writer: &mut impl Write, mouse: bool) -> io::Result<()> {
    execute!(writer, EnterAlternateScreen, EnableBracketedPaste)?;
    if mouse {
        execute!(writer, EnableMouseCapture)?;
    }
    Ok(())
}

fn disable_screen(writer: &mut impl Write, mouse: bool) -> io::Result<()> {
    let paste = execute!(writer, DisableBracketedPaste);
    let mouse = if mouse {
        execute!(writer, DisableMouseCapture)
    } else {
        Ok(())
    };
    let screen = execute!(writer, LeaveAlternateScreen);
    let cursor = execute!(writer, Show);
    paste.and(mouse).and(screen).and(cursor)
}

/// Terminal wrapper that manages raw mode and alternate screen.
pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    mouse_enabled: bool,
    active: bool,
}

impl Tui {
    /// Initialize the terminal: enter alternate screen and enable raw mode.
    /// Optionally enables mouse capture.
    pub fn new(enable_mouse: bool) -> Result<Self> {
        let mut stdout = io::stdout();
        terminal::enable_raw_mode()?;
        let setup = (|| {
            enable_screen(&mut stdout, enable_mouse)?;
            Terminal::new(CrosstermBackend::new(stdout))
        })();
        let terminal = match setup {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = disable_screen(&mut io::stdout(), enable_mouse);
                let _ = terminal::disable_raw_mode();
                return Err(error.into());
            }
        };
        Ok(Self {
            terminal,
            mouse_enabled: enable_mouse,
            active: true,
        })
    }

    /// Restore the terminal to its original state.
    pub fn restore(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        let screen = disable_screen(self.terminal.backend_mut(), self.mouse_enabled);
        let raw = terminal::disable_raw_mode();
        screen.and(raw)?;
        self.active = false;
        Ok(())
    }

    /// Temporarily leave the alternate screen and disable raw mode + mouse.
    /// This lets the browser handle native text selection (for clipboard copy).
    #[allow(dead_code)]
    pub fn suspend(&mut self) -> Result<()> {
        self.restore()
    }

    /// Re-enter the alternate screen and restore raw mode + mouse.
    /// Call after `suspend()` to return to the TUI.
    #[allow(dead_code)]
    pub fn resume(&mut self) -> Result<()> {
        if self.active {
            return Ok(());
        }
        terminal::enable_raw_mode()?;
        if let Err(error) = enable_screen(self.terminal.backend_mut(), self.mouse_enabled) {
            let _ = disable_screen(self.terminal.backend_mut(), self.mouse_enabled);
            let _ = terminal::disable_raw_mode();
            return Err(error.into());
        }
        self.active = true;
        self.terminal.clear()?;
        Ok(())
    }

    /// Get a mutable reference to the underlying terminal for drawing.
    pub fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        // Covers ordinary event/render errors, not just normal exit and panics.
        let _ = self.restore();
    }
}

/// Install a panic hook that restores the terminal before printing panic info.
pub fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = terminal::disable_raw_mode();
        let _ = disable_screen(&mut io::stdout(), true);
        original_hook(panic_info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_lifecycle_enables_and_restores_bracketed_paste_without_mouse() {
        let mut output = Vec::new();
        enable_screen(&mut output, false).unwrap();
        disable_screen(&mut output, false).unwrap();
        let output = String::from_utf8(output).unwrap();
        let enabled = output.find("\x1b[?2004h").unwrap();
        let disabled = output.find("\x1b[?2004l").unwrap();
        assert!(enabled < disabled);
        assert!(output.contains("\x1b[?1049l"));
        assert!(!output.contains("\x1b[?1000h"));
    }

    #[test]
    fn bracketed_paste_cleanup_attempts_remaining_modes_after_write_failure() {
        struct FailFirst {
            fail: bool,
            output: Vec<u8>,
        }
        impl Write for FailFirst {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.fail {
                    self.fail = false;
                    return Err(io::Error::other("injected write failure"));
                }
                self.output.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut output = FailFirst {
            fail: true,
            output: Vec::new(),
        };
        assert!(disable_screen(&mut output, true).is_err());
        assert!(String::from_utf8(output.output)
            .unwrap()
            .contains("\x1b[?1049l"));
    }
}
