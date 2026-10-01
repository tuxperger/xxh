//! `xxh <target>` — establish a session and hand over an interactive shell
//! (T019/T024/T027/T042; 002 T012). Stage progress goes to stderr (§FR-025); all
//! error classes surface distinguishably via `SessionError` (§FR-026).
//!
//! The backend comes from the single factory `target_io::open_transport` (C-T7):
//! `xxh-core` never branches on the family.

use xxh_config::Effective;
use xxh_core::session::{ExecCommand, Progress, Session, SessionError, silent_progress};
use xxh_transport::ResolvedTarget;

use super::target_io::open_transport;

/// Stage-progress sink: short lines on stderr so they never mix with shell stdout.
/// `quiet` silences it — a one-command run reports stages only on request, so a
/// script's stderr carries just the command's own output (004 §FR-012).
pub fn progress(quiet: bool) -> Progress<'static> {
    if quiet {
        silent_progress()
    } else {
        &|msg: &str| eprintln!("xxh: ▸ {msg}")
    }
}

/// The session's environment components, as every login builds them: the base
/// component (session marker + demo alias) and the client's terminfo entry for
/// $TERM — hosts rarely know modern terminals (ghostty/kitty/…), and a missing
/// entry breaks line editing. gzip is safe before host capabilities are known.
pub fn env_components() -> Result<Vec<xxh_core::deploy::Component>, SessionError> {
    let mut env = vec![xxh_core::session::minimal_env_component("gz")?];
    if let Some(ti) = xxh_core::session::terminfo_component("gz") {
        env.push(ti);
    }
    Ok(env)
}

/// Connect to `target` with the effective settings and run the interactive shell —
/// or, given `exec`, one command with the client's stdio attached (004). Returns
/// the remote shell's or command's exit code.
pub async fn run(
    target: ResolvedTarget,
    eff: &Effective,
    exec: Option<&(ExecCommand, bool)>,
    quiet: bool,
) -> Result<i32, SessionError> {
    let progress = progress(quiet);
    let (transport, target) = open_transport(target, eff, progress).await?;
    let env = env_components()?;
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
