//! Integration (006 T010): diagnosing a real sshd host.
//!
//! - on the standard image nothing fails, the host stays clean, and a login with
//!   the same settings then succeeds (US1 scenarios 1, 3, 4; SC-002);
//! - a shell that exists nowhere is a failed `shell` check with an action, and the
//!   host is still clean (§FR-006).
//!
//! Requires docker; skips (passes) when docker is absent.

mod common;

use common::{Fixture, docker_available, eff};
use xxh_config::CleanupMode;
use xxh_core::doctor::{CheckStatus, diagnose_target};
use xxh_core::session::{Session, minimal_env_component, silent_progress};
use xxh_transport::{AuthPolicy, RusshTransport, Transport};

#[test]
fn doctor_over_ssh() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    let fx = Fixture::boot();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let env = vec![minimal_env_component("gz").unwrap()];
        let sh = eff("sh", CleanupMode::Ephemeral);
        let mut t = RusshTransport::new();
        t.connect(&fx.target(), &AuthPolicy::default())
            .await
            .expect("connect");

        let report = diagnose_target(&mut t, "host", &sh, &env, &[])
            .await
            .expect("diagnose");
        let failed: Vec<_> = report
            .checks
            .iter()
            .filter(|c| c.status == CheckStatus::Fail)
            .collect();
        assert!(failed.is_empty(), "{failed:?}");
        assert!(report.platform.as_deref().unwrap().starts_with("linux/"));
        for id in ["platform", "tools", "root", "shell", "space"] {
            assert!(
                report.checks.iter().any(|c| c.id == id),
                "missing check {id}: {report:?}"
            );
        }
        assert_eq!(fx.cleanliness().await, "CLEAN", "doctor must not write");

        // All passed ⇒ the login works (US1 scenario 4).
        let mut s = Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &sh,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("login after a clean diagnosis");
        assert_eq!(s.run_command("true").await.expect("run"), 0);
        s.finish().await.expect("finish");

        // A shell that exists nowhere: a failure with a way out.
        let nosuch = eff("nosuch-shell", CleanupMode::Ephemeral);
        let report = diagnose_target(&mut t, "host", &nosuch, &env, &[])
            .await
            .expect("diagnose");
        let shell = report.checks.iter().find(|c| c.id == "shell").unwrap();
        assert_eq!(shell.status, CheckStatus::Fail, "{shell:?}");
        assert!(shell.action.as_deref().unwrap().contains("--shell"));
        t.disconnect().await.unwrap();

        assert_eq!(fx.cleanliness().await, "CLEAN");
    });
}
