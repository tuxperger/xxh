//! Container-runtime transport backend (002-container-targets, research R2).
//!
//! Wraps the runtime CLI (`docker` | `podman` — one identical surface) via
//! `tokio::process`: `inspect` for connect/attach checks, `exec` as the ONLY data
//! channel (C-C5 — no `cp`, no mounts, no image writes), and a locally allocated
//! PTY pair driving `exec -it` for the interactive session (R5). The user's
//! runtime configuration (`DOCKER_HOST`, contexts, podman connections) is honoured
//! by inheriting the environment untouched (C-C4).
//!
//! Never logged, on any verbosity: runtime socket paths and `-e` env values
//! (C-T9/C-C15) — error messages are our own wording, not raw CLI stderr that may
//! embed socket paths.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::tty::{RawModeGuard, local_tty_size};
use crate::{
    AuthPolicy, ContainerRuntime, ExecOutput, PtySpec, ResolvedTarget, RuntimeSelector, Transport,
    TransportError,
};

/// Transport over the container runtime CLI. One instance serves one target;
/// `connect` resolves the runtime (auto-order docker → podman) and verifies the
/// container is running before anything else happens.
pub struct ContainerCliTransport {
    /// Resolved runtime; `Some` once connected.
    runtime: Option<ContainerRuntime>,
    reference: String,
    exec_user: Option<String>,
    timeout_s: u64,
}

impl ContainerCliTransport {
    pub fn new() -> Self {
        Self {
            runtime: None,
            reference: String::new(),
            exec_user: None,
            timeout_s: 10,
        }
    }

    /// The runtime picked by `connect` (for verbose reporting, C-A3/C-C15).
    pub fn runtime(&self) -> Option<ContainerRuntime> {
        self.runtime
    }

    fn connected(&self) -> Result<ContainerRuntime, TransportError> {
        self.runtime
            .ok_or_else(|| TransportError::Channel("not connected".into()))
    }

    /// Base `exec` invocation: `<runtime> exec [-u user] <ref> …`.
    fn exec_cmd(&self, interactive: bool, tty: bool) -> Result<Command, TransportError> {
        let rt = self.connected()?;
        let mut c = Command::new(rt.binary_name());
        c.arg("exec");
        if interactive {
            c.arg("-i");
        }
        if tty {
            c.arg("-t");
        }
        if let Some(u) = &self.exec_user {
            c.args(["-u", u]);
        }
        Ok(c)
    }
}

impl Default for ContainerCliTransport {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve a runtime selector to a live runtime (C-A3): an explicit choice is
/// verified as-is (no silent substitution, C-A4); `Auto` probes
/// [`ContainerRuntime::AUTO_ORDER`] and takes the first available one.
pub async fn resolve_runtime(
    selector: RuntimeSelector,
    timeout_s: u64,
) -> Result<ContainerRuntime, TransportError> {
    match selector {
        RuntimeSelector::Explicit(rt) => {
            check_runtime(rt, timeout_s).await?;
            Ok(rt)
        }
        RuntimeSelector::Auto => {
            let mut reasons = Vec::new();
            for rt in ContainerRuntime::AUTO_ORDER {
                match check_runtime(rt, timeout_s).await {
                    Ok(()) => return Ok(rt),
                    Err(e) => reasons.push(format!("{rt}: {e}")),
                }
            }
            Err(TransportError::BackendUnavailable(format!(
                "no container runtime available ({})",
                reasons.join("; ")
            )))
        }
    }
}

/// Availability of one runtime, with the three distinguishable failure states of
/// C-C1: not installed / daemon-socket unavailable / no socket access.
async fn check_runtime(rt: ContainerRuntime, timeout_s: u64) -> Result<(), TransportError> {
    let bin = rt.binary_name();
    if which(bin).is_none() {
        return Err(TransportError::BackendUnavailable(format!(
            "{rt} is not installed (`{bin}` not found on PATH)"
        )));
    }
    // `info` requires a live, reachable daemon/socket (podman answers locally,
    // which is exactly its "available" condition).
    let out = run_bounded(
        Command::new(bin).arg("info").stdin(Stdio::null()),
        timeout_s,
    )
    .await?;
    if out.status.success() {
        return Ok(());
    }
    // Own wording only: the CLI's stderr embeds the socket path (C-T9).
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    if stderr.contains("permission denied") {
        Err(TransportError::Auth(format!(
            "no access to the {rt} runtime socket (check your user's permissions)"
        )))
    } else {
        Err(TransportError::Connect(format!(
            "{rt} daemon/socket is not available"
        )))
    }
}

/// Run a CLI command with a hard upper bound so a hung daemon cannot hang us (C-C3).
async fn run_bounded(
    cmd: &mut Command,
    timeout_s: u64,
) -> Result<std::process::Output, TransportError> {
    tokio::time::timeout(Duration::from_secs(timeout_s), cmd.output())
        .await
        .map_err(|_| TransportError::Timeout(timeout_s))?
        .map_err(TransportError::Io)
}

/// The bootstrap protocol needs a POSIX `sh` inside the container (R3/C-C6);
/// both docker and podman report a missing binary as "executable file not found".
/// Turn that into an explicit, actionable delivery error before anything is written.
fn check_sh_present(cmd: &str, out: &std::process::Output) -> Result<(), TransportError> {
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("executable file not found")
        || (stderr.contains("no such file or directory") && cmd.starts_with("sh"))
    {
        return Err(TransportError::Channel(
            "the container has no POSIX `sh` — xxh needs a POSIX shell inside the \
             container to deliver the environment; scratch-style images without a \
             shell are not supported"
                .into(),
        ));
    }
    Ok(())
}

#[async_trait::async_trait]
impl Transport for ContainerCliTransport {
    async fn connect(
        &mut self,
        target: &ResolvedTarget,
        _auth: &AuthPolicy, // SSH-family only; access = runtime socket rights (C-T8)
    ) -> Result<(), TransportError> {
        // Container family only: an SSH target fails fast (C-T6).
        let ResolvedTarget::Container(target) = target else {
            return Err(TransportError::BackendUnavailable(
                "container runtime backend serves container targets only; SSH \
                 targets need an SSH backend"
                    .into(),
            ));
        };
        if target.reference.is_empty() {
            return Err(TransportError::Connect("empty container reference".into()));
        }
        self.timeout_s = target.connect_timeout_s;
        self.exec_user = target.exec_user.clone();

        // (1) CLI found, (2) daemon/socket answering — both distinguishable (C-C1).
        let rt = resolve_runtime(target.runtime, self.timeout_s).await?;

        // (3) the container exists and (4) is running.
        let out = run_bounded(
            Command::new(rt.binary_name())
                .args([
                    "inspect",
                    "--type",
                    "container",
                    "--format",
                    "{{.State.Running}}",
                    &target.reference,
                ])
                .stdin(Stdio::null()),
            self.timeout_s,
        )
        .await?;
        if !out.status.success() {
            return Err(TransportError::Connect(format!(
                "container not found: {}",
                target.reference
            )));
        }
        if String::from_utf8_lossy(&out.stdout).trim() != "true" {
            return Err(TransportError::Connect(format!(
                "container is stopped: {}",
                target.reference
            )));
        }

        self.runtime = Some(rt);
        self.reference = target.reference.clone();
        tracing::debug!(runtime = %rt, container = %self.reference, "container attach");
        Ok(())
    }

    async fn exec(&mut self, cmd: &str) -> Result<ExecOutput, TransportError> {
        let mut c = self.exec_cmd(false, false)?;
        c.arg(&self.reference).args(["sh", "-c", cmd]);
        c.stdin(Stdio::null());
        let out = c.output().await?;
        check_sh_present(cmd, &out)?;
        Ok(ExecOutput {
            exit_code: out.status.code().unwrap_or(-1),
            stdout: out.stdout,
            stderr: out.stderr,
        })
    }

    async fn upload_stream(
        &mut self,
        remote_cmd: &str,
        data: Vec<u8>,
    ) -> Result<ExecOutput, TransportError> {
        let mut c = self.exec_cmd(true, false)?;
        c.arg(&self.reference)
            .args(["sh", "-c", remote_cmd])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&data).await?;
            stdin.shutdown().await?;
        }
        let out = child.wait_with_output().await?;
        check_sh_present(remote_cmd, &out)?;
        Ok(ExecOutput {
            exit_code: out.status.code().unwrap_or(-1),
            stdout: out.stdout,
            stderr: out.stderr,
        })
    }

    async fn open_pty(&mut self, spec: &PtySpec) -> Result<i32, TransportError> {
        // Local PTY pair; `exec -it` sits on the slave, we proxy stdio through the
        // master (R5/C-C8). The CLI translates resizes of its tty into the exec
        // session, so SIGWINCH handling is: resize master + signal the child (C-C9).
        let (cols, rows) = local_tty_size().unwrap_or((spec.cols, spec.rows));
        let pty = Pty::open(cols, rows)?;

        let mut c = self.exec_cmd(true, true)?;
        c.args(["-e", &format!("TERM={}", spec.term)]);
        for (k, v) in &spec.env {
            c.args(["-e", &format!("{k}={v}")]);
        }
        c.arg(&self.reference)
            .args(["sh", "-c", &format!("exec {}", spec.shell_cmd)]);
        c.stdin(pty.slave_stdio()?)
            .stdout(pty.slave_stdio()?)
            .stderr(pty.slave_stdio()?);
        // The CLI must see the slave as its controlling terminal for -t to work.
        // SAFETY: setsid + TIOCSCTTY on the freshly-dup'ed stdin, pre-exec only.
        #[allow(unsafe_code)]
        unsafe {
            c.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = c.spawn()?;
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

        // The exec's exit code is the session's exit code (C-C10); container death
        // ends the child, EOFs the master and lands here — no hang (C-C11).
        let status = child.wait().await?;
        let _ = out_task.await; // drain remaining output before restoring the tty
        winch_task.abort();
        in_task.abort();
        Ok(status.code().unwrap_or(-1))
    }

    async fn disconnect(&mut self) -> Result<(), TransportError> {
        // exec-model: no persistent connection to tear down.
        self.runtime = None;
        Ok(())
    }
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
        let ws: libc::winsize = winsize(cols, rows);
        // SAFETY: openpty fills two fresh fds we immediately take ownership of;
        // `winp` is read-only (const winsize).
        #[allow(unsafe_code)]
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &ws,
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

/// Locate an executable on PATH without extra dependencies.
fn which(bin: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let cand = dir.join(bin);
        if cand.is_file() { Some(cand) } else { None }
    })
}
