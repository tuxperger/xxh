//! `xxh <host>` — establish a session and hand over an interactive shell
//! (T019/T024/T027/T042). Stage progress goes to stderr (§FR-025); all error
//! classes surface distinguishably via `SessionError` (§FR-026).

use xxh_config::{Effective, TransportBackend};
use xxh_core::session::{Progress, Session, SessionError};
use xxh_transport::{ResolvedSshTarget, RusshTransport, SshCliTransport, Transport};

/// Stage-progress sink: short lines on stderr so they never mix with shell stdout.
fn progress() -> Progress<'static> {
    &|msg: &str| eprintln!("xxh: ▸ {msg}")
}

/// Connect to `host` with the effective settings and run the interactive shell.
/// Returns the remote shell's exit code.
pub async fn run(host: &str, eff: &Effective) -> Result<i32, SessionError> {
    // Select the transport backend (Принцип III); the rest of the flow is identical.
    match eff.transport {
        TransportBackend::Ssh => {
            let t = SshCliTransport::new().map_err(SessionError::from)?;
            connect_and_run(t, host, eff).await
        }
        TransportBackend::Russh => connect_and_run(RusshTransport::new(), host, eff).await,
    }
}

async fn connect_and_run<T: Transport>(
    transport: T,
    host: &str,
    eff: &Effective,
) -> Result<i32, SessionError> {
    let mut target = ResolvedSshTarget::new(host);
    target.connect_timeout_s = eff.connect_timeout_s;

    // Base environment component (session marker + demo alias); gzip is safe
    // before host capabilities are known (part of the minimal host contract).
    let env = vec![xxh_core::session::minimal_env_component("gz")?];
    // Enabled plugins in resolved load order; resolution failures abort before
    // anything reaches the host (§FR-021).
    let plugins = crate::commands::plugin::session_plugins(eff)?;

    let mut session =
        Session::establish(transport, &target, eff, &env, &plugins, progress()).await?;
    progress()(&format!("shell {}", eff.shell));
    // The resolved shell is launched over a PTY; on exit the remote trap cleans up.
    let code = session.run_interactive().await?;
    session.finish().await?;
    Ok(code)
}
