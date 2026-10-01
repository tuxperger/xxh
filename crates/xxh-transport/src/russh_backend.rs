//! russh transport backend (T010) — the primary, pure-Rust SSH client (research R1/R7).
//!
//! Covers: ssh-config resolution (russh-config plus `ssh_config_extra`),
//! known_hosts verification that **rejects on mismatch** (analysis U3) on every
//! hop, public-key auth with ssh-agent keys and identity files in OpenSSH order,
//! interactive (password / keyboard-interactive) fallback (§FR-029a), `ProxyJump`
//! chains over `direct-tcpip`, opt-in agent forwarding (012
//! contracts/russh-auth-and-jump.md), plus exec, upload_stream and an interactive
//! PTY.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use russh::client::{
    self, AuthResult, ChannelOpenHandle, Handle, Handler, KeyboardInteractiveAuthResponse,
};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::AgentClient;
use russh::keys::{PrivateKeyWithHashAlg, PublicKey, load_secret_key};
use russh::{ChannelMsg, ChannelOpenFailure, Disconnect};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::ssh_config_extra::{self, AgentSetting, HostExtras};
use crate::tty::{RawModeGuard, local_tty_size};
use crate::{AuthPolicy, ExecOutput, PtySpec, ResolvedTarget, Transport, TransportError};

fn te(msg: impl std::fmt::Display) -> TransportError {
    TransportError::Channel(msg.to_string())
}

/// The longest `ProxyJump` chain followed (012 C-J4).
const MAX_HOPS: usize = 8;

/// russh client handler. Verifies the server host key against `known_hosts` and,
/// when the user allowed it, serves agent-forwarding channels from the local agent.
struct ClientHandler {
    host: String,
    port: u16,
    /// The local agent socket forwarded channels are joined to; `None` refuses
    /// every agent channel the server opens (012 C-J10).
    agent_forward: Option<PathBuf>,
}

impl Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, server_key: &PublicKey) -> Result<bool, Self::Error> {
        // TOFU with mismatch rejection: if the host is already pinned in known_hosts
        // and the key differs, refuse (U3). Unknown hosts are accepted (as ssh's
        // StrictHostKeyChecking=accept-new) and pinned.
        Ok(known_hosts_check(&self.host, self.port, server_key))
    }

    async fn server_channel_open_agent_forward(
        &mut self,
        channel: russh::Channel<client::Msg>,
        reply: ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let Some(sock) = self.agent_forward.clone() else {
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        reply.accept().await;
        // Join the channel to the local agent; nothing of the exchange is logged
        // (C-J12).
        tokio::spawn(async move {
            if let Ok(mut agent) = tokio::net::UnixStream::connect(&sock).await {
                let mut ch = channel.into_stream();
                let _ = tokio::io::copy_bidirectional(&mut ch, &mut agent).await;
            }
        });
        Ok(())
    }
}

/// Transport backed by the russh library.
pub struct RusshTransport {
    handle: Option<Handle<ClientHandler>>,
    /// Connections to the `ProxyJump` hops, kept open while the target is used.
    jumps: Vec<Handle<ClientHandler>>,
    /// Request agent forwarding on the user's shell/command channels.
    forward_agent: bool,
}

impl RusshTransport {
    pub fn new() -> Self {
        Self {
            handle: None,
            jumps: Vec::new(),
            forward_agent: false,
        }
    }

    fn handle_mut(&mut self) -> Result<&mut Handle<ClientHandler>, TransportError> {
        self.handle
            .as_mut()
            .ok_or_else(|| TransportError::Channel("not connected".into()))
    }

    /// Ask for the agent on a channel of the user's shell or command (C-J10).
    async fn maybe_forward(&self, ch: &russh::Channel<client::Msg>) -> Result<(), TransportError> {
        if self.forward_agent {
            ch.agent_forward(false).await.map_err(te)?;
        }
        Ok(())
    }
}

impl Default for RusshTransport {
    fn default() -> Self {
        Self::new()
    }
}

/// One SSH connection on the way to the target — the target itself or a
/// `ProxyJump` hop — with its own resolved ssh_config (012 research R2).
#[derive(Debug, Clone)]
struct Hop {
    /// The name as the user or the config wrote it.
    label: String,
    host: String,
    port: u16,
    user: String,
    identity_files: Vec<PathBuf>,
    /// The identity files came from the config for this host (not the defaults).
    configured_ids: bool,
    extras: HostExtras,
    proxy_jump: Option<String>,
    proxy_command: Option<String>,
}

fn set(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.trim().is_empty() && !s.trim().eq_ignore_ascii_case("none"))
}

/// Resolve `alias` through `~/.ssh/config`; explicit user/port win.
fn resolve_hop(alias: &str, user: Option<String>, port: Option<u16>) -> Hop {
    // NB: use the getters — `host()`/`port()`/`user()` merge the per-host config
    // over defaults; the raw `host_name` field is just the alias.
    let cfg = russh_config::parse_home(alias).ok();
    let configured = cfg
        .as_ref()
        .and_then(|c| c.host_config.identity_file.clone());
    Hop {
        label: alias.to_string(),
        host: cfg
            .as_ref()
            .map(|c| c.host().to_string())
            .unwrap_or_else(|| alias.to_string()),
        port: port
            .or_else(|| cfg.as_ref().map(|c| c.port()))
            .unwrap_or(22),
        user: user
            .or_else(|| cfg.as_ref().map(|c| c.user()))
            .unwrap_or_else(whoami),
        configured_ids: configured.is_some(),
        identity_files: configured.unwrap_or_else(default_identity_files),
        extras: ssh_config_extra::for_host(alias),
        proxy_jump: set(cfg.as_ref().and_then(|c| c.host_config.proxy_jump.clone())),
        proxy_command: set(cfg.and_then(|c| c.host_config.proxy_command)),
    }
}

/// One `ProxyJump` entry: `[user@]host[:port]`, `[v6]:port` allowed.
fn parse_jump(entry: &str) -> (Option<String>, String, Option<u16>) {
    let entry = entry.trim().trim_start_matches("ssh://");
    let (user, rest) = match entry.rsplit_once('@') {
        Some((u, r)) => (Some(u.to_string()), r),
        None => (None, entry),
    };
    if let Some(v6) = rest.strip_prefix('[') {
        if let Some((host, tail)) = v6.split_once(']') {
            let port = tail.strip_prefix(':').and_then(|p| p.parse().ok());
            return (user, host.to_string(), port);
        }
    }
    match rest.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => (user, h.to_string(), p.parse().ok()),
        _ => (user, rest.to_string(), None),
    }
}

/// How a hop name becomes a [`Hop`] — ssh_config in real use, a table in tests.
type Resolver = dyn Fn(&str, Option<String>, Option<u16>) -> Hop;

/// The hops to `alias`, outermost first, the target last (C-J4). Like OpenSSH,
/// the first jump host is reached through its own `ProxyJump`; later entries of
/// a list go through the previous entry instead.
fn jump_chain(
    alias: &str,
    user: Option<String>,
    port: Option<u16>,
    resolve: &Resolver,
) -> Result<Vec<Hop>, TransportError> {
    fn walk(
        hop: Hop,
        resolve: &Resolver,
        seen: &mut Vec<String>,
        out: &mut Vec<Hop>,
    ) -> Result<(), TransportError> {
        if let (Some(cmd), None) = (&hop.proxy_command, &hop.proxy_jump) {
            let _ = cmd; // never logged or run: it is the user's command line
            return Err(TransportError::BackendUnavailable(format!(
                "`{}` is reached through a ProxyCommand, which the built-in client does \
                 not run; use `--transport ssh`",
                hop.label
            )));
        }
        if let Some(pj) = hop.proxy_jump.clone() {
            for (i, entry) in pj.split(',').enumerate() {
                let (u, h, p) = parse_jump(entry);
                if seen.contains(&h) {
                    return Err(TransportError::Connect(format!(
                        "ProxyJump loops back to `{h}`"
                    )));
                }
                seen.push(h.clone());
                let next = resolve(&h, u, p);
                if i == 0 {
                    walk(next, resolve, seen, out)?;
                } else {
                    out.push(Hop {
                        proxy_jump: None,
                        proxy_command: None,
                        ..next
                    });
                }
                if out.len() >= MAX_HOPS {
                    return Err(TransportError::Connect(format!(
                        "ProxyJump chain to `{}` is longer than {MAX_HOPS} hops",
                        hop.label
                    )));
                }
            }
        }
        out.push(hop);
        Ok(())
    }
    let target = resolve(alias, user, port);
    let mut seen = vec![target.label.clone()];
    let mut out = Vec::new();
    walk(target, resolve, &mut seen, &mut out)?;
    Ok(out)
}

/// Name the hop in an error, keeping its class (C-J6).
fn via(label: &str, e: TransportError) -> TransportError {
    match e {
        TransportError::Auth(m) => TransportError::Auth(format!("via {label}: {m}")),
        TransportError::HostKey(m) => TransportError::HostKey(format!("via {label}: {m}")),
        TransportError::Timeout(s) => {
            TransportError::Connect(format!("via {label}: timed out after {s}s"))
        }
        TransportError::BackendUnavailable(m) => TransportError::BackendUnavailable(m),
        other => TransportError::Connect(format!("via {label}: {other}")),
    }
}

/// The agent's keys in the order they are offered (C-J2): those matching the
/// host's configured identity files first, then the rest unless `IdentitiesOnly`.
fn order_keys(agent: Vec<PublicKey>, configured: &[PublicKey], only: bool) -> Vec<PublicKey> {
    let is_configured = |k: &PublicKey| configured.iter().any(|c| c.key_data() == k.key_data());
    let (mut first, rest): (Vec<_>, Vec<_>) = agent.into_iter().partition(is_configured);
    if !only {
        first.extend(rest);
    }
    first
}

/// The agent for a host: `IdentityAgent` or `$SSH_AUTH_SOCK` (C-J3).
fn agent_socket(extras: &HostExtras) -> Option<PathBuf> {
    match &extras.identity_agent {
        Some(AgentSetting::Disabled) => None,
        Some(AgentSetting::Socket(p)) => Some(p.clone()),
        Some(AgentSetting::FromEnv) | None => std::env::var_os("SSH_AUTH_SOCK").map(PathBuf::from),
    }
}

fn read_public(path: &Path) -> Option<PublicKey> {
    let text = std::fs::read_to_string(format!("{}.pub", path.display())).ok()?;
    PublicKey::from_openssh(text.trim()).ok()
}

/// Authenticate on one hop (C-J1..C-J3). An explicit identity (ssh `-i`) is used
/// exclusively — a broken path is a clear auth error, not a silent fallback.
async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    hop: &Hop,
    explicit: Option<&PathBuf>,
    policy: &AuthPolicy,
) -> Result<(), TransportError> {
    let user = hop.user.as_str();
    if let Some(path) = explicit {
        if policy.allow_pubkey {
            let key = load_identity(path)?;
            let kwh = PrivateKeyWithHashAlg::new(Arc::new(key), None);
            if let Ok(AuthResult::Success) = handle.authenticate_publickey(user, kwh).await {
                return Ok(());
            }
        }
    } else {
        let mut offered: Vec<PublicKey> = Vec::new();
        let only = hop.extras.identities_only == Some(true);
        let configured: Vec<PublicKey> = if hop.configured_ids {
            hop.identity_files
                .iter()
                .filter_map(|f| read_public(f))
                .collect()
        } else {
            Vec::new()
        };
        let sock = policy
            .allow_agent
            .then(|| agent_socket(&hop.extras))
            .flatten();
        if let Some(sock) = sock {
            // An agent that is not running is simply skipped (US1 scenario 2).
            if let Ok(mut agent) = AgentClient::connect_uds(&sock).await {
                let keys = agent
                    .request_identities()
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|id| match id {
                        AgentIdentity::PublicKey { key, .. } => Some(key),
                        _ => None,
                    })
                    .collect();
                for key in order_keys(keys, &configured, only) {
                    let hash = if key.algorithm().is_rsa() {
                        handle
                            .best_supported_rsa_hash()
                            .await
                            .ok()
                            .flatten()
                            .flatten()
                    } else {
                        None
                    };
                    let res = handle
                        .authenticate_publickey_with(user, key.clone(), hash, &mut agent)
                        .await;
                    if matches!(res, Ok(AuthResult::Success)) {
                        return Ok(());
                    }
                    offered.push(key);
                }
            }
        }
        if policy.allow_pubkey {
            for id in &hop.identity_files {
                if read_public(id)
                    .is_some_and(|p| offered.iter().any(|o| o.key_data() == p.key_data()))
                {
                    continue;
                }
                let Ok(key) = load_identity(id) else {
                    continue;
                };
                let kwh = PrivateKeyWithHashAlg::new(Arc::new(key), None);
                if let Ok(AuthResult::Success) = handle.authenticate_publickey(user, kwh).await {
                    return Ok(());
                }
            }
        }
    }
    if policy.allow_interactive && interactive_auth(handle, user).await? {
        return Ok(());
    }
    Err(TransportError::Auth(format!(
        "no accepted authentication method for `{user}@{}`",
        hop.host
    )))
}

#[async_trait::async_trait]
impl Transport for RusshTransport {
    async fn connect(
        &mut self,
        target: &ResolvedTarget,
        auth: &AuthPolicy,
    ) -> Result<(), TransportError> {
        // SSH family only: a container target fails fast, no network action (C-T6).
        let ResolvedTarget::Ssh(target) = target else {
            return Err(TransportError::BackendUnavailable(
                "russh backend serves SSH targets only; container targets need the \
                 container runtime backend"
                    .into(),
            ));
        };
        let hops = jump_chain(
            &target.alias,
            target.user.clone(),
            target.port,
            &resolve_hop,
        )?;
        let last = hops.len() - 1;
        let forward = target
            .forward_agent
            .or(hops[last].extras.forward_agent)
            .unwrap_or(false);
        let agent_forward = if forward {
            agent_socket(&hops[last].extras)
        } else {
            None
        };

        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(3600)),
            ..Default::default()
        });
        let timeout = Duration::from_secs(target.connect_timeout_s);
        let mut handles: Vec<Handle<ClientHandler>> = Vec::new();
        for (i, hop) in hops.iter().enumerate() {
            let handler = ClientHandler {
                host: hop.host.clone(),
                port: hop.port,
                agent_forward: if i == last {
                    agent_forward.clone()
                } else {
                    None
                },
            };
            // Bound each hop's connect with the configured timeout (§FR-031).
            let step = async {
                let mut handle = match handles.last() {
                    None => tokio::time::timeout(
                        timeout,
                        client::connect(config.clone(), (hop.host.as_str(), hop.port), handler),
                    )
                    .await
                    .map_err(|_| TransportError::Timeout(target.connect_timeout_s))?
                    .map_err(|e| classify_connect(&e))?,
                    Some(prev) => {
                        // Only a TCP forward is asked of the hop: nothing runs
                        // there and nothing is written there (C-J8).
                        let ch = prev
                            .channel_open_direct_tcpip(
                                hop.host.clone(),
                                u32::from(hop.port),
                                "127.0.0.1",
                                0,
                            )
                            .await
                            .map_err(|e| {
                                // The previous hop refused the tunnel: it is the
                                // one to name (C-J6).
                                TransportError::Connect(format!(
                                    "via {}: cannot open a tunnel to {}:{}: {e}",
                                    hops[i - 1].label,
                                    hop.host,
                                    hop.port
                                ))
                            })?;
                        tokio::time::timeout(
                            timeout,
                            client::connect_stream(config.clone(), ch.into_stream(), handler),
                        )
                        .await
                        .map_err(|_| TransportError::Timeout(target.connect_timeout_s))?
                        .map_err(|e| classify_connect(&e))?
                    }
                };
                let explicit = if i == last {
                    target.identity.as_ref()
                } else {
                    None
                };
                authenticate(&mut handle, hop, explicit, auth).await?;
                Ok::<_, TransportError>(handle)
            };
            let handle = match step.await {
                Ok(h) => h,
                // A hop names itself in the error; the target's own errors keep
                // their plain form (C-J6).
                Err(e) if i < last => return Err(via(&hop.label, e)),
                Err(e) => return Err(e),
            };
            handles.push(handle);
        }

        self.handle = handles.pop();
        self.jumps = handles;
        self.forward_agent = agent_forward.is_some();
        Ok(())
    }

    async fn exec(&mut self, cmd: &str) -> Result<ExecOutput, TransportError> {
        let handle = self.handle_mut()?;
        let mut ch = handle.channel_open_session().await.map_err(te)?;
        ch.exec(true, cmd).await.map_err(te)?;
        collect_channel(&mut ch).await
    }

    async fn upload_stream(
        &mut self,
        remote_cmd: &str,
        data: Vec<u8>,
    ) -> Result<ExecOutput, TransportError> {
        let handle = self.handle_mut()?;
        let mut ch = handle.channel_open_session().await.map_err(te)?;
        ch.exec(true, remote_cmd).await.map_err(te)?;
        ch.data_bytes(data).await.map_err(te)?;
        ch.eof().await.map_err(te)?;
        collect_channel(&mut ch).await
    }

    async fn exec_stream(&mut self, cmd: &str) -> Result<i32, TransportError> {
        let handle = self.handle_mut()?;
        let ch = handle.channel_open_session().await.map_err(te)?;
        self.maybe_forward(&ch).await?;
        ch.exec(true, cmd).await.map_err(te)?;
        let (mut read_half, write_half) = ch.split();

        // Client stdin → channel; EOF on our stdin closes the command's stdin
        // (C-X2). The guard stops the forwarder if this future is dropped (C-X5).
        let mut writer = write_half.make_writer();
        let stdin_task = AbortOnDrop(tokio::spawn(async move {
            let mut stdin = tokio::io::stdin();
            let mut buf = [0u8; 8192];
            loop {
                match stdin.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if writer.write_all(&buf[..n]).await.is_err() {
                            return;
                        }
                    }
                }
            }
            let _ = writer.shutdown().await;
        }));

        let mut stdout = tokio::io::stdout();
        let mut stderr = tokio::io::stderr();
        let mut code = None;
        while let Some(msg) = read_half.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    stdout.write_all(&data).await?;
                    stdout.flush().await?;
                }
                ChannelMsg::ExtendedData { data, .. } => {
                    stderr.write_all(&data).await?;
                    stderr.flush().await?;
                }
                ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status as i32),
                ChannelMsg::ExitSignal { signal_name, .. } => {
                    code = Some(128 + signal_number(&signal_name));
                }
                _ => {}
            }
        }
        drop(stdin_task);
        // No status at all (connection lost mid-command) is "unknown" (C-X3).
        Ok(code.unwrap_or(255))
    }

    async fn open_pty(&mut self, spec: &PtySpec) -> Result<i32, TransportError> {
        let handle = self.handle_mut()?;
        let ch = handle.channel_open_session().await.map_err(te)?;
        self.maybe_forward(&ch).await?;
        // Use the real local terminal size; spec dims are the non-tty fallback.
        let (cols, rows) = local_tty_size().unwrap_or((spec.cols, spec.rows));
        ch.request_pty(
            true,
            &spec.term,
            u32::from(cols),
            u32::from(rows),
            0,
            0,
            &[],
        )
        .await
        .map_err(te)?;

        // Prefix env exports so the remote shell init sees them (⭐ nix runtime vars).
        let mut prefix = String::new();
        for (k, v) in &spec.env {
            prefix.push_str(&format!("export {k}={}; ", shell_quote(v)));
        }
        ch.exec(true, format!("{prefix}exec {}", spec.shell_cmd))
            .await
            .map_err(te)?;

        let (mut read_half, write_half) = ch.split();
        let write_half = std::sync::Arc::new(write_half);

        // Raw mode for the interactive phase: keystrokes (Tab, Ctrl-C, …) go to
        // the remote PTY as bytes instead of being interpreted by the local tty
        // (Ctrl-C must interrupt the remote command, not kill the session).
        // The guard restores the terminal on scope exit, panics included.
        let _raw = RawModeGuard::enter();

        // Forward local stdin to the channel; stream remote output to std{out,err}.
        let mut writer = write_half.make_writer();
        let stdin_task = tokio::spawn(async move {
            let mut stdin = tokio::io::stdin();
            let mut buf = [0u8; 4096];
            loop {
                match stdin.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if writer.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // Propagate local terminal resizes to the remote PTY (SIGWINCH).
        let winch_half = write_half.clone();
        let winch_task = tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let Ok(mut winch) = signal(SignalKind::window_change()) else {
                return;
            };
            while winch.recv().await.is_some() {
                if let Some((c, r)) = local_tty_size() {
                    let _ = winch_half
                        .window_change(u32::from(c), u32::from(r), 0, 0)
                        .await;
                }
            }
        });

        let mut stdout = tokio::io::stdout();
        let mut stderr = tokio::io::stderr();
        let mut code = 0;
        while let Some(msg) = read_half.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    let _ = stdout.write_all(&data).await;
                    let _ = stdout.flush().await;
                }
                ChannelMsg::ExtendedData { data, .. } => {
                    let _ = stderr.write_all(&data).await;
                    let _ = stderr.flush().await;
                }
                ChannelMsg::ExitStatus { exit_status } => code = exit_status as i32,
                _ => {}
            }
        }
        stdin_task.abort();
        winch_task.abort();
        Ok(code)
    }

    async fn disconnect(&mut self) -> Result<(), TransportError> {
        if let Some(handle) = self.handle.take() {
            let _ = handle.disconnect(Disconnect::ByApplication, "", "").await;
        }
        // Innermost hop first, as the tunnels nest.
        while let Some(hop) = self.jumps.pop() {
            let _ = hop.disconnect(Disconnect::ByApplication, "", "").await;
        }
        Ok(())
    }
}

/// Aborts the wrapped task when dropped, so a cancelled `exec_stream` never
/// leaves a forwarder reading the client's stdin.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// POSIX number of the signal an SSH `exit-signal` names (0 when unknown).
fn signal_number(sig: &russh::Sig) -> i32 {
    match sig {
        russh::Sig::HUP => 1,
        russh::Sig::INT => 2,
        russh::Sig::QUIT => 3,
        russh::Sig::ILL => 4,
        russh::Sig::ABRT => 6,
        russh::Sig::FPE => 8,
        russh::Sig::KILL => 9,
        russh::Sig::USR1 => 10,
        russh::Sig::SEGV => 11,
        russh::Sig::PIPE => 13,
        russh::Sig::ALRM => 14,
        russh::Sig::TERM => 15,
        _ => 0,
    }
}

/// Collect stdout/stderr/exit-code from a session channel until it closes.
async fn collect_channel(
    ch: &mut russh::Channel<client::Msg>,
) -> Result<ExecOutput, TransportError> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_code = -1;
    while let Some(msg) = ch.wait().await {
        match msg {
            ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, .. } => stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => exit_code = exit_status as i32,
            _ => {}
        }
    }
    Ok(ExecOutput {
        exit_code,
        stdout,
        stderr,
    })
}

/// Interactive auth: try keyboard-interactive, then password (§FR-029a). Prompts
/// are read from the controlling terminal; responses are never logged (Принцип V).
async fn interactive_auth(
    handle: &mut Handle<ClientHandler>,
    user: &str,
) -> Result<bool, TransportError> {
    let mut resp = handle
        .authenticate_keyboard_interactive_start(user, None)
        .await
        .map_err(|e| TransportError::Auth(e.to_string()))?;
    loop {
        match resp {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => break,
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let mut answers = Vec::with_capacity(prompts.len());
                for p in &prompts {
                    answers.push(read_secret(&p.prompt)?);
                }
                resp = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(|e| TransportError::Auth(e.to_string()))?;
            }
        }
    }

    // Password fallback.
    let pw = read_secret(&format!("{user}'s password: "))?;
    let ok = handle
        .authenticate_password(user, pw)
        .await
        .map_err(|e| TransportError::Auth(e.to_string()))?
        .success();
    Ok(ok)
}

/// Read a secret from the terminal. (Echo suppression is a refinement; the value is
/// never logged regardless.)
fn read_secret(prompt: &str) -> Result<String, TransportError> {
    // A prompt needs a terminal. With stdin redirected (a pipeline, a script, the
    // one-command mode) the "answer" would be the command's own input — refuse
    // instead of reading it (004 §FR-009, C-XC6).
    if !crate::tty::stdin_is_tty() {
        return Err(TransportError::Auth(format!(
            "interactive authentication is needed ({}) but stdin is not a terminal; \
             load the key into ssh-agent, use an unencrypted key, or run xxh from \
             a terminal",
            prompt.trim().trim_end_matches(':')
        )));
    }
    use std::io::Write;
    eprint!("{prompt}");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(TransportError::Io)?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// Load a private key; an encrypted key prompts for its passphrase once
/// (the passphrase is read from the terminal and never logged, Принцип V).
fn load_identity(path: &std::path::Path) -> Result<russh::keys::PrivateKey, TransportError> {
    match load_secret_key(path, None) {
        Ok(key) => Ok(key),
        Err(russh::keys::Error::KeyIsEncrypted) => {
            let pass = read_secret(&format!("passphrase for `{}`: ", path.display()))?;
            load_secret_key(path, Some(&pass)).map_err(|e| {
                TransportError::Auth(format!("cannot decrypt identity `{}`: {e}", path.display()))
            })
        }
        Err(e) => Err(TransportError::Auth(format!(
            "cannot load identity `{}`: {e}",
            path.display()
        ))),
    }
}

/// The identity files OpenSSH tries when the config names none.
fn default_identity_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let dir = std::path::Path::new(&home).join(".ssh");
        for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
            let p = dir.join(name);
            if p.is_file() {
                out.push(p);
            }
        }
    }
    out
}

/// Verify a server key against `~/.ssh/known_hosts`. Returns false (→ auth fails →
/// HostKey error) if the host is pinned with a different key; true otherwise.
fn known_hosts_check(host: &str, port: u16, server_key: &PublicKey) -> bool {
    let Ok(server_line) = server_key.to_openssh() else {
        return false;
    };
    let server_b64 = server_line.split_whitespace().nth(1).unwrap_or("");

    let Some(home) = std::env::var_os("HOME") else {
        return true; // no HOME → cannot check; accept (TOFU)
    };
    let path = std::path::Path::new(&home).join(".ssh").join("known_hosts");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return true; // no known_hosts yet → accept (TOFU)
    };

    known_hosts_verdict(&text, host, port, server_b64)
}

/// The known_hosts decision on `text`: a host on a non-standard port is also
/// looked up as `[host]:port`, as OpenSSH writes it (012 research R6).
fn known_hosts_verdict(text: &str, host: &str, port: u16, server_b64: &str) -> bool {
    let bracketed = format!("[{host}]:{port}");
    let mut host_pinned = false;
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(hosts), Some(_kt), Some(b64)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if hosts
            .split(',')
            .any(|h| h == host || (port != 22 && h == bracketed))
        {
            host_pinned = true;
            if b64 == server_b64 {
                return true; // known and matches
            }
        }
    }
    // Pinned but never matched → mismatch → reject. Not pinned → accept (TOFU).
    !host_pinned
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "root".to_string())
}

fn classify_connect(e: &russh::Error) -> TransportError {
    TransportError::Connect(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> PublicKey {
        let sk = russh::keys::PrivateKey::from(
            russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[seed; 32]),
        );
        sk.public_key().clone()
    }

    /// C-J2: the host's configured keys first; `IdentitiesOnly` keeps only them.
    #[test]
    fn agent_keys_follow_the_host_config() {
        let (a, b, c) = (key(1), key(2), key(3));
        let agent = vec![a.clone(), b.clone(), c.clone()];
        let mut configured_b = b.clone();
        configured_b.set_comment("from the .pub file");
        assert_eq!(
            order_keys(agent.clone(), &[configured_b.clone()], false),
            vec![b.clone(), a.clone(), c.clone()]
        );
        assert_eq!(order_keys(agent.clone(), &[configured_b], true), vec![b]);
        assert_eq!(order_keys(agent.clone(), &[], false), agent);
    }

    #[test]
    fn jump_entries_parse() {
        assert_eq!(parse_jump("bastion"), (None, "bastion".into(), None));
        assert_eq!(
            parse_jump(" ops@bastion:2222 "),
            (Some("ops".into()), "bastion".into(), Some(2222))
        );
        assert_eq!(parse_jump("[::1]:22"), (None, "::1".into(), Some(22)));
        assert_eq!(
            parse_jump("ssh://u@h:7"),
            (Some("u".into()), "h".into(), Some(7))
        );
    }

    fn hop(label: &str, jump: Option<&str>, cmd: Option<&str>) -> Hop {
        Hop {
            label: label.into(),
            host: format!("{label}.example"),
            port: 22,
            user: "u".into(),
            identity_files: Vec::new(),
            configured_ids: false,
            extras: HostExtras::default(),
            proxy_jump: jump.map(str::to_string),
            proxy_command: cmd.map(str::to_string),
        }
    }

    /// A tiny ssh_config: name → (ProxyJump, ProxyCommand).
    fn table(
        entries: &'static [(&'static str, Option<&'static str>, Option<&'static str>)],
    ) -> impl Fn(&str, Option<String>, Option<u16>) -> Hop {
        move |name, user, port| {
            let (_, j, c) = entries
                .iter()
                .find(|(n, _, _)| *n == name)
                .copied()
                .unwrap_or((name, None, None));
            let mut h = hop(name, j, c);
            if let Some(u) = user {
                h.user = u;
            }
            if let Some(p) = port {
                h.port = p;
            }
            h
        }
    }

    fn labels(hops: &[Hop]) -> Vec<&str> {
        hops.iter().map(|h| h.label.as_str()).collect()
    }

    /// C-J4: the first jump host's own ProxyJump is followed; later list entries
    /// go through the previous entry; `none` means direct.
    #[test]
    fn chains_follow_openssh_rules() {
        let r = table(&[
            ("target", Some("b1,b2"), None),
            ("b1", Some("outer"), None),
            ("b2", Some("ignored"), None),
            ("direct", None, None),
        ]);
        let hops = jump_chain("target", None, None, &r).unwrap();
        assert_eq!(labels(&hops), ["outer", "b1", "b2", "target"]);
        assert_eq!(
            labels(&jump_chain("direct", None, None, &r).unwrap()),
            ["direct"]
        );

        let r = table(&[("t", Some("ops@b:2200"), None)]);
        let hops = jump_chain("t", None, None, &r).unwrap();
        assert_eq!((hops[0].user.as_str(), hops[0].port), ("ops", 2200));
    }

    #[test]
    fn loops_depth_and_proxy_command_are_refused() {
        let r = table(&[("a", Some("b"), None), ("b", Some("a"), None)]);
        assert!(matches!(
            jump_chain("a", None, None, &r),
            Err(TransportError::Connect(m)) if m.contains("loops")
        ));
        let r = table(&[
            ("h0", Some("h1"), None),
            ("h1", Some("h2"), None),
            ("h2", Some("h3"), None),
            ("h3", Some("h4"), None),
            ("h4", Some("h5"), None),
            ("h5", Some("h6"), None),
            ("h6", Some("h7"), None),
            ("h7", Some("h8"), None),
            ("h8", Some("h9"), None),
        ]);
        assert!(jump_chain("h0", None, None, &r).is_err(), "deeper than 8");
        let r = table(&[("t", None, Some("nc %h %p"))]);
        match jump_chain("t", None, None, &r) {
            Err(TransportError::BackendUnavailable(m)) => {
                assert!(m.contains("--transport ssh"), "{m}");
                assert!(!m.contains("nc"), "the command line is not echoed");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn hop_errors_name_the_hop_and_keep_their_class() {
        assert!(matches!(
            via("bastion", TransportError::Auth("denied".into())),
            TransportError::Auth(m) if m == "via bastion: denied"
        ));
        assert!(matches!(
            via("bastion", TransportError::Timeout(10)),
            TransportError::Connect(m) if m.contains("via bastion") && m.contains("10s")
        ));
    }

    #[test]
    fn known_hosts_accepts_bracketed_ports() {
        let text = "[h]:2222 ssh-ed25519 AAAA\nplain ssh-ed25519 BBBB\n";
        assert!(known_hosts_verdict(text, "h", 2222, "AAAA"));
        assert!(
            !known_hosts_verdict(text, "h", 2222, "ZZZZ"),
            "mismatch rejected"
        );
        assert!(
            known_hosts_verdict(text, "h", 22, "ZZZZ"),
            "port 22 is plain `h`: unpinned"
        );
        assert!(known_hosts_verdict(text, "plain", 2022, "BBBB"));
        assert!(!known_hosts_verdict(text, "plain", 2022, "CCCC"));
    }
}
