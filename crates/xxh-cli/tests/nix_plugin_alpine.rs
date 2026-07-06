//! ⭐ Integration test (T053, C-IT-S7, §SC-010/012, feature `nix-source`): a static
//! nixpkgs-built tool is delivered to and runs on an Alpine host that has neither
//! Nix nor root; the host ends clean. Requires docker + nix on the client;
//! skips (passes) when either is absent.

#![cfg(feature = "nix-source")]

mod common;

use common::{Fixture, docker_available, eff};
use xxh_config::CleanupMode;
use xxh_core::session::{Session, SessionPlugin, minimal_env_component, silent_progress};
use xxh_plugins::registry::Registry;
use xxh_plugins::source::{Availability, PackageSource, SourceSpec};
use xxh_plugins::sources::nix::NixProvider;
use xxh_transport::RusshTransport;

#[test]
fn nix_static_ripgrep_runs_on_alpine_without_nix() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    if let Availability::Unavailable { reason } = NixProvider::new().availability() {
        // Degradation path (§FR-040, §SC-012): no Nix on the client only disables
        // this provider; the rest of the suite covers the base tool.
        eprintln!("skipping: nix unavailable on client ({reason})");
        return;
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let scratch = std::env::temp_dir().join(format!("xxh-nixit-{}-{nanos:x}", std::process::id()));
    let reg_root = scratch.join("registry");
    let nix_cache = scratch.join("nix-cache");
    std::fs::create_dir_all(&nix_cache).unwrap();
    // SAFETY: single-test binary; set before any provider use.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("XXH_NIX_CACHE_DIR", &nix_cache);
    }

    let fx = Fixture::boot();
    let registry = Registry::open(&reg_root);
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let m = registry
            .install(&SourceSpec::parse("nixpkgs:ripgrep").unwrap())
            .await
            .expect("nix build + install of ripgrep");

        let plugins = vec![SessionPlugin {
            manifest: registry.manifest(&m.name).unwrap(),
            dir: registry.package_dir(&m.name).unwrap(),
        }];
        let env = vec![minimal_env_component("gz").unwrap()];
        let mut session = Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &eff("sh", CleanupMode::Ephemeral),
            &env,
            &plugins,
            silent_progress(),
        )
        .await
        .expect("establish with nix plugin");

        // The static rg must be on PATH inside the session — on a musl/BusyBox
        // host with no Nix and no root (§FR-034/035).
        let code = session
            .run_command("rg --version >/dev/null")
            .await
            .expect("run rg");
        assert_eq!(code, 0, "static ripgrep must run on the Alpine host");
        session.finish().await.expect("finish");

        // Mandatory cleanliness assert (Принцип VIII).
        assert_eq!(fx.cleanliness().await, "CLEAN", "~/.xxh must be gone");
    });

    let _ = std::fs::remove_dir_all(&scratch);
}
