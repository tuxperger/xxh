//! `xxh <target>` — establish a session and hand over an interactive shell
//! (T019/T024/T027/T042; 002 T012). Stage progress goes to stderr (§FR-025); all
//! error classes surface distinguishably via `SessionError` (§FR-026).
//!
//! The backend is chosen here — the single factory point (C-T7): the SSH family
//! (russh / system-ssh) for `ResolvedTarget::Ssh`, the container runtime backend
//! for `ResolvedTarget::Container`. `xxh-core` never branches on the family.

use xxh_config::{Effective, TransportBackend};
use xxh_core::session::{ExecCommand, Progress, Session, SessionError, silent_progress};
use xxh_transport::{
    ContainerCliTransport, ResolvedTarget, RuntimeSelector, RusshTransport, SshCliTransport,
    Transport,
};

/// Stage-progress sink: short lines on stderr so they never mix with shell stdout.
/// `quiet` silences it — a one-command run reports stages only on request, so a
/// script's stderr carries just the command's own output (004 §FR-012).
fn progress(quiet: bool) -> Progress<'static> {
    if quiet {
        silent_progress()
    } else {
        &|msg: &str| eprintln!("xxh: ▸ {msg}")
    }
}

/// Connect to `target` with the effective settings and run the interactive shell —
/// or, given `exec`, one command with the client's stdio attached (004). Returns
/// the remote shell's or command's exit code. The backend is selected by target family
/// and (for SSH) the configured transport; nothing downstream knows the difference.
pub async fn run(
    target: ResolvedTarget,
    eff: &Effective,
    exec: Option<&(ExecCommand, bool)>,
    quiet: bool,
) -> Result<i32, SessionError> {
    match &target {
        ResolvedTarget::Ssh(_) => match eff.transport {
            TransportBackend::Ssh => {
                let t = SshCliTransport::new().map_err(SessionError::from)?;
                connect_and_run(t, target, eff, exec, quiet).await
            }
            TransportBackend::Russh => {
                connect_and_run(RusshTransport::new(), target, eff, exec, quiet).await
            }
        },
        ResolvedTarget::Container(ct) => {
            // Resolve the runtime once (auto-order docker → podman) so the chosen
            // one can be reported (C-A3/C-C15); the backend then just verifies it.
            let rt = xxh_transport::resolve_runtime(ct.runtime, ct.connect_timeout_s)
                .await
                .map_err(SessionError::from)?;
            progress(quiet)(&format!("runtime {rt}"));
            let mut ct = ct.clone();
            ct.runtime = RuntimeSelector::Explicit(rt);
            connect_and_run(
                ContainerCliTransport::new(),
                ResolvedTarget::Container(ct),
                eff,
                exec,
                quiet,
            )
            .await
        }
    }
}

async fn connect_and_run<T: Transport>(
    transport: T,
    target: ResolvedTarget,
    eff: &Effective,
    exec: Option<&(ExecCommand, bool)>,
    quiet: bool,
) -> Result<i32, SessionError> {
    let progress = progress(quiet);
    // Base environment component (session marker + demo alias); gzip is safe
    // before host capabilities are known (part of the minimal host contract).
    let mut env = vec![xxh_core::session::minimal_env_component("gz")?];
    // Ship the client's terminfo entry for $TERM: hosts rarely know modern
    // terminals (ghostty/kitty/…), and a missing entry breaks line editing.
    if let Some(ti) = xxh_core::session::terminfo_component("gz") {
        env.push(ti);
    }
    // Enabled plugins in resolved load order; resolution failures abort before
    // anything reaches the target (§FR-021).
    let plugins = crate::commands::plugin::session_plugins(eff)?;

    let mut session = Session::establish(transport, &target, eff, &env, &plugins, progress).await?;
    let code = match exec {
        // One command, stdio attached; the same remote trap cleans up on exit.
        Some((cmd, tty)) => session.run_exec(cmd, *tty).await?,
        None => {
            progress(&format!("shell {}", eff.shell));
            // The resolved shell is launched over a PTY; on exit the remote trap
            // cleans up.
            session.run_interactive().await?
        }
    };
    session.finish().await?;
    Ok(code)
}
