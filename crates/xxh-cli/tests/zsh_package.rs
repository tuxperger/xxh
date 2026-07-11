//! Integration test (T020, §FR-008..011): the first-party zsh *package* is
//! resolved on the client, delivered to a host that has no zsh, and runs there;
//! the host ends clean. The package now lives in its own repo (`xxh-shell-zsh`):
//! point `$XXH_ZSH_PACKAGE_DIR` at a checkout with a fetched payload
//! (`./fetch.sh linux-x86_64`). Requires docker; skips (passes) otherwise.

mod common;

use common::{Fixture, docker_available, eff};
use xxh_config::CleanupMode;
use xxh_core::session::{Session, minimal_env_component, silent_progress};
use xxh_transport::RusshTransport;

#[test]
fn packaged_zsh_is_delivered_and_runs_on_host() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    let arch = std::process::Command::new("uname")
        .arg("-m")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if arch != "x86_64" {
        eprintln!("skipping: container arch is not x86_64");
        return;
    }
    let Some(pkg_dir) = std::env::var_os("XXH_ZSH_PACKAGE_DIR").map(std::path::PathBuf::from)
    else {
        eprintln!("skipping: XXH_ZSH_PACKAGE_DIR not set (point it at an xxh-shell-zsh checkout)");
        return;
    };
    if !pkg_dir.join("dist/linux-x86_64/bin/zsh").is_file() {
        eprintln!("skipping: zsh payload not fetched (run fetch.sh in $XXH_ZSH_PACKAGE_DIR)");
        return;
    }

    // shellpkg::find expects <search-dir>/zsh/manifest.toml, so expose the
    // checkout under a `zsh/` entry via a symlink in a scratch search dir.
    let shells_dir = std::env::temp_dir().join(format!("xxh-zsh-pkg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&shells_dir);
    std::fs::create_dir_all(&shells_dir).unwrap();
    std::os::unix::fs::symlink(&pkg_dir, shells_dir.join("zsh")).unwrap();

    // Resolve shells only from the package checkout (exclusive override).
    // SAFETY: single test in this binary, set before any resolution.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("XXH_SHELLS_DIR", &shells_dir);
    }

    let fx = Fixture::boot();
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(async {
        // The host must NOT have zsh — that is the point of the package.
        assert_eq!(
            fx.host_exec("command -v zsh >/dev/null 2>&1 && echo HAVE || echo NONE")
                .await,
            "NONE",
            "test image must not ship zsh"
        );

        let env = vec![minimal_env_component("gz").unwrap()];
        let mut s = Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &eff("zsh", CleanupMode::Ephemeral),
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("establish with packaged zsh");

        // Shell + env components were delivered.
        assert!(s.delivery_report().delivered >= 2, "shell + env expected");

        // The delivered zsh is on PATH (prelude) and actually runs on the host.
        let code = s
            .run_command("zsh -fc 'echo zsh-runs-$ZSH_VERSION' | grep -q '^zsh-runs-5'")
            .await
            .expect("run zsh");
        assert_eq!(code, 0, "delivered zsh must execute on the host");

        s.finish().await.expect("finish");
        assert_eq!(
            fx.cleanliness().await,
            "CLEAN",
            "ephemeral session must leave the host clean"
        );
    });
}
