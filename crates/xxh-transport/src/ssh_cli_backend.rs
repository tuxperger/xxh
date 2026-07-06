//! System-`ssh` transport backend (T009) — fallback/compat + debugging (research R2).
//!
//! Uses SSH connection multiplexing (ControlMaster) so `connect` authenticates once
//! and later `exec`/`open_pty` reuse the master without re-prompting. Interactive auth
//! (password/keyboard-interactive) works because stdio is inherited (§FR-029a).

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::{AuthPolicy, ExecOutput, PtySpec, ResolvedSshTarget, Transport, TransportError};

/// Transport backed by the host's `ssh` binary.
pub struct SshCliTransport {
    ssh_bin: String,
    alias: String,
    /// Multiplexing control socket dir; `Some` once connected.
    ctl_dir: Option<std::path::PathBuf>,
    base_args: Vec<String>,
}

impl SshCliTransport {
    /// Create an unconnected backend. Returns `BackendUnavailable` if no `ssh` on PATH
    /// (U2: no silent fallback, a clear transport error instead).
    pub fn new() -> Result<Self, TransportError> {
        let ssh_bin = "ssh".to_string();
        // Cheap availability probe; the real check happens in `connect`.
        if which(&ssh_bin).is_none() {
            return Err(TransportError::BackendUnavailable(
                "`ssh` not found on PATH".into(),
            ));
        }
        Ok(Self {
            ssh_bin,
            alias: String::new(),
            ctl_dir: None,
            base_args: Vec::new(),
        })
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new(&self.ssh_bin);
        c.args(&self.base_args);
        c
    }
}

#[async_trait::async_trait]
impl Transport for SshCliTransport {
    async fn connect(
        &mut self,
        target: &ResolvedSshTarget,
        _auth: &AuthPolicy,
    ) -> Result<(), TransportError> {
        self.alias = target.alias.clone();

        let ctl_dir = std::env::temp_dir().join(format!("xxh-ctl-{}", std::process::id()));
        std::fs::create_dir_all(&ctl_dir)?;
        let ctl_path = ctl_dir.join("cm");

        // Respect ssh-config, known_hosts, agent, ProxyJump implicitly via the system
        // client. Add multiplexing + a bounded connect timeout (§FR-031).
        let mut base = vec![
            "-o".into(),
            "ControlMaster=auto".into(),
            "-o".into(),
            format!("ControlPath={}", ctl_path.display()),
            "-o".into(),
            "ControlPersist=60".into(),
            "-o".into(),
            format!("ConnectTimeout={}", target.connect_timeout_s),
        ];
        if let Some(u) = &target.user {
            base.push("-o".into());
            base.push(format!("User={u}"));
        }
        if let Some(p) = target.port {
            base.push("-p".into());
            base.push(p.to_string());
        }
        self.base_args = base;
        self.ctl_dir = Some(ctl_dir);

        // Establish the master by running a trivial command; stdio inherited so
        // interactive auth prompts reach the user.
        let mut c = self.cmd();
        c.arg(&self.alias).arg("--").arg("true");
        let fut = c.status();
        let status = tokio::time::timeout(Duration::from_secs(target.connect_timeout_s + 2), fut)
            .await
            .map_err(|_| TransportError::Timeout(target.connect_timeout_s))??;

        if !status.success() {
            return Err(TransportError::Connect(format!(
                "ssh to `{}` exited with {}",
                self.alias, status
            )));
        }
        Ok(())
    }

    async fn exec(&mut self, cmd: &str) -> Result<ExecOutput, TransportError> {
        let mut c = self.cmd();
        c.arg(&self.alias).arg("--").arg(cmd);
        c.stdin(Stdio::null());
        let out = c.output().await?;
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
        let mut c = self.cmd();
        c.arg(&self.alias)
            .arg("--")
            .arg(remote_cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&data).await?;
            stdin.shutdown().await?;
        }
        let out = child.wait_with_output().await?;
        Ok(ExecOutput {
            exit_code: out.status.code().unwrap_or(-1),
            stdout: out.stdout,
            stderr: out.stderr,
        })
    }

    async fn open_pty(&mut self, spec: &PtySpec) -> Result<i32, TransportError> {
        // Prefix env exports so the remote shell init sees them (used by ⭐ nix
        // runtime-data vars later). `-tt` forces a PTY; stdio inherited for interaction.
        let mut prefix = String::new();
        for (k, v) in &spec.env {
            prefix.push_str(&format!("export {k}={}; ", shell_quote(v)));
        }
        let full = format!("{prefix}exec {}", spec.shell_cmd);

        let mut c = self.cmd();
        c.arg("-tt")
            .arg("-o")
            .arg(format!("SetEnv=TERM={}", spec.term))
            .arg(&self.alias)
            .arg("--")
            .arg(full);
        let status = c.status().await?;
        Ok(status.code().unwrap_or(-1))
    }

    async fn disconnect(&mut self) -> Result<(), TransportError> {
        if !self.alias.is_empty() {
            // Best-effort close of the master.
            let mut c = self.cmd();
            let _ = c.arg("-O").arg("exit").arg(&self.alias).status().await;
        }
        if let Some(dir) = self.ctl_dir.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
        Ok(())
    }
}

/// Minimal single-quote shell escaping for env values.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Locate an executable on PATH without extra dependencies.
fn which(bin: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let cand = dir.join(bin);
        if cand.is_file() { Some(cand) } else { None }
    })
}
