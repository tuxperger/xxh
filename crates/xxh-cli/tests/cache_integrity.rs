//! Integration (014 T006): a kept environment is checked before it is used.
//!
//! - a file of a kept component changed on the target → the next login warns,
//!   sends exactly that component again, and the command sees the intact file
//!   (§FR-001..003, SC-001);
//! - a file added to a kept component → detected and replaced (US1 scenario 3);
//! - an unchanged environment → nothing is sent (§FR-008);
//! - the host ends clean.
//!
//! Requires docker; skips (passes) when docker is absent.

mod common;

use common::{Fixture, docker_available, eff};
use xxh_config::CleanupMode;
use xxh_core::session::{Session, minimal_env_component, silent_progress};
use xxh_transport::RusshTransport;

#[test]
fn tampered_kept_environment_is_replaced() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    let fx = Fixture::boot();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let keep = eff("sh", CleanupMode::Keep);
    let env = vec![minimal_env_component("gz").unwrap()];
    let hash = env[0].hash.clone();

    rt.block_on(async {
        let login = || async {
            Session::establish(
                RusshTransport::new(),
                &fx.target(),
                &keep,
                &env,
                &[],
                silent_progress(),
            )
            .await
            .expect("login")
        };

        let mut s = login().await;
        assert_eq!(s.delivery_report().delivered, 1);
        assert_eq!(s.run_command("true").await.expect("run"), 0);
        s.finish().await.expect("finish");

        // Tamper with the kept env.sh: the next login must not source it.
        fx.host_exec(&format!(
            "echo 'export XXH_SESSION=pwned' >> $HOME/.xxh/cache/{hash}/env.sh"
        ))
        .await;
        let mut s = login().await;
        assert_eq!(s.delivery_report().delivered, 1, "exactly the tampered one");
        assert!(
            s.notes()
                .iter()
                .any(|n| n.contains("modified") && n.contains("./env.sh was changed")),
            "{:?}",
            s.notes()
        );
        assert_eq!(
            s.run_command("test \"$XXH_SESSION\" = 1")
                .await
                .expect("run"),
            0,
            "the intact env.sh is in use"
        );
        s.finish().await.expect("finish");

        // A file planted next to it is just as foreign.
        fx.host_exec(&format!("echo x > $HOME/.xxh/cache/{hash}/planted"))
            .await;
        let s = login().await;
        assert_eq!(s.delivery_report().delivered, 1);
        assert!(
            s.notes().iter().any(|n| n.contains("./planted was added")),
            "{:?}",
            s.notes()
        );
        s.finish().await.expect("finish");
        let planted = fx
            .host_exec(&format!(
                "test -e $HOME/.xxh/cache/{hash}/planted && echo yes || echo no"
            ))
            .await;
        assert_eq!(planted.trim(), "no");

        // Unchanged: nothing sent, nothing to report.
        let s = login().await;
        assert_eq!(s.delivery_report().delivered, 0);
        assert!(s.notes().is_empty(), "{:?}", s.notes());
        s.finish().await.expect("finish");

        // Clean up the kept environment; the host ends clean.
        let mut s = Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &eff("sh", CleanupMode::Ephemeral),
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("ephemeral login");
        assert_eq!(s.run_command("true").await.expect("run"), 0);
        s.finish().await.expect("finish");
        assert_eq!(fx.cleanliness().await, "CLEAN");
    });
}
