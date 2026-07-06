//! Integration test (T041, C-IT-S6, §SC-006): plugins installed from **git** and a
//! **local path** are applied in a real session; a plugin whose hook fails yields a
//! localized plugin-class error while the session keeps working; the host ends clean.
//!
//! Requires docker + git; skips (passes) when docker is absent.

mod common;

use std::path::{Path, PathBuf};

use common::{Fixture, docker_available, eff, run_ok};
use xxh_config::CleanupMode;
use xxh_core::session::{Session, SessionPlugin, minimal_env_component};
use xxh_plugins::registry::Registry;
use xxh_plugins::resolver;
use xxh_plugins::source::SourceSpec;
use xxh_transport::RusshTransport;

fn write_plugin(dir: &Path, name: &str, env_line: &str, failing_hook: bool) {
    std::fs::create_dir_all(dir).unwrap();
    let mut manifest = format!("name = \"{name}\"\nversion = \"1.0.0\"\napi_version = \"1.0.0\"\n");
    if failing_hook {
        manifest.push_str("[hooks.post_deploy]\nrun = \"hook.sh\"\n");
        std::fs::write(
            dir.join("hook.sh"),
            "echo deliberately-broken >&2; exit 1\n",
        )
        .unwrap();
    }
    std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
    std::fs::write(dir.join("env.sh"), format!("{env_line}\n")).unwrap();
}

fn scratch(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("xxh-pgl-{name}-{}-{nanos:x}", std::process::id()))
}

#[test]
fn git_and_local_plugins_apply_and_broken_hook_does_not_kill_session() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    let fx = Fixture::boot();

    // Registry in a scratch dir (respected via $XXH_PLUGINS_DIR by open_default,
    // but here we open it directly to avoid env races).
    let reg_root = scratch("registry");
    let registry = Registry::open(&reg_root);

    // Local-path plugin.
    let local_src = scratch("local-plugin");
    write_plugin(&local_src, "local-greet", "export PLUGIN_LOCAL=1", false);

    // Git plugin: a real local repository fetched through the GitProvider.
    let git_src = scratch("git-plugin");
    write_plugin(&git_src, "git-greet", "export PLUGIN_GIT=1", false);
    run_ok("git", &["-C", git_src.to_str().unwrap(), "init", "-q"]);
    run_ok("git", &["-C", git_src.to_str().unwrap(), "add", "."]);
    run_ok(
        "git",
        &[
            "-C",
            git_src.to_str().unwrap(),
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "init",
        ],
    );

    // Plugin whose post_deploy hook fails: must not take the session down (§FR-019).
    let broken_src = scratch("broken-plugin");
    write_plugin(&broken_src, "broken", "export PLUGIN_BROKEN=1", true);

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        registry
            .install(&SourceSpec::Local {
                path: local_src.clone(),
            })
            .await
            .expect("install local plugin");
        registry
            .install(&SourceSpec::parse(&format!("file://{}", git_src.display())).unwrap())
            .await
            .expect("install git plugin");
        registry
            .install(&SourceSpec::Local {
                path: broken_src.clone(),
            })
            .await
            .expect("install broken plugin");

        // Assemble session plugins in resolved order (as the CLI does, T039).
        let names = ["local-greet", "git-greet", "broken"];
        let mut plugins: Vec<SessionPlugin> = names
            .iter()
            .map(|n| SessionPlugin {
                manifest: registry.manifest(n).unwrap(),
                dir: registry.package_dir(n).unwrap(),
            })
            .collect();
        let manifests: Vec<_> = plugins.iter().map(|p| p.manifest.clone()).collect();
        let order = resolver::resolve(&manifests).expect("resolution");
        plugins.sort_by_key(|p| order.iter().position(|n| *n == p.manifest.name).unwrap());

        // The broken hook's failure must be reported as plugin-class, localized.
        let seen = std::sync::Mutex::new(Vec::<String>::new());
        let progress = |msg: &str| seen.lock().unwrap().push(msg.to_string());

        let env = vec![minimal_env_component("gz").unwrap()];
        let mut session = Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &eff("sh", CleanupMode::Ephemeral),
            &env,
            &plugins,
            &progress,
        )
        .await
        .expect("session must establish despite the broken hook");

        // Both plugins' env.sh were sourced in the remote environment (§FR-016/017).
        let code = session
            .run_command("test \"$PLUGIN_LOCAL\" = 1 && test \"$PLUGIN_GIT\" = 1")
            .await
            .expect("run in session");
        assert_eq!(
            code, 0,
            "git & local plugin env must be applied in the session"
        );
        session.finish().await.expect("finish");

        let messages = seen.lock().unwrap().join("\n");
        assert!(
            messages.contains("broken") && messages.contains("session continues"),
            "broken plugin must be reported as a localized plugin failure, got:\n{messages}"
        );

        // Mandatory cleanliness assert (Принцип VIII).
        assert_eq!(
            fx.cleanliness().await,
            "CLEAN",
            "~/.xxh must be gone after exit"
        );
    });

    for d in [&reg_root, &local_src, &git_src, &broken_src] {
        let _ = std::fs::remove_dir_all(d);
    }
}
