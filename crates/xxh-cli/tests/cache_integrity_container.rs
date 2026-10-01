//! Integration (014 T009): a target that cannot check a kept environment
//! (no `sha256sum`) does not get it trusted — the login warns and sends it
//! again, every time — and the same rules hold in a container (§FR-006,
//! §FR-009). The image is never changed.
//!
//! Requires a container runtime; skips (passes) when absent.

mod common;

use common::{ContainerFixture, eff, runtime_available, test_runtime};
use xxh_config::CleanupMode;
use xxh_core::session::{Session, minimal_env_component, silent_progress};
use xxh_transport::ContainerCliTransport;

#[test]
fn unverifiable_target_resends_the_kept_environment() {
    let runtime = test_runtime();
    if !runtime_available(&runtime) {
        eprintln!("skipping: container runtime `{runtime}` not available");
        return;
    }
    let fx = ContainerFixture::boot();
    // The running container loses its sha256sum (the image is untouched).
    fx.exec("rm -f \"$(command -v sha256sum)\"");
    assert_eq!(fx.exec("command -v sha256sum || echo none").trim(), "none");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let keep = eff("sh", CleanupMode::Keep);
    let env = vec![minimal_env_component("gz").unwrap()];
    rt.block_on(async {
        for round in 0..2 {
            let mut s = Session::establish(
                ContainerCliTransport::new(),
                &fx.target(),
                &keep,
                &env,
                &[],
                silent_progress(),
            )
            .await
            .expect("login");
            assert_eq!(
                s.delivery_report().delivered,
                1,
                "round {round}: nothing kept is trusted unchecked"
            );
            if round == 1 {
                assert!(
                    s.notes().iter().any(|n| n.contains("cannot be checked")),
                    "{:?}",
                    s.notes()
                );
            }
            assert_eq!(
                s.run_command("test \"$XXH_SESSION\" = 1")
                    .await
                    .expect("run"),
                0
            );
            s.finish().await.expect("finish");
        }
        let mut s = Session::establish(
            ContainerCliTransport::new(),
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
    });
    assert_eq!(fx.cleanliness(), "CLEAN");
    assert!(fx.image_digest_unchanged());
}
