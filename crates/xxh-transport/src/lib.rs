//! xxh-transport — SSH transport abstraction (Принцип III).
//!
//! The rest of the tool works only through [`Transport`] and never learns which
//! backend is in use. Two backends implement it: [`SshCliTransport`] (wrapper over
//! the system `ssh`, T009) and the russh-based backend (T010).
//! See contracts/transport-trait.md.

use std::collections::BTreeMap;

mod russh_backend;
mod ssh_cli_backend;
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
            connect_timeout_s: 10,
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
/// # Contract obligations (contracts/transport-trait.md)
/// - C-T1: never log secrets (enforced by callers + backends).
/// - C-T2: all data flows only over the established connection.
/// - C-T3: `connect` honours the timeout and never hangs.
#[async_trait::async_trait]
pub trait Transport: Send {
    /// Establish and authenticate the connection. Honours ssh-config, known_hosts,
    /// ssh-agent and ProxyJump; supports key and interactive auth.
    async fn connect(
        &mut self,
        target: &ResolvedSshTarget,
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
