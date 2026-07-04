//! Session orchestration (T016–T018): connect → detect → deploy → run → cleanup.
//!
//! Zero-footprint by construction (Принцип I): platform detection streams the
//! bootstrap script over stdin and creates nothing on the host; only after a
//! supported platform is confirmed is `~/.xxh` created. Cleanup is guaranteed by the
//! remote `trap` (bootstrap.sh) plus a reconcile sweep on the next connect.

use std::collections::BTreeSet;

use xxh_config::{CleanupMode, Effective};
use xxh_transport::{AuthPolicy, ResolvedSshTarget, Transport};

use crate::ShellError;
use crate::deploy::{Component, ComponentKind};
use crate::platform::Platform;

/// The embedded reference bootstrap script (Принцип I; contracts/bootstrap-protocol.md).
const BOOTSTRAP_SH: &str = include_str!("../../../bootstrap/bootstrap.sh");

/// Remote path for the bootstrap script, inside the ephemeral root so cleanup removes it.
const REMOTE_BOOT: &str = "$HOME/.xxh/boot.sh";

/// Errors distinguishable by class for the CLI (§FR-026).
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Transport(#[from] xxh_transport::TransportError),
    #[error(transparent)]
    Shell(#[from] ShellError),
}

/// A connected, prepared session ready to launch a shell.
pub struct Session<T: Transport> {
    transport: T,
    platform: Platform,
    keep: bool,
    session_id: String,
    /// Remote path of the assembled env script to source before the shell starts.
    env_source: Option<String>,
}

impl<T: Transport> Session<T> {
    /// Establish a session: connect, detect platform (leaving the host untouched on
    /// failure), reconcile stale artefacts, and deliver the environment.
    pub async fn establish(
        mut transport: T,
        target: &ResolvedSshTarget,
        eff: &Effective,
        env_components: &[Component],
    ) -> Result<Self, SessionError> {
        transport.connect(target, &AuthPolicy::default()).await?;

        // 1) Detect platform by streaming the script over stdin — creates nothing on
        //    the host, so an unsupported platform leaves it clean (C-B1, §FR-007).
        let detect = transport
            .upload_stream("sh -s -- detect", BOOTSTRAP_SH.as_bytes().to_vec())
            .await?;
        if detect.exit_code != 0 {
            return Err(ShellError::Other(format!(
                "platform detection failed: {}",
                String::from_utf8_lossy(&detect.stderr)
            ))
            .into());
        }
        let platform = Platform::parse_detect(&detect.stdout_str())?;

        // 2) Install the bootstrap script into the ephemeral root and sweep any stale
        //    artefacts from a previously crashed session (§FR-006).
        transport
            .upload_stream(
                &format!("mkdir -p $HOME/.xxh && cat > {REMOTE_BOOT} && chmod +x {REMOTE_BOOT}"),
                BOOTSTRAP_SH.as_bytes().to_vec(),
            )
            .await?;
        transport
            .exec(&format!("sh {REMOTE_BOOT} reconcile"))
            .await?;

        // 3) Deliver only components missing from the host cache (§FR-013, VI).
        let host_hashes = list_cache(&mut transport).await?;
        let mut env_paths = Vec::new();
        for comp in super::deploy::missing(env_components, &host_hashes) {
            transport
                .upload_stream(
                    &format!("sh {REMOTE_BOOT} recv {} {}", comp.hash, comp.fmt),
                    comp.payload.clone(),
                )
                .await?;
        }
        // Assemble: every Config component contributes an `env.sh` if present.
        for comp in env_components {
            if comp.kind == ComponentKind::Config {
                env_paths.push(format!("$HOME/.xxh/cache/{}/env.sh", comp.hash));
            }
        }
        let env_source = if env_paths.is_empty() {
            None
        } else {
            let mut s = String::new();
            for p in &env_paths {
                s.push_str(&format!(". {p} 2>/dev/null; "));
            }
            Some(s)
        };

        Ok(Self {
            transport,
            platform,
            keep: matches!(eff.cleanup, CleanupMode::Keep),
            session_id: session_id(),
            env_source,
        })
    }

    /// Detected host platform.
    pub fn platform(&self) -> &Platform {
        &self.platform
    }

    /// Build the remote command that sources the env then execs `shell_cmd`.
    fn shell_invocation(&self, shell_cmd: &str) -> String {
        let keep = if self.keep { "1" } else { "0" };
        let prelude = self.env_source.clone().unwrap_or_default();
        // bootstrap `run` installs the cleanup trap, then execs the given argv.
        format!(
            "sh {REMOTE_BOOT} run {} {keep} sh -c '{}exec {}'",
            self.session_id,
            prelude.replace('\'', "'\\''"),
            shell_cmd.replace('\'', "'\\''")
        )
    }

    /// Launch an interactive shell over a PTY (real use).
    pub async fn run_interactive(&mut self, shell_cmd: &str) -> Result<i32, SessionError> {
        let cmd = self.shell_invocation(shell_cmd);
        let spec = xxh_transport::PtySpec {
            term: std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".into()),
            cols: 80,
            rows: 24,
            shell_cmd: cmd,
            env: Default::default(),
        };
        Ok(self.transport.open_pty(&spec).await?)
    }

    /// Run a non-interactive command inside the prepared environment (tests / scripts).
    pub async fn run_command(&mut self, shell_cmd: &str) -> Result<i32, SessionError> {
        let cmd = self.shell_invocation(shell_cmd);
        let out = self.transport.exec(&cmd).await?;
        Ok(out.exit_code)
    }

    /// Close the transport. Host cleanup is guaranteed by the remote trap during
    /// `run`; this also disconnects cleanly.
    pub async fn finish(mut self) -> Result<(), SessionError> {
        self.transport.disconnect().await?;
        Ok(())
    }
}

async fn list_cache<T: Transport>(t: &mut T) -> Result<BTreeSet<String>, SessionError> {
    let out = t.exec(&format!("sh {REMOTE_BOOT} list-cache")).await?;
    Ok(out
        .stdout_str()
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

fn session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("s{n:x}")
}

/// Build a minimal environment component that marks the xxh session and adds a demo
/// alias, proving config delivery end-to-end. Real dotfiles/plugins/shell packages
/// extend this set (T020, US4).
pub fn minimal_env_component(fmt: &str) -> Result<Component, ShellError> {
    let dir = std::env::temp_dir().join(format!("xxh-env-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| ShellError::Other(e.to_string()))?;
    std::fs::write(
        dir.join("env.sh"),
        b"export XXH_SESSION=1\nalias xxh-hello='echo hello-from-xxh'\n",
    )
    .map_err(|e| ShellError::Other(e.to_string()))?;
    let comp = Component::pack_dir(ComponentKind::Config, &dir, fmt);
    let _ = std::fs::remove_dir_all(&dir);
    comp
}
