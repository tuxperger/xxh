//! ⭐ Integration test (003 T014, US1, §SC-001..003, feature `nix-source`): a
//! program taken from a flake output is delivered to — and runs on — a host that
//! has neither Nix nor root, and the host ends clean. Runs against the image
//! selected by `XXH_TEST_IMAGE` (alpine by default: musl + BusyBox). Requires
//! docker + nix on the client; skips (passes) when either is absent.

#![cfg(feature = "nix-source")]

mod common;

use common::{Fixture, docker_available, eff, make_test_flake, nix_available};
use xxh_config::CleanupMode;
use xxh_core::session::{Session, SessionPlugin, minimal_env_component, silent_progress};
use xxh_plugins::registry::Registry;
use xxh_plugins::source::SourceSpec;
use xxh_transport::RusshTransport;

#[test]
fn flake_program_runs_on_a_host_without_nix() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    if !nix_available() {
        // Degradation path (003 §FR-020): no Nix on the client only disables this
        // source; the rest of the suite covers the base tool.
        eprintln!("skipping: nix with flakes not available on client");
        return;
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let scratch =
        std::env::temp_dir().join(format!("xxh-flakeit-{}-{nanos:x}", std::process::id()));
    let flake_dir = scratch.join("flake");
    std::fs::create_dir_all(scratch.join("nix-cache")).unwrap();
    // SAFETY: single-test binary; set before any provider use.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("XXH_NIX_CACHE_DIR", scratch.join("nix-cache"));
    }
    make_test_flake(&flake_dir);

    let fx = Fixture::boot();
    let registry = Registry::open(scratch.join("registry"));
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let spec = SourceSpec::parse(&format!("flake:{}#tool", flake_dir.display())).unwrap();
        let m = registry
            .install(&spec)
            .await
            .expect("build + install of the flake program");
        // Wrapped program: named after the output, pinned to the commit (§FR-008/014).
        assert_eq!(m.name, "tool");
        assert!(matches!(
            registry.entry("tool").unwrap().source,
            SourceSpec::Flake {
                revision: Some(_),
                ..
            }
        ));

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
        .expect("establish with flake plugin");

        // One command, because an ephemeral environment is torn down as soon as a
        // command exits: the static program runs from PATH on a host with no Nix
        // and no root (§FR-013), and the runtime data it needs resolves inside the
        // delivered package, never into /nix/store (§FR-011).
        let code = session
            .run_command(
                "sh -c 'hello >/dev/null && [ ! -e /nix ] \
                 && [ -f \"$SSL_CERT_FILE\" ] && [ -z \"${TERMINFO:-}\" ] \
                 && [ -d \"${TERMINFO_DIRS##*:}\" ]'",
            )
            .await
            .expect("run the flake program");
        assert_eq!(
            code, 0,
            "the flake program must run on the host with package-local runtime data"
        );
        session.finish().await.expect("finish");

        // Mandatory cleanliness assert (Принцип VIII).
        assert_eq!(fx.cleanliness().await, "CLEAN", "~/.xxh must be gone");
    });

    let _ = std::fs::remove_dir_all(&scratch);
}
