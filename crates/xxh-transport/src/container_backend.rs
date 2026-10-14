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
pub(crate) async fn run_bounded(
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
pub(crate) fn check_sh_present(
    cmd: &str,
    out: &std::process::Output,
) -> Result<(), TransportError> {
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

    async fn exec_stream(&mut self, cmd: &str) -> Result<i32, TransportError> {
        // `-i` without `-t`: stdin is forwarded, no TTY is allocated, the streams
        // stay separate; stdio is inherited from the client (C-X1/C-X2).
        let mut c = self.exec_cmd(true, false)?;
        c.arg(&self.reference).args(["sh", "-c", cmd]);
        c.kill_on_drop(true);
        let status = c.status().await?;
        Ok(crate::exit_code_of(&status))
    }

    async fn open_pty(&mut self, spec: &PtySpec) -> Result<i32, TransportError> {
        let mut c = self.exec_cmd(true, true)?;
        c.args(["-e", &format!("TERM={}", spec.term)]);
        for (k, v) in &spec.env {
            c.args(["-e", &format!("{k}={v}")]);
        }
        c.arg(&self.reference)
            .args(["sh", "-c", &format!("exec {}", spec.shell_cmd)]);
        let (code, _) = crate::pty::run_in_local_pty(
            c,
            spec.cols,
            spec.rows,
            None::<fn(tokio::process::ChildStderr)>,
        )
        .await?;
        Ok(code)
    }

    async fn disconnect(&mut self) -> Result<(), TransportError> {
        // exec-model: no persistent connection to tear down.
        self.runtime = None;
        Ok(())
    }
}

/// Locate an executable on PATH without extra dependencies.
pub(crate) fn which(bin: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let cand = dir.join(bin);
        if cand.is_file() { Some(cand) } else { None }
    })
}

/// Whether a runtime endpoint variable (`DOCKER_HOST`, `CONTAINER_HOST`) keeps
/// the runtime CLI on this machine: unset, empty or a unix socket (007 C-K9).
fn is_local_endpoint(value: Option<&str>) -> bool {
    value.is_none_or(|v| v.is_empty() || v.starts_with("unix://"))
}

/// The names of the running containers of `rt`, for completion (007 T007,
/// C-K8/C-K9). Read-only (`ps`), bounded by `budget`, and silent: a missing or
/// hung runtime, an error, or a runtime pointed at another machine all give an
/// empty list — completion must neither wait nor reach the network.
pub fn running_containers(rt: ContainerRuntime, budget: Duration) -> Vec<String> {
    let endpoint = match rt {
        ContainerRuntime::Docker => "DOCKER_HOST",
        ContainerRuntime::Podman => "CONTAINER_HOST",
    };
    if !is_local_endpoint(std::env::var(endpoint).ok().as_deref()) {
        return Vec::new();
    }
    match which(rt.binary_name()) {
        Some(bin) => list_running(&bin, budget),
        None => Vec::new(),
    }
}

/// [`running_containers`] of the first installed runtime in
/// [`ContainerRuntime::AUTO_ORDER`] — what a `container:` target would use.
pub fn running_containers_auto(budget: Duration) -> Vec<String> {
    ContainerRuntime::AUTO_ORDER
        .into_iter()
        .find(|rt| which(rt.binary_name()).is_some())
        .map(|rt| running_containers(rt, budget))
        .unwrap_or_default()
}

fn list_running(bin: &std::path::Path, budget: Duration) -> Vec<String> {
    use std::io::Read as _;

    let Ok(mut child) = std::process::Command::new(bin)
        .args(["ps", "--format", "{{.Names}}"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let Some(mut stdout) = child.stdout.take() else {
        return Vec::new();
    };
    // The reader runs aside so the wait below is bounded whatever the runtime does.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        let _ = tx.send(text);
    });
    let text = rx.recv_timeout(budget);
    if text.is_err() {
        let _ = child.kill();
    }
    let ok = child.wait().is_ok_and(|s| s.success());
    match text {
        Ok(text) if ok => {
            let mut names: Vec<String> = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect();
            names.sort();
            names
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Write an executable script through a child `sh`. This process never holds a
    /// write descriptor to it, so a concurrent `fork` in another test cannot make
    /// the kernel refuse to run it ("text file busy").
    pub(crate) fn write_script(path: &std::path::Path, body: &str) {
        use std::io::Write as _;
        let mut child = std::process::Command::new("sh")
            .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
            .arg(path)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("sh");
        let mut stdin = child.stdin.take().expect("stdin");
        stdin
            .write_all(format!("#!/bin/sh\n{body}\n").as_bytes())
            .expect("write script");
        drop(stdin);
        assert!(child.wait().expect("sh").success());
    }

    /// A stand-in runtime CLI: a script in its own temp dir.
    fn fake_runtime(tag: &str, body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xxh-fake-rt-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("runtime");
        write_script(&bin, body);
        bin
    }

    fn remove(bin: &std::path::Path) {
        std::fs::remove_dir_all(bin.parent().unwrap()).unwrap();
    }

    /// Names come back sorted; a failing runtime offers nothing (007 T007).
    #[test]
    fn running_containers_are_listed() {
        let bin = fake_runtime(
            "ok",
            "[ \"$1 $2 $3\" = 'ps --format {{.Names}}' ] || exit 2\necho web\necho app",
        );
        assert_eq!(list_running(&bin, Duration::from_secs(5)), ["app", "web"]);
        remove(&bin);

        let bin = fake_runtime("fail", "echo half; echo 'daemon down' >&2; exit 1");
        assert!(list_running(&bin, Duration::from_secs(5)).is_empty());
        remove(&bin);

        assert!(
            list_running(
                std::path::Path::new("/nonexistent/rt"),
                Duration::from_secs(1)
            )
            .is_empty()
        );
    }

    /// A hung runtime costs the budget, not its own time (007 C-K9, SC-003).
    #[test]
    fn hung_runtime_is_cut_off() {
        let bin = fake_runtime("hung", "exec sleep 30");
        let started = std::time::Instant::now();
        assert!(list_running(&bin, Duration::from_millis(100)).is_empty());
        assert!(started.elapsed() < Duration::from_secs(5));
        remove(&bin);
    }

    #[test]
    fn only_local_endpoints_are_asked() {
        assert!(is_local_endpoint(None));
        assert!(is_local_endpoint(Some("")));
        assert!(is_local_endpoint(Some("unix:///run/user/1000/docker.sock")));
        assert!(!is_local_endpoint(Some("tcp://10.0.0.5:2376")));
        assert!(!is_local_endpoint(Some("ssh://me@build")));
    }
}
