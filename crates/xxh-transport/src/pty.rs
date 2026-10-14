//! Interactive session over a locally allocated PTY, shared by the CLI-driven
//! backends (`docker|podman exec -it`, `kubectl exec -it`) — 002 R5/C-C8..C-C11,
//! extracted for 016 T001.
//!
//! The runtime CLI runs on the slave end as its controlling terminal; we proxy
//! the client's stdio through the master, propagate SIGWINCH and return the
//! CLI's exit code, which is the exec session's exit code.

use std::process::Stdio;

use tokio::process::Command;

use crate::TransportError;
use crate::tty::{RawModeGuard, local_tty_size};

/// Run `cmd` (an `exec -it`-style invocation) on a fresh local PTY sized to the
/// client terminal (falling back to `cols`×`rows`) and return its exit code.
/// Given `own_stderr`, the CLI's stderr is a pipe handed to it instead of the
/// terminal (kubectl's exit note is filtered there, 016 R4); its result is
/// returned alongside the code.
pub(crate) async fn run_in_local_pty<T>(
    mut cmd: Command,
    cols: u16,
    rows: u16,
    own_stderr: Option<impl FnOnce(tokio::process::ChildStderr) -> T>,
) -> Result<(i32, Option<T>), TransportError> {
    // Local PTY pair; the CLI sits on the slave, we proxy stdio through the
    // master (R5/C-C8). The CLI translates resizes of its tty into the exec
    // session, so SIGWINCH handling is: resize master + signal the child (C-C9).
    let (cols, rows) = local_tty_size().unwrap_or((cols, rows));
    let pty = Pty::open(cols, rows)?;

    cmd.stdin(pty.slave_stdio()?).stdout(pty.slave_stdio()?);
    if own_stderr.is_some() {
        cmd.stderr(Stdio::piped());
    } else {
        cmd.stderr(pty.slave_stdio()?);
    }
    // The CLI must see the slave as its controlling terminal for -t to work.
    // SAFETY: setsid + TIOCSCTTY on the freshly-dup'ed stdin, pre-exec only.
    #[allow(unsafe_code)]
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    let stderr_result = match (own_stderr, child.stderr.take()) {
        (Some(take), Some(stderr)) => Some(take(stderr)),
        _ => None,
    };
    // The command still owns its copies of the slave: while one is open here
    // the master never reaches EOF and the output pump waits forever after
    // the exec ends (found by 011: `-t` hung on every container).
    drop(cmd);
    pty.close_slave();

    // Raw local terminal for the interactive phase; the guard restores it on
    // scope exit, panics and container death included (C-C10/C-C11).
    let _raw = RawModeGuard::enter();

    // stdout pump: master → local stdout until EOF/EIO (child gone).
    let master_out = pty.master_clone()?;
    let out_task = tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let mut src = master_out;
        let mut dst = std::io::stdout();
        let mut buf = [0u8; 8192];
        loop {
            match src.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if dst.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    let _ = dst.flush();
                }
            }
        }
    });

    // stdin pump: local stdin → master. Left blocked on the local read after
    // the session ends (same as the SSH path); the runtime shutdown reaps it.
    let master_in = pty.master_clone()?;
    let in_task = tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let mut src = std::io::stdin();
        let mut dst = master_in;
        let mut buf = [0u8; 8192];
        loop {
            match src.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if dst.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Resize propagation (C-C9): local SIGWINCH → new winsize on the master
    // + SIGWINCH to the CLI child, which re-reads its tty size.
    let master_fd = pty.master_raw_fd();
    let child_pid = child.id();
    let winch_task = tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut winch) = signal(SignalKind::window_change()) else {
            return;
        };
        while winch.recv().await.is_some() {
            if let Some((c, r)) = local_tty_size() {
                set_winsize(master_fd, c, r);
            }
            if let Some(pid) = child_pid {
                // SAFETY: signalling our own child process.
                #[allow(unsafe_code)]
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGWINCH);
                }
            }
        }
    });

    // The exec's exit code is the session's exit code (C-C10); target death
    // ends the child, EOFs the master and lands here — no hang (C-C11).
    let status = child.wait().await?;
    let _ = out_task.await; // drain remaining output before restoring the tty
    winch_task.abort();
    in_task.abort();
    Ok((status.code().unwrap_or(-1), stderr_result))
}

/// A locally allocated PTY pair (openpty). The master lives as long as the value;
/// the slave is handed to the child's stdio and closed in the parent after spawn.
struct Pty {
    master: std::fs::File,
    slave: std::cell::RefCell<Option<std::os::fd::OwnedFd>>,
}

impl Pty {
    fn open(cols: u16, rows: u16) -> Result<Self, TransportError> {
        use std::os::fd::FromRawFd;
        let mut master: libc::c_int = 0;
        let mut slave: libc::c_int = 0;
        // `mut`: Apple's libc declares `winp` as `*mut winsize` (Linux: `*const`);
        // a `*mut` pointer coerces to both.
        let mut ws: libc::winsize = winsize(cols, rows);
        // SAFETY: openpty fills two fresh fds we immediately take ownership of;
        // it only reads `winp`.
        #[allow(unsafe_code)]
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &raw mut ws,
            )
        };
        if rc != 0 {
            return Err(TransportError::Io(std::io::Error::last_os_error()));
        }
        // SAFETY: both fds are valid and owned exclusively by us from here on.
        #[allow(unsafe_code)]
        let (master, slave) = unsafe {
            (
                std::fs::File::from_raw_fd(master),
                std::os::fd::OwnedFd::from_raw_fd(slave),
            )
        };
        Ok(Self {
            master,
            slave: std::cell::RefCell::new(Some(slave)),
        })
    }

    /// A `Stdio` dup of the slave end for one child stdio slot.
    fn slave_stdio(&self) -> Result<Stdio, TransportError> {
        let slave = self.slave.borrow();
        let fd = slave
            .as_ref()
            .ok_or_else(|| TransportError::Channel("pty slave already closed".into()))?;
        Ok(Stdio::from(fd.try_clone().map_err(TransportError::Io)?))
    }

    /// Drop the parent's slave fd so master EOFs when the child exits.
    fn close_slave(&self) {
        self.slave.borrow_mut().take();
    }

    fn master_clone(&self) -> Result<std::fs::File, TransportError> {
        self.master.try_clone().map_err(TransportError::Io)
    }

    fn master_raw_fd(&self) -> std::os::fd::RawFd {
        use std::os::fd::AsRawFd;
        self.master.as_raw_fd()
    }
}

fn winsize(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

fn set_winsize(fd: std::os::fd::RawFd, cols: u16, rows: u16) {
    let ws = winsize(cols, rows);
    // SAFETY: TIOCSWINSZ with a valid winsize on our own master fd.
    #[allow(unsafe_code)]
    unsafe {
        libc::ioctl(fd, libc::TIOCSWINSZ, &ws);
    }
}
