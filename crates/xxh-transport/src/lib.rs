//! xxh-transport — transport abstraction (Принцип III, конституция v1.5.0).
//!
//! The rest of the tool works only through [`Transport`] and never learns which
//! backend — or transport *family* — is in use. Two families implement it:
//! SSH ([`RusshTransport`], [`SshCliTransport`]) and container runtimes
//! ([`ContainerCliTransport`] over docker/podman exec).
//! See contracts/transport-trait.md and 002-container-targets/contracts/.

use std::collections::BTreeMap;

mod container_backend;
mod russh_backend;
mod ssh_cli_backend;
mod tty;
pub use container_backend::{ContainerCliTransport, resolve_runtime};
pub use russh_backend::RusshTransport;
pub use ssh_cli_backend::SshCliTransport;

/// Error class for transport problems. Maps to CLI exit code 10 (§FR-026).
///
/// Variants are deliberately distinct so the user can tell a connection failure
/// from an auth failure from a host-key mismatch (Принцип VII).
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// Could not establish the connection (host unreachable, refused, DNS, ...).
    #[error("connection failed: {0}")]
    Connect(String),
    /// Authentication was rejected.
    #[error("authentication failed: {0}")]
    Auth(String),
    /// Host key did not match `known_hosts` (§FR-029; U3 — reject, never auto-accept).
    #[error("host key verification failed: {0}")]
    HostKey(String),
    /// A channel/exec/PTY operation failed after connecting.
    #[error("channel error: {0}")]
    Channel(String),
    /// The connect timeout elapsed (§FR-031, default ~10s).
    #[error("timed out after {0}s")]
    Timeout(u64),
    /// Underlying I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// The selected backend is unavailable (e.g. system `ssh` not found). No silent
    /// fallback (U2): the caller gets a clear transport error.
    #[error("transport backend unavailable: {0}")]
    BackendUnavailable(String),
}

/// Which SSH-config-compatible target to connect to.
#[derive(Debug, Clone)]
pub struct ResolvedSshTarget {
    /// Host alias / hostname as given on the command line (§FR-001).
    pub alias: String,
    /// Optional explicit user (otherwise taken from ssh-config).
    pub user: Option<String>,
    /// Optional explicit port.
    pub port: Option<u16>,
    /// Optional explicit private key (like ssh `-i`); when set it is used
    /// **exclusively** for public-key auth (IdentitiesOnly semantics).
    pub identity: Option<std::path::PathBuf>,
    /// Connect timeout in seconds (§FR-031).
    pub connect_timeout_s: u64,
}

impl ResolvedSshTarget {
    /// Construct a target from a host alias, using the default connect timeout.
    pub fn new(alias: impl Into<String>) -> Self {
        Self {
            alias: alias.into(),
            user: None,
            port: None,
            identity: None,
            connect_timeout_s: 10,
        }
    }
}

/// A container runtime the container family can drive (002 data-model).
/// MVP: docker and podman — an identical CLI surface behind one backend;
/// further runtimes (nerdctl/containerd, kubectl) are future feature-gated
/// extensions of the same enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerRuntime {
    Docker,
    Podman,
}

impl ContainerRuntime {
    /// Name of the CLI binary that drives this runtime.
    pub fn binary_name(&self) -> &'static str {
        match self {
            ContainerRuntime::Docker => "docker",
            ContainerRuntime::Podman => "podman",
        }
    }

    /// Deterministic auto-selection order (contracts/target-addressing.md C-A3).
    pub const AUTO_ORDER: [ContainerRuntime; 2] =
        [ContainerRuntime::Docker, ContainerRuntime::Podman];
}

impl std::fmt::Display for ContainerRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.binary_name())
    }
}

/// How the runtime for a container target is chosen (C-A3): an explicit choice
/// (scheme/flag/config) wins; `Auto` probes [`ContainerRuntime::AUTO_ORDER`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeSelector {
    Auto,
    Explicit(ContainerRuntime),
}

/// A running local container as a session target (002 data-model).
#[derive(Debug, Clone)]
pub struct ContainerTarget {
    /// Container name or id, exactly as the user wrote it.
    pub reference: String,
    pub runtime: RuntimeSelector,
    /// Exec-session user (runtime `-u`); sourced from the shared `user` key
    /// (C-A5). `None` keeps the container's configured user.
    pub exec_user: Option<String>,
    pub connect_timeout_s: u64,
}

impl ContainerTarget {
    pub fn new(reference: impl Into<String>) -> Self {
        Self {
            reference: reference.into(),
            runtime: RuntimeSelector::Auto,
            exec_user: None,
            connect_timeout_s: 10,
        }
    }
}

/// The one target type [`Transport::connect`] accepts: either transport family,
/// distinguishable but uniform (Принцип III). A backend given a target of the
/// other family answers [`TransportError::BackendUnavailable`] immediately —
/// no silent fallback (C-T6).
#[derive(Debug, Clone)]
pub enum ResolvedTarget {
    Ssh(ResolvedSshTarget),
    Container(ContainerTarget),
}

impl From<ResolvedSshTarget> for ResolvedTarget {
    fn from(t: ResolvedSshTarget) -> Self {
        ResolvedTarget::Ssh(t)
    }
}

impl From<ContainerTarget> for ResolvedTarget {
    fn from(t: ContainerTarget) -> Self {
        ResolvedTarget::Container(t)
    }
}

impl ResolvedTarget {
    /// Short human label for progress lines (§FR-025) — the SSH alias or the
    /// container reference. Never includes secrets or socket paths (C-T9).
    pub fn label(&self) -> &str {
        match self {
            ResolvedTarget::Ssh(t) => &t.alias,
            ResolvedTarget::Container(t) => &t.reference,
        }
    }

    /// The connect timeout for either family (§FR-031).
    pub fn connect_timeout_s(&self) -> u64 {
        match self {
            ResolvedTarget::Ssh(t) => t.connect_timeout_s,
            ResolvedTarget::Container(t) => t.connect_timeout_s,
        }
    }
}

/// Which authentication methods the transport may use (§FR-029a).
#[derive(Debug, Clone)]
pub struct AuthPolicy {
    pub allow_agent: bool,
    pub allow_pubkey: bool,
    pub allow_interactive: bool,
}

impl Default for AuthPolicy {
    fn default() -> Self {
        Self {
            allow_agent: true,
            allow_pubkey: true,
            allow_interactive: true,
        }
    }
}

/// Result of a one-shot remote command.
#[derive(Debug, Clone)]
pub struct ExecOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ExecOutput {
    /// Stdout decoded lossily as UTF-8 and trimmed — handy for `uname`, cache listings.
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim().to_string()
    }
}

/// Parameters for an interactive PTY session.
#[derive(Debug, Clone)]
pub struct PtySpec {
    pub term: String,
    pub cols: u16,
    pub rows: u16,
    /// Command line to run as the login shell on the host.
    pub shell_cmd: String,
    /// Extra environment to export in the remote shell init.
    pub env: BTreeMap<String, String>,
}

/// Stable internal transport interface (Принцип III).
///
/// # Contract obligations (contracts/transport-trait.md + v2)
/// - C-T1/C-T9: never log secrets, socket paths or exec env values.
/// - C-T2: all data flows only over the established connection.
/// - C-T3: `connect` honours the timeout and never hangs.
/// - C-T6: a target of the wrong family fails fast with `BackendUnavailable`.
/// - C-T8: `AuthPolicy` is SSH-family only; container backends ignore it.
#[async_trait::async_trait]
pub trait Transport: Send {
    /// Establish the connection / attach to the target. SSH family honours
    /// ssh-config, known_hosts, ssh-agent and ProxyJump; the container family
    /// authenticates purely via the user's access to the runtime socket/API.
    async fn connect(
        &mut self,
        target: &ResolvedTarget,
        auth: &AuthPolicy,
    ) -> Result<(), TransportError>;

    /// Run a one-shot command and capture its output.
    async fn exec(&mut self, cmd: &str) -> Result<ExecOutput, TransportError>;

    /// Stream `data` to `remote_cmd`'s stdin (e.g. a tar stream into bootstrap).
    async fn upload_stream(
        &mut self,
        remote_cmd: &str,
        data: Vec<u8>,
    ) -> Result<ExecOutput, TransportError>;

    /// Open an interactive PTY running the shell; returns the shell's exit status.
    async fn open_pty(&mut self, spec: &PtySpec) -> Result<i32, TransportError>;

    /// Close the connection.
    async fn disconnect(&mut self) -> Result<(), TransportError>;
}

#[cfg(test)]
mod tests {
    //! Family-guard tests (002 T007, C-T6): a backend handed a target of the
    //! wrong family fails fast with `BackendUnavailable` and performs no network
    //! or process action — no daemon, no ssh binary, no docker needed.

    use super::*;

    fn ssh() -> ResolvedTarget {
        ResolvedTarget::Ssh(ResolvedSshTarget::new("example"))
    }
    fn container() -> ResolvedTarget {
        ResolvedTarget::Container(ContainerTarget::new("app1"))
    }

    #[tokio::test]
    async fn russh_backend_rejects_container_target() {
        let mut t = RusshTransport::new();
        let err = t.connect(&container(), &AuthPolicy::default()).await;
        assert!(
            matches!(err, Err(TransportError::BackendUnavailable(_))),
            "russh must reject a container target with BackendUnavailable, got {err:?}"
        );
    }

    #[tokio::test]
    async fn container_backend_rejects_ssh_target() {
        let mut t = ContainerCliTransport::new();
        let err = t.connect(&ssh(), &AuthPolicy::default()).await;
        assert!(
            matches!(err, Err(TransportError::BackendUnavailable(_))),
            "container backend must reject an SSH target with BackendUnavailable, got {err:?}"
        );
    }

    #[test]
    fn labels_and_timeouts_are_family_agnostic() {
        assert_eq!(ssh().label(), "example");
        assert_eq!(container().label(), "app1");
        assert_eq!(ssh().connect_timeout_s(), 10);
        assert_eq!(container().connect_timeout_s(), 10);
    }
}
