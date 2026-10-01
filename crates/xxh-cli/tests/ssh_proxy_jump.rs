//! Integration (012 T010): the built-in client reaches hosts that are only
//! reachable through a bastion, as described by `ProxyJump` in ~/.ssh/config.
//!
//! - bastion → target: login, a command on the *target*, the target clean
//!   afterwards and nothing left on the bastion (§FR-003, §FR-009);
//! - bastion → hop → target, a two-entry `ProxyJump` list (US2 scenario 2);
//! - an unreachable hop is a transport error naming that hop (§FR-005);
//! - a hop whose host key does not match known_hosts is refused (§FR-004).
//!
//! Requires docker; skips (passes) when docker is absent.

mod common;

use common::{Fixture, docker_available, eff};
use xxh_config::CleanupMode;
use xxh_core::session::{Session, SessionError, minimal_env_component, silent_progress};
use xxh_transport::{ResolvedSshTarget, ResolvedTarget, RusshTransport, TransportError};

fn alias(name: &str) -> ResolvedTarget {
    ResolvedTarget::Ssh(ResolvedSshTarget::new(name))
}

#[test]
fn logins_through_proxy_jump() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    let fx = Fixture::boot();
    let target = fx.hidden("target");
    let hop = fx.hidden("hop");
    let key = fx.home.join(".ssh/id_ed25519");
    std::fs::write(
        fx.home.join(".ssh/config"),
        format!(
            "Host bastion\n  HostName 127.0.0.1\n  Port {port}\n  User tester\n  IdentityFile {key}\n\
             Host inner\n  HostName {t}\n  User tester\n  ProxyJump bastion\n\
             Host deep\n  HostName {t}\n  User tester\n  ProxyJump bastion,tester@{h}\n\
             Host broken\n  HostName {t}\n  User tester\n  ProxyJump tester@127.0.0.1:1\n\
             Host spoofed\n  HostName {t}\n  User tester\n  ProxyJump bastion,tester@{h}\n",
            port = fx.port,
            key = key.display(),
            t = target.name,
            h = hop.name,
        ),
    )
    .unwrap();
    // Only the target has this file: proves where the command ran.
    target.exec("touch \"$HOME/I-AM-THE-TARGET\"");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let sh = eff("sh", CleanupMode::Ephemeral);
    let env = vec![minimal_env_component("gz").unwrap()];

    rt.block_on(async {
        for name in ["inner", "deep"] {
            let mut s = Session::establish(
                RusshTransport::new(),
                &alias(name),
                &sh,
                &env,
                &[],
                silent_progress(),
            )
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(
                s.run_command("test -f \"$HOME/I-AM-THE-TARGET\" && test \"$XXH_SESSION\" = 1")
                    .await
                    .expect("run"),
                0,
                "{name}: the command must run on the target, in the environment"
            );
            s.finish().await.expect("finish");
            assert_eq!(target.cleanliness(), "CLEAN", "{name}: target");
            assert_eq!(fx.cleanliness().await, "CLEAN", "{name}: bastion");
            assert_eq!(hop.cleanliness(), "CLEAN", "{name}: hop");
        }

        // An unreachable hop: transport class, and the hop is named.
        let err = Session::establish(
            RusshTransport::new(),
            &alias("broken"),
            &sh,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .err()
        .expect("unreachable hop");
        match &err {
            SessionError::Transport(TransportError::Connect(m)) => {
                assert!(m.contains("via 127.0.0.1"), "{m}");
            }
            other => panic!("expected a transport error naming the hop: {other:?}"),
        }

        // The hop pinned with another key: refused like a direct host would be.
        let kh = fx.home.join(".ssh/known_hosts");
        let mut pinned = std::fs::read_to_string(&kh).unwrap();
        pinned.push_str(&format!(
            "{} ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOJzWa1kQ4XtW0cDJfnlQ2pVLM5w9KH8n3o3jC2Zx4kE\n",
            hop.name
        ));
        std::fs::write(&kh, pinned).unwrap();
        let err = Session::establish(
            RusshTransport::new(),
            &alias("spoofed"),
            &sh,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .err()
        .expect("mismatched hop key");
        assert!(
            matches!(&err, SessionError::Transport(_)) && err.to_string().contains(&hop.name),
            "{err}"
        );
        assert_eq!(target.cleanliness(), "CLEAN");
    });
}
