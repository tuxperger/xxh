//! Integration (005 T011/T015): `status` and `clean` against a real sshd host.
//!
//! - `inspect` on a clean host finds nothing and leaves it clean (§FR-012, SC-003);
//! - after a `--keep` login it reports the kept root, its last use and current
//!   components (§FR-006, §FR-007);
//! - a live session blocks `clean` until forced (§FR-004, §FR-011, SC-004);
//! - `prune` keeps exactly what the client's plan uses (§FR-008), and the next
//!   login then delivers nothing;
//! - a crash leftover is removed without force; an undeletable entry is reported
//!   as left (§FR-005);
//! - the host ends clean (SC-001, Принцип VIII).
//!
//! Requires docker; skips (passes) when docker is absent.

mod common;

use std::path::Path;
use std::time::Duration;

use common::{Fixture, docker_available, eff};
use xxh_config::CleanupMode;
use xxh_core::remote_env::{self, ComponentState};
use xxh_core::session::{
    Session, SessionPlugin, detect_platform, minimal_env_component, plan_components,
    silent_progress,
};
use xxh_plugins::Manifest;
use xxh_transport::{AuthPolicy, RusshTransport, Transport};

const MANIFEST: &str = "name = \"extra\"\nversion = \"1.0.0\"\napi_version = \"1.0.0\"\n";

fn write_plugin(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("plugin.toml"), MANIFEST).unwrap();
    std::fs::write(dir.join("env.sh"), "export XXH_EXTRA=1\n").unwrap();
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[test]
fn status_and_clean_over_ssh() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    let fx = Fixture::boot();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let plugin_dir = std::env::temp_dir().join(format!("xxh-renv-plugin-{}", std::process::id()));
    write_plugin(&plugin_dir);
    let plugins = vec![SessionPlugin {
        manifest: Manifest::parse(MANIFEST).unwrap(),
        dir: plugin_dir.clone(),
    }];
    let keep = eff("sh", CleanupMode::Keep);

    rt.block_on(async {
        let mut t = RusshTransport::new();
        t.connect(&fx.target(), &AuthPolicy::default())
            .await
            .expect("connect");

        // A clean host: nothing found, nothing created (§FR-012, SC-003).
        assert_eq!(remote_env::inspect(&mut t).await.expect("inspect"), vec![]);
        assert_eq!(fx.cleanliness().await, "CLEAN", "status must not write");
        let none = remote_env::clean(&mut t, false).await.expect("clean");
        assert!(none.removed.is_empty() && none.left.is_empty());

        // Kept login with a plugin, then status: one kept root, current components.
        let env = vec![minimal_env_component("gz").unwrap()];
        let mut s = Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &keep,
            &env,
            &plugins,
            silent_progress(),
        )
        .await
        .expect("keep login");
        // `run` is what marks the environment kept and records the time.
        assert_eq!(s.run_command("true").await.expect("run"), 0);
        s.finish().await.expect("finish");

        let platform = detect_platform(&mut t).await.expect("detect");
        let plan = plan_components(&platform, &keep, &env, &plugins, silent_progress())
            .expect("plan")
            .components;
        let mut envs = remote_env::inspect(&mut t).await.expect("inspect");
        let summary = remote_env::classify(&mut envs, Some(&plan)).unwrap();
        assert_eq!(envs.len(), 1, "{envs:?}");
        let e = &envs[0];
        assert!(e.root.ends_with("/.xxh"));
        assert!(e.kept);
        let used = e.last_used.expect("a kept login records its time");
        assert!(
            now().abs_diff(used) < 60,
            "last use {used} vs now {}",
            now()
        );
        assert_eq!(e.components.len(), 2);
        assert!(
            e.components
                .iter()
                .all(|c| c.state == ComponentState::Current),
            "{e:?}"
        );
        assert!(summary.deliver.is_empty() && summary.reuse.len() == 2);

        // A live session blocks clean; nothing is touched (§FR-004, §FR-011).
        let mut live = Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &keep,
            &env,
            &plugins,
            silent_progress(),
        )
        .await
        .expect("live login");
        let running = tokio::spawn(async move {
            let _ = live.run_command("sleep 4").await;
            let _ = live.finish().await;
        });
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let refused = remote_env::clean(&mut t, false).await.expect("clean");
        assert_eq!(refused.refused.len(), 1, "{refused:?}");
        assert!(refused.removed.is_empty());
        assert_eq!(fx.cleanliness().await, "DIRTY", "refusal removes nothing");
        let refused = remote_env::prune(&mut t, false, &[]).await.expect("prune");
        assert_eq!(refused.refused.len(), 1, "pruning is guarded too");
        running.await.unwrap();

        // Plugin dropped from the client: prune keeps only the current plan, and
        // the next login has nothing to deliver (§FR-008).
        let plan: Vec<String> = plan_components(&platform, &keep, &env, &[], silent_progress())
            .expect("plan")
            .components
            .into_iter()
            .map(|c| c.hash)
            .collect();
        let pruned = remote_env::prune(&mut t, false, &plan)
            .await
            .expect("prune");
        assert_eq!(pruned.removed.len(), 1, "only the plugin: {pruned:?}");
        let s = Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &keep,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("login after prune");
        assert_eq!(s.delivery_report().delivered, 0);
        s.finish().await.expect("finish");

        // A crash leftover (dead marker) does not block (C-R4); an entry the user
        // cannot delete is reported as left (§FR-005).
        fx.host_exec(
            "echo 999999 > $HOME/.xxh/sessions/crashed && mkdir -p $HOME/.xxh/cache/ro \
             && touch $HOME/.xxh/cache/ro/f && chmod 500 $HOME/.xxh/cache/ro",
        )
        .await;
        let partial = remote_env::clean(&mut t, false).await.expect("clean");
        assert!(partial.refused.is_empty(), "dead markers never block");
        assert_eq!(partial.left.len(), 1, "{partial:?}");
        fx.host_exec("chmod 700 $HOME/.xxh/cache/ro").await;

        let done = remote_env::clean(&mut t, false).await.expect("clean");
        assert_eq!(done.removed.len(), 1, "{done:?}");
        assert!(done.left.is_empty());
        assert_eq!(remote_env::inspect(&mut t).await.expect("inspect"), vec![]);
        t.disconnect().await.unwrap();

        // The mandatory cleanliness assert (SC-001, Принцип VIII).
        assert_eq!(fx.cleanliness().await, "CLEAN");
    });
    let _ = std::fs::remove_dir_all(&plugin_dir);
}
