//! Integration (005 T012): `status`/`clean` in a running container behave as on an
//! SSH host (§FR-003): inspecting a fresh container writes nothing, a `--keep`
//! environment is found and removed, and afterwards the container's filesystem
//! carries no xxh artefacts and its image is unchanged (Принцип I, C-DT3).
//!
//! Requires a container runtime; skips (passes) with a message when absent.

mod common;

use common::{ContainerFixture, eff, runtime_available, test_runtime};
use xxh_config::CleanupMode;
use xxh_core::remote_env;
use xxh_core::session::{Session, minimal_env_component, silent_progress};
use xxh_transport::{AuthPolicy, ContainerCliTransport, Transport};

#[test]
fn status_and_clean_in_a_container() {
    let runtime = test_runtime();
    if !runtime_available(&runtime) {
        eprintln!("skipping: container runtime `{runtime}` not available");
        return;
    }
    let fx = ContainerFixture::boot();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut t = ContainerCliTransport::new();
        t.connect(&fx.target(), &AuthPolicy::default())
            .await
            .expect("connect");

        assert_eq!(remote_env::inspect(&mut t).await.expect("inspect"), vec![]);
        assert!(fx.diff_clean(), "status must not write:\n{}", fx.diff());

        let env = vec![minimal_env_component("gz").unwrap()];
        let mut s = Session::establish(
            ContainerCliTransport::new(),
            &fx.target(),
            &eff("sh", CleanupMode::Keep),
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("keep login");
        assert_eq!(s.run_command("true").await.expect("run"), 0);
        s.finish().await.expect("finish");

        let envs = remote_env::inspect(&mut t).await.expect("inspect");
        assert_eq!(envs.len(), 1, "{envs:?}");
        assert!(envs[0].kept);
        assert_eq!(envs[0].components.len(), 1);

        let done = remote_env::clean(&mut t, false).await.expect("clean");
        assert_eq!(done.removed.len(), 1, "{done:?}");
        assert!(done.left.is_empty() && done.refused.is_empty());
        t.disconnect().await.unwrap();
    });

    assert_eq!(fx.cleanliness(), "CLEAN");
    assert!(
        fx.diff_clean(),
        "container fs must be free of xxh artefacts:\n{}",
        fx.diff()
    );
    assert!(fx.image_digest_unchanged(), "the image must not change");
}
