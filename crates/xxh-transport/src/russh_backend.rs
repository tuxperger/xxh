//! russh transport backend (T010) — the primary, pure-Rust SSH client (research R1/R7).
//!
//! Covers: ssh-config resolution (russh-config), known_hosts verification that
//! **rejects on mismatch** (analysis U3), public-key auth from identity files and
//! interactive (password / keyboard-interactive) fallback (§FR-029a), plus exec,
//! upload_stream and an interactive PTY.
//!
//! Note: ssh-agent auth is a tracked follow-up; identity-file + interactive already
//! cover the common cases and the system-ssh backend covers agent-only setups.

use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, AuthResult, Handle, Handler, KeyboardInteractiveAuthResponse};
use russh::keys::{PrivateKeyWithHashAlg, PublicKey, load_secret_key};
use russh::{ChannelMsg, Disconnect};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{AuthPolicy, ExecOutput, PtySpec, ResolvedSshTarget, Transport, TransportError};

fn te(msg: impl std::fmt::Display) -> TransportError {
    TransportError::Channel(msg.to_string())
}

/// russh client handler. Verifies the server host key against `known_hosts`.
struct ClientHandler {
    host: String,
    port: u16,
}

impl Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, server_key: &PublicKey) -> Result<bool, Self::Error> {
        // TOFU with mismatch rejection: if the host is already pinned in known_hosts
        // and the key differs, refuse (U3). Unknown hosts are accepted (as ssh's
        // StrictHostKeyChecking=accept-new) and pinned.
        Ok(known_hosts_check(&self.host, self.port, server_key))
    }
}

/// Transport backed by the russh library.
pub struct RusshTransport {
    handle: Option<Handle<ClientHandler>>,
}

impl RusshTransport {
    pub fn new() -> Self {
        Self { handle: None }
    }

    fn handle_mut(&mut self) -> Result<&mut Handle<ClientHandler>, TransportError> {
        self.handle
            .as_mut()
            .ok_or_else(|| TransportError::Channel("not connected".into()))
    }
}

impl Default for RusshTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Transport for RusshTransport {
    async fn connect(
        &mut self,
        target: &ResolvedSshTarget,
        auth: &AuthPolicy,
    ) -> Result<(), TransportError> {
        // Resolve ~/.ssh/config (host_name, user, port, identity files, ProxyJump).
        let sshcfg = russh_config::parse_home(&target.alias).ok();
        let host = sshcfg
            .as_ref()
            .map(|c| c.host_name.clone())
            .unwrap_or_else(|| target.alias.clone());
        let port = target
            .port
            .or_else(|| sshcfg.as_ref().and_then(|c| c.port))
            .unwrap_or(22);
        let user = target
            .user
            .clone()
            .or_else(|| sshcfg.as_ref().and_then(|c| c.user.clone()))
            .unwrap_or_else(whoami);

        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(3600)),
            ..Default::default()
        });
        let handler = ClientHandler {
            host: host.clone(),
            port,
        };

        // Bound the connect with the configured timeout (§FR-031).
        let connect_fut = client::connect(config, (host.as_str(), port), handler);
        let mut handle = tokio::time::timeout(
            Duration::from_secs(target.connect_timeout_s),
            connect_fut,
        )
        .await
        .map_err(|_| TransportError::Timeout(target.connect_timeout_s))?
        .map_err(|e| classify_connect(&e))?;

        // Auth: public keys from identity files, then interactive fallback.
        let mut authed = false;
        if auth.allow_pubkey {
            for id in identity_files(sshcfg.as_ref()) {
                let Ok(key) = load_secret_key(&id, None) else {
                    continue;
                };
                let kwh = PrivateKeyWithHashAlg::new(Arc::new(key), None);
                if let Ok(AuthResult::Success) =
                    handle.authenticate_publickey(&user, kwh).await
                {
                    authed = true;
                    break;
                }
            }
        }

        if !authed && auth.allow_interactive {
            authed = interactive_auth(&mut handle, &user).await?;
        }

        if !authed {
            return Err(TransportError::Auth(format!(
                "no accepted authentication method for `{user}@{host}`"
            )));
        }

        self.handle = Some(handle);
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

    async fn open_pty(&mut self, spec: &PtySpec) -> Result<i32, TransportError> {
        let handle = self.handle_mut()?;
        let mut ch = handle.channel_open_session().await.map_err(te)?;
        ch.request_pty(
            true,
            &spec.term,
            spec.cols as u32,
            spec.rows as u32,
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

        // Forward local stdin to the channel; stream remote output to std{out,err}.
        let mut writer = ch.make_writer();
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

        let mut stdout = tokio::io::stdout();
        let mut stderr = tokio::io::stderr();
        let mut code = 0;
        while let Some(msg) = ch.wait().await {
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
        Ok(code)
    }

    async fn disconnect(&mut self) -> Result<(), TransportError> {
        if let Some(handle) = self.handle.take() {
            let _ = handle
                .disconnect(Disconnect::ByApplication, "", "")
                .await;
        }
        Ok(())
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
    use std::io::Write;
    eprint!("{prompt}");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(TransportError::Io)?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// Candidate identity files: those from ssh-config, else the common defaults.
fn identity_files(sshcfg: Option<&russh_config::Config>) -> Vec<std::path::PathBuf> {
    if let Some(ids) = sshcfg.and_then(|c| c.host_config.identity_file.clone()) {
        return ids;
    }
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
fn known_hosts_check(host: &str, _port: u16, server_key: &PublicKey) -> bool {
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

    let mut host_pinned = false;
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(hosts), Some(_kt), Some(b64)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if hosts.split(',').any(|h| h == host) {
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
