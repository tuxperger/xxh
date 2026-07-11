//! Integration tests (002 T019/T020, US3): zero-footprint inside the container and
//! image immutability. A normal exit leaves the container's filesystem free of xxh
//! artefacts (`diff` clean) with the image digest unchanged (C-DT3); `--keep`
//! retains only the content-addressed cache so a re-entry re-delivers nothing
//! (Принцип VI); an abnormal disconnect is reconciled on the next entry (C-DT6).
//!
//! Requires a container runtime; skips (passes) with a message when absent (C-DT8).

mod common;

use common::{ContainerFixture, eff, runtime_available, test_runtime};
use xxh_config::CleanupMode;
use xxh_core::session::{Session, minimal_env_component, silent_progress};
use xxh_transport::ContainerCliTransport;

fn skip_if_no_runtime() -> bool {
    let runtime = test_runtime();
    if !runtime_available(&runtime) {
        eprintln!("skipping: container runtime `{runtime}` not available");
        return true;
    }
    false
}

#[test]
fn ephemeral_exit_leaves_container_clean_and_image_unchanged() {
    if skip_if_no_runtime() {
        return;
    }
    let fx = ContainerFixture::boot();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let eff = eff("sh", CleanupMode::Ephemeral);
        let env = vec![minimal_env_component("gz").unwrap()];
        let mut s = Session::establish(
            ContainerCliTransport::new(),
            &fx.target(),
            &eff,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("establish");
        s.run_command("true").await.expect("run");
        s.finish().await.expect("finish");
    });

    assert_eq!(
        fx.cleanliness(),
        "CLEAN",
        "xxh root must be gone after exit"
    );
    assert!(
        fx.diff_clean(),
        "container fs must be free of xxh artefacts:\n{}",
        fx.diff()
    );
    assert!(
        fx.image_digest_unchanged(),
        "the image must never be rebuilt/committed by a session"
    );
}

#[test]
fn keep_retains_cache_so_reentry_redelivers_nothing() {
    if skip_if_no_runtime() {
        return;
    }
    let fx = ContainerFixture::boot();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (first_delivered, second_delivered, second_reused) = rt.block_on(async {
        let eff = eff("sh", CleanupMode::Keep);
        // One env instance reused across entries: identical bytes → identical
        // content hash, so the second entry can recognise the cached component.
        let env = vec![minimal_env_component("gz").unwrap()];

        // First entry: everything is delivered and the cache is kept on exit.
        let mut s1 = Session::establish(
            ContainerCliTransport::new(),
            &fx.target(),
            &eff,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("establish #1");
        let first = s1.delivery_report();
        s1.run_command("true").await.expect("run #1");
        s1.finish().await.expect("finish #1");

        // Second entry with the identical content: the kept cache is reused, so
        // nothing is transferred again (Принцип VI, semantics from T003).
        let s2 = Session::establish(
            ContainerCliTransport::new(),
            &fx.target(),
            &eff,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("establish #2");
        let second = s2.delivery_report();
        s2.finish().await.expect("finish #2");
        (first.delivered, second.delivered, second.reused)
    });

    assert!(
        first_delivered >= 1,
        "first entry must deliver the env component"
    );
    assert_eq!(
        second_delivered, 0,
        "a kept cache must re-deliver nothing on re-entry"
    );
    assert!(
        second_reused >= 1,
        "the env component must be reused from the kept cache"
    );
}

#[test]
fn abnormal_disconnect_is_reconciled_on_next_entry() {
    if skip_if_no_runtime() {
        return;
    }
    let fx = ContainerFixture::boot();

    // Simulate a crashed session: an orphaned session marker with a dead PID plus
    // a stray cache entry, exactly what a killed client would leave behind (C-C14).
    fx.exec(
        "mkdir -p \"$HOME/.xxh/sessions\" \"$HOME/.xxh/cache/deadbeef\" && \
         echo 999999 > \"$HOME/.xxh/sessions/stale\"",
    );
    assert_eq!(
        fx.exec("test -e \"$HOME/.xxh/cache/deadbeef\" && echo PRESENT || echo GONE")
            .trim(),
        "PRESENT",
        "precondition: the stale artefacts exist before re-entry"
    );

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        // A fresh ephemeral entry runs `reconcile`, which sweeps the dead marker
        // and drops the orphaned root before proceeding (C-DT6).
        let eff = eff("sh", CleanupMode::Ephemeral);
        let env = vec![minimal_env_component("gz").unwrap()];
        let mut s = Session::establish(
            ContainerCliTransport::new(),
            &fx.target(),
            &eff,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("establish after crash");
        s.run_command("true").await.expect("run");
        s.finish().await.expect("finish");
    });

    assert_eq!(
        fx.cleanliness(),
        "CLEAN",
        "sweep + ephemeral exit must leave no root"
    );
    assert!(
        fx.diff_clean(),
        "no xxh artefacts may remain after reconcile:\n{}",
        fx.diff()
    );
}
