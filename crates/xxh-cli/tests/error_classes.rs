//! Integration checks for distinguishable error classes (T045, §FR-026/031, §SC-005).
//!
//! - Unreachable host → the real `xxh` binary exits with the transport code (10)
//!   within the configured timeout and creates no artefacts.
//! - Missing shell on a live host → a shell-class error and an untouched host
//!   (docker-gated; skips when docker is absent).

mod common;

use std::process::Command;
use std::time::{Duration, Instant};

use common::{Fixture, docker_available, eff};
use xxh_config::CleanupMode;
use xxh_core::ShellError;
use xxh_core::session::{Session, SessionError, silent_progress};
use xxh_transport::RusshTransport;

#[test]
fn unreachable_host_exits_10_within_timeout() {
    // 192.0.2.1 (TEST-NET-1) is guaranteed unroutable — connect must time out.
    let started = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_xxh"))
        .args(["192.0.2.1", "--connect-timeout", "3"])
        .output()
        .expect("run xxh");
    let elapsed = started.elapsed();

    assert_eq!(
        out.status.code(),
        Some(10),
        "transport failures must exit 10 (§FR-026); stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        elapsed < Duration::from_secs(15),
        "connect must respect the timeout (§FR-031), took {elapsed:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("transport"),
        "error must be rendered with its class (T043), got:\n{stderr}"
    );
}

#[test]
fn missing_shell_is_a_shell_class_error_and_host_stays_clean() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    let fx = Fixture::boot();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let err = match Session::establish(
            RusshTransport::new(),
            &fx.target(),
            &eff("nosuchshell", CleanupMode::Ephemeral),
            &[],
            &[],
            silent_progress(),
        )
        .await
        {
            Err(e) => e,
            Ok(_) => panic!("nosuchshell must not establish"),
        };
        assert!(
            matches!(err, SessionError::Shell(ShellError::NotAvailable(_))),
            "missing shell must be shell-class (exit 20, §FR-011/026): {err}"
        );
        // No partial deployment: the host must be untouched (§FR-011).
        assert_eq!(
            fx.cleanliness().await,
            "CLEAN",
            "no artefacts on a missing shell"
        );
    });
}
