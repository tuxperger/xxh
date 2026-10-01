//! Integration (012 T006/T013): the built-in client and a real `ssh-agent`.
//!
//! - the key is only in the agent (its file moved away): login and a command
//!   work with the default transport (US1 scenario 1);
//! - an agent socket that leads nowhere is skipped: the key file is used
//!   (US1 scenario 2);
//! - the agent reaches the user's command only when forwarding was asked for
//!   (US3, §FR-006); the host ends clean.
//!
//! Requires docker and `ssh-agent`/`ssh-add` on the client; skips otherwise.

mod common;

use std::process::Command;

use common::{Fixture, docker_available, eff};
use xxh_config::CleanupMode;
use xxh_core::session::{ExecCommand, Session, minimal_env_component, silent_progress};
use xxh_transport::{ResolvedTarget, RusshTransport};

fn set_agent(sock: &std::path::Path) {
    #[allow(unsafe_code)]
    // SAFETY: single-test binary; set between sessions, never concurrently.
    unsafe {
        std::env::set_var("SSH_AUTH_SOCK", sock);
    }
}

#[test]
fn agent_auth_and_forwarding() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    if Command::new("ssh-agent").arg("-h").output().is_err() {
        eprintln!("skipping: ssh-agent not available");
        return;
    }
    let fx = Fixture::boot();
    let sock = fx.home.join("agent.sock");
    let started = Command::new("ssh-agent")
        .args(["-a", sock.to_str().unwrap(), "-s"])
        .output()
        .expect("start ssh-agent");
    let out = String::from_utf8_lossy(&started.stdout);
    let pid = out
        .split("SSH_AGENT_PID=")
        .nth(1)
        .and_then(|s| s.split(';').next())
        .expect("agent pid")
        .to_string();
    let key = fx.home.join(".ssh/id_ed25519");
    let added = Command::new("ssh-add")
        .arg(&key)
        .env("SSH_AUTH_SOCK", &sock)
        .output()
        .expect("ssh-add");
    assert!(added.status.success(), "{added:?}");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let sh = eff("sh", CleanupMode::Ephemeral);
    let env = vec![minimal_env_component("gz").unwrap()];
    let line = |l: &str| ExecCommand::ShellLine(l.to_string());
    let with_forwarding = |on: bool| {
        let ResolvedTarget::Ssh(mut t) = fx.target() else {
            unreachable!()
        };
        t.forward_agent = on.then_some(true);
        ResolvedTarget::Ssh(t)
    };

    rt.block_on(async {
        // The key exists only in the agent.
        let away = fx.home.join("id_ed25519.away");
        std::fs::rename(&key, &away).unwrap();
        set_agent(&sock);
        let mut s = Session::establish(
            RusshTransport::new(),
            &with_forwarding(false),
            &sh,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("login with the agent's key");
        // Not forwarded unless asked: the command sees no agent.
        assert_eq!(
            s.run_exec(&line("test -z \"${SSH_AUTH_SOCK:-}\""), false)
                .await
                .expect("run"),
            0,
            "no agent without -A"
        );
        s.finish().await.expect("finish");

        // Forwarding asked for: the command sees the agent socket.
        let mut s = Session::establish(
            RusshTransport::new(),
            &with_forwarding(true),
            &sh,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("login with forwarding");
        assert_eq!(
            s.run_exec(&line("test -S \"$SSH_AUTH_SOCK\""), false)
                .await
                .expect("run"),
            0,
            "the forwarded agent is visible"
        );
        s.finish().await.expect("finish");

        // An agent socket that leads nowhere is skipped; the file is used.
        std::fs::rename(&away, &key).unwrap();
        set_agent(&fx.home.join("no-agent-here.sock"));
        let mut s = Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &sh,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("login with the key file");
        assert_eq!(s.run_command("true").await.expect("run"), 0);
        s.finish().await.expect("finish");

        assert_eq!(fx.cleanliness().await, "CLEAN");
    });
    let _ = Command::new("kill").arg(&pid).output();
}
