//! Integration tests (T029/T030, C-IT-S4/S5; 023 T008): with `--keep` the
//! content-addressed cache survives between sessions and a second entry
//! re-transfers **nothing** — even generated components re-created by the client
//! (§FR-012..014, 023 §FR-002, §SC-001); a changed plugin is the only thing sent
//! again (023 §FR-004); without `--keep` the host ends clean.
//!
//! Requires docker; skips (passes) when docker is absent.

mod common;

use std::path::Path;

use common::{Fixture, docker_available, eff};
use xxh_config::CleanupMode;
use xxh_core::session::{Session, SessionPlugin, minimal_env_component, silent_progress};
use xxh_plugins::Manifest;
use xxh_transport::RusshTransport;

const MANIFEST: &str = "name = \"tool\"\nversion = \"1.0.0\"\napi_version = \"1.0.0\"\n";

/// A plugin shipping an executable: proves the archive keeps the mode bits (C-A7).
fn write_plugin(dir: &Path, says: &str) {
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    std::fs::write(dir.join("plugin.toml"), MANIFEST).unwrap();
    std::fs::write(
        dir.join("env.sh"),
        "export PATH=\"$XXH_COMPONENT_DIR/bin:$PATH\"\n",
    )
    .unwrap();
    let tool = dir.join("bin/xxh-tool");
    std::fs::write(&tool, format!("#!/bin/sh\necho {says}\n")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn keep_reuses_cache_and_ephemeral_cleans_up() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    let fx = Fixture::boot();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let plugin_dir = std::env::temp_dir().join(format!("xxh-reuse-plugin-{}", std::process::id()));
    write_plugin(&plugin_dir, "v1");
    // Keep the client's archive cache away from the developer's (023 T006).
    let pack_cache = plugin_dir.with_extension("packed");
    #[allow(unsafe_code)]
    // SAFETY: single-test binary, set before any thread is spawned.
    unsafe {
        std::env::set_var("XXH_PACK_CACHE_DIR", &pack_cache);
    }
    let plugins = vec![SessionPlugin {
        manifest: Manifest::parse(MANIFEST).unwrap(),
        dir: plugin_dir.clone(),
    }];

    rt.block_on(async {
        let keep = eff("sh", CleanupMode::Keep);

        // Session 1 (--keep): everything is new and gets delivered.
        let t1 = std::time::Instant::now();
        let mut s1 = Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &keep,
            &[minimal_env_component("gz").unwrap()],
            &plugins,
            silent_progress(),
        )
        .await
        .expect("first establish");
        let t1 = t1.elapsed();
        let r1 = s1.delivery_report();
        assert_eq!(r1.reused, 0, "first entry has nothing to reuse");
        assert_eq!(r1.delivered, 2, "first entry delivers env and plugin");
        assert_eq!(
            s1.run_command("test \"$(xxh-tool)\" = v1")
                .await
                .expect("run"),
            0,
            "the plugin's executable must run on the host (modes kept, C-A7)"
        );
        s1.finish().await.expect("finish 1");

        // Keep mode: the cache must survive the session (§FR-012, inverse assert).
        assert_eq!(fx.cleanliness().await, "DIRTY", "--keep must retain ~/.xxh");
        let cached = fx.host_exec("ls -1 $HOME/.xxh/cache | wc -l").await;
        assert!(
            cached.trim().parse::<u32>().unwrap() >= 2,
            "cache must hold the delivered components"
        );

        // Session 2 (--keep): the env component is generated afresh, as on every
        // client run, yet nothing is delivered (023 §FR-002, C-A8, §SC-001).
        let t2 = std::time::Instant::now();
        let mut s2 = Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &keep,
            &[minimal_env_component("gz").unwrap()],
            &plugins,
            silent_progress(),
        )
        .await
        .expect("second establish");
        let t2 = t2.elapsed();
        // Measurement record: re-entry establish time vs first entry. The hard
        // guarantee is `delivered == 0` below; timing is logged (not asserted)
        // because tiny test payloads make ratios noisy.
        eprintln!(
            "SC-001: first establish {t1:?}, cached re-entry {t2:?} ({:.0}% of first)",
            t2.as_secs_f64() / t1.as_secs_f64() * 100.0
        );
        let r2 = s2.delivery_report();
        assert_eq!(r2.delivered, 0, "unchanged components must not be re-sent");
        assert_eq!(r2.reused, 2, "cached components must be reused");
        assert_eq!(
            s2.run_command("test \"$XXH_SESSION\" = 1 && test \"$(xxh-tool)\" = v1")
                .await
                .expect("run"),
            0,
            "env and plugin must work when served from cache"
        );
        s2.finish().await.expect("finish 2");

        // Session 3 (--keep): one plugin file edited ⇒ exactly that component is
        // sent again (023 §FR-004, US1 scenario 3).
        write_plugin(&plugin_dir, "v2");
        let mut s3 = Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &keep,
            &[minimal_env_component("gz").unwrap()],
            &plugins,
            silent_progress(),
        )
        .await
        .expect("third establish");
        let r3 = s3.delivery_report();
        assert_eq!(r3.delivered, 1, "only the edited plugin is re-sent");
        assert_eq!(r3.reused, 1);
        assert_eq!(
            s3.run_command("test \"$(xxh-tool)\" = v2")
                .await
                .expect("run"),
            0,
            "the edited plugin must be the one in use"
        );
        s3.finish().await.expect("finish 3");

        // Session 4 (ephemeral, default): after exit the host is clean again —
        // the mandatory cleanliness assert (§FR-005, Принцип VIII).
        let ephemeral = eff("sh", CleanupMode::Ephemeral);
        let mut s4 = Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &ephemeral,
            &[minimal_env_component("gz").unwrap()],
            &plugins,
            silent_progress(),
        )
        .await
        .expect("fourth establish");
        assert_eq!(s4.run_command("true").await.expect("run"), 0);
        s4.finish().await.expect("finish 4");
        assert_eq!(
            fx.cleanliness().await,
            "CLEAN",
            "ephemeral session must leave the host as it was"
        );
    });
    let _ = std::fs::remove_dir_all(&plugin_dir);
    let _ = std::fs::remove_dir_all(&pack_cache);
}
