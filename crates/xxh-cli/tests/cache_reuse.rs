//! Integration tests (T029/T030, C-IT-S4/S5): with `--keep` the content-addressed
//! cache survives between sessions and a second entry re-transfers **nothing**
//! (§FR-012..014, §SC-004); without `--keep` the host ends clean.
//!
//! Requires docker; skips (passes) when docker is absent.

mod common;

use common::{Fixture, docker_available, eff};
use xxh_config::CleanupMode;
use xxh_core::session::{Session, minimal_env_component, silent_progress};
use xxh_transport::RusshTransport;

#[test]
fn keep_reuses_cache_and_ephemeral_cleans_up() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    let fx = Fixture::boot();
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(async {
        let keep = eff("sh", CleanupMode::Keep);
        let env = vec![minimal_env_component("gz").unwrap()];

        // Session 1 (--keep): everything is new and gets delivered.
        let t1 = std::time::Instant::now();
        let mut s1 = Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &keep,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("first establish");
        let t1 = t1.elapsed();
        let r1 = s1.delivery_report();
        assert_eq!(r1.reused, 0, "first entry has nothing to reuse");
        assert!(
            r1.delivered >= 1,
            "first entry must deliver the env component"
        );
        assert_eq!(s1.run_command("true").await.expect("run"), 0);
        s1.finish().await.expect("finish 1");

        // Keep mode: the cache must survive the session (§FR-012, inverse assert).
        assert_eq!(fx.cleanliness().await, "DIRTY", "--keep must retain ~/.xxh");
        let cached = fx.host_exec("ls -1 $HOME/.xxh/cache | wc -l").await;
        assert!(
            cached.trim().parse::<u32>().unwrap() >= 1,
            "cache must hold the delivered component"
        );

        // Session 2 (--keep): identical components ⇒ all reused, none delivered
        // (§FR-013/014, §SC-004: re-entry transfers only what changed — nothing).
        let t2 = std::time::Instant::now();
        let mut s2 = Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &keep,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("second establish");
        let t2 = t2.elapsed();
        // §SC-004 measurement record: re-entry establish time vs first entry.
        // The hard guarantee is `delivered == 0` below; timing is logged (not
        // asserted) because tiny test payloads make ratios noisy.
        eprintln!(
            "SC-004: first establish {t1:?}, cached re-entry {t2:?} ({:.0}% of first)",
            t2.as_secs_f64() / t1.as_secs_f64() * 100.0
        );
        let r2 = s2.delivery_report();
        assert_eq!(r2.delivered, 0, "unchanged components must not be re-sent");
        assert!(r2.reused >= 1, "cached components must be reused");
        assert_eq!(
            s2.run_command("test \"$XXH_SESSION\" = 1")
                .await
                .expect("run"),
            0,
            "env must work when served from cache"
        );
        s2.finish().await.expect("finish 2");

        // Session 3 (ephemeral, default): after exit the host is clean again —
        // the mandatory cleanliness assert (§FR-005, Принцип VIII).
        let ephemeral = eff("sh", CleanupMode::Ephemeral);
        let mut s3 = Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &ephemeral,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("third establish");
        assert_eq!(s3.run_command("true").await.expect("run"), 0);
        s3.finish().await.expect("finish 3");
        assert_eq!(
            fx.cleanliness().await,
            "CLEAN",
            "ephemeral session must leave the host as it was"
        );
    });
}
