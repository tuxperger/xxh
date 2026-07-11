//! Integration test (002 T021, US4): dual-transport parity (C-DT5, §SC-004). One
//! image, one config (shell + env) reached two ways — SSH into the container's
//! sshd and the container transport's `exec` into the SAME container — must
//! deliver the identical set of content-addressed components. The tool above the
//! transport does not know or care which family carried the bytes (Принцип III).
//!
//! Requires docker; skips (passes) when docker is absent.

mod common;

use std::process::Command;

use common::{Fixture, docker_available, eff};
use xxh_config::CleanupMode;
use xxh_core::session::{Session, minimal_env_component, silent_progress};
use xxh_transport::{ContainerCliTransport, RusshTransport};

/// The set of cache hashes present under `home`'s xxh root inside the container.
fn cache_set(name: &str, home: &str) -> Vec<String> {
    let out = Command::new("docker")
        .args([
            "exec",
            name,
            "sh",
            "-c",
            &format!("ls -1 {home}/.xxh/cache 2>/dev/null || true"),
        ])
        .output()
        .expect("docker exec ls cache");
    let mut v: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    v.sort();
    v
}

#[test]
fn ssh_and_container_transports_deliver_identical_components() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    let fx = Fixture::boot();
    let rt = tokio::runtime::Runtime::new().unwrap();

    let (ssh_report, ctr_report, ssh_cache, ctr_cache) = rt.block_on(async {
        // One config, one env component set — reused verbatim for both transports.
        let cfg = eff("sh", CleanupMode::Keep);
        let env = vec![minimal_env_component("gz").unwrap()];

        // Entry A: SSH (russh) as `tester` → cache lands under /home/tester.
        let mut ssh = Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &cfg,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("establish over ssh");
        let ssh_report = ssh.delivery_report();
        ssh.run_command("true").await.expect("run over ssh");
        ssh.finish().await.expect("finish ssh");
        let ssh_cache = cache_set(fx.name(), "/home/tester");

        // Entry B: container transport (exec) as root → cache under /root.
        let mut ctr = Session::establish(
            ContainerCliTransport::new(),
            &fx.container_target(),
            &cfg,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("establish over container");
        let ctr_report = ctr.delivery_report();
        ctr.run_command("true").await.expect("run over container");
        ctr.finish().await.expect("finish container");
        let ctr_cache = cache_set(fx.name(), "/root");

        (ssh_report, ctr_report, ssh_cache, ctr_cache)
    });

    // Same image + same config ⇒ same components delivered by either transport.
    assert!(!ssh_cache.is_empty(), "ssh entry must deliver components");
    assert_eq!(
        ssh_cache, ctr_cache,
        "both transports must deliver the identical set of content-addressed \
         components (ssh={ssh_cache:?}, container={ctr_cache:?})"
    );
    assert_eq!(
        ssh_report.delivered, ctr_report.delivered,
        "both transports must deliver the same number of components with a clean cache"
    );
}
