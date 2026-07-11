//! Local-terminal helpers shared by the interactive PTY paths of every backend
//! (russh channel PTY, container exec PTY): raw mode with guaranteed restore and
//! window-size queries for SIGWINCH propagation.

/// Local terminal dimensions, `None` when stdin is not a tty (e.g. piped input).
pub(crate) fn local_tty_size() -> Option<(u16, u16)> {
    // SAFETY: read-only ioctl on fd 0 into a zeroed winsize struct.
    #[allow(unsafe_code)]
    unsafe {
        if libc::isatty(libc::STDIN_FILENO) != 1 {
            return None;
        }
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ, &mut ws) != 0
            || ws.ws_col == 0
            || ws.ws_row == 0
        {
            return None;
        }
        Some((ws.ws_col, ws.ws_row))
    }
}

/// Puts the local terminal into raw mode for the lifetime of the guard and
/// restores the saved settings on drop. `None` when stdin is not a tty.
pub(crate) struct RawModeGuard {
    saved: libc::termios,
}

impl RawModeGuard {
    pub(crate) fn enter() -> Option<Self> {
        // SAFETY: standard tcgetattr/cfmakeraw/tcsetattr sequence on fd 0.
        #[allow(unsafe_code)]
        unsafe {
            if libc::isatty(libc::STDIN_FILENO) != 1 {
                return None;
            }
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut t) != 0 {
                return None;
            }
            let saved = t;
            libc::cfmakeraw(&mut t);
            if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t) != 0 {
                return None;
            }
            Some(Self { saved })
        }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        // SAFETY: restores the termios captured in `enter` on the same fd.
        #[allow(unsafe_code)]
        unsafe {
            let _ = libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.saved);
        }
    }
}
