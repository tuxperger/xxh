//! Integration test (002 T027, US-cross-cutting): container transport error classes
//! are distinguishable and never leave a partially-deployed environment (C-DT4,
//! FR-008). Covers: container not found, container stopped, and runtime not
//! installed (a stripped PATH via the real `xxh` binary).
//!
//! Requires docker for the not-found/stopped cases; the not-installed case needs
//! no runtime. Skips (passes) when docker is absent.

mod common;

use std::process::Command;

use common::{runtime_available, test_runtime};
use xxh_transport::{
    AuthPolicy, ContainerCliTransport, ContainerRuntime, ContainerTarget, ResolvedTarget,
    RuntimeSelector, Transport, TransportError,
};

fn container_target(reference: &str) -> ResolvedTarget {
    ResolvedTarget::Container(ContainerTarget {
        reference: reference.into(),
        runtime: RuntimeSelector::Explicit(ContainerRuntime::Docker),
        exec_user: None,
        connect_timeout_s: 10,
    })
}

#[test]
fn missing_container_is_a_distinct_connect_error() {
    if !runtime_available("docker") {
        eprintln!("skipping: docker not available");
        return;
    }
    let name = format!("xxh-absent-{}", std::process::id());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt.block_on(async {
        let mut t = ContainerCliTransport::new();
        t.connect(&container_target(&name), &AuthPolicy::default())
            .await
    });
    match err {
        Err(TransportError::Connect(msg)) => {
            assert!(
                msg.contains("not found"),
                "message must say not found: {msg}"
            );
            assert!(
                msg.contains(&name),
                "message must name the reference: {msg}"
            );
        }
        other => panic!("expected a Connect error for a missing container, got {other:?}"),
    }
}

#[test]
fn stopped_container_is_distinct_and_leaves_no_footprint() {
    if !runtime_available("docker") {
        eprintln!("skipping: docker not available");
        return;
    }
    // A container that exists but is not running: `true` exits immediately, and
    // without `--rm` it stays in the `exited` state.
    let name = format!("xxh-stopped-{}", std::process::id());
    let _ = Command::new("docker")
        .args(["run", "-d", "--name", &name, "alpine:3.20", "true"])
        .output()
        .expect("create stopped container");
    // Ensure it has exited before we probe it.
    let _ = Command::new("docker").args(["wait", &name]).output();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt.block_on(async {
        let mut t = ContainerCliTransport::new();
        t.connect(&container_target(&name), &AuthPolicy::default())
            .await
    });
    // No delivery may have happened: the container fs must be untouched by xxh.
    let diff = String::from_utf8_lossy(
        &Command::new("docker")
            .args(["diff", &name])
            .output()
            .expect("docker diff")
            .stdout,
    )
    .to_string();
    let _ = Command::new("docker").args(["rm", "-f", &name]).output();

    match err {
        Err(TransportError::Connect(msg)) => {
            assert!(msg.contains("stopped"), "message must say stopped: {msg}");
        }
        other => panic!("expected a Connect error for a stopped container, got {other:?}"),
    }
    assert!(
        !diff.contains(".xxh"),
        "a failed connect must leave no xxh artefacts; diff:\n{diff}"
    );
}

#[test]
fn runtime_not_installed_is_a_transport_error() {
    // No runtime binary on PATH: the container backend fails fast with the
    // transport exit code (10) and a distinct "not installed" message, before any
    // connection is attempted. Needs no live daemon.
    let out = Command::new(env!("CARGO_BIN_EXE_xxh"))
        .args(["docker:anything"])
        .env("PATH", "/nonexistent-xxh-path")
        .output()
        .expect("run xxh with an empty PATH");

    assert_eq!(
        out.status.code(),
        Some(10),
        "a missing runtime must map to the transport exit code (10)"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not installed"),
        "the error must distinguish 'runtime not installed': {stderr}"
    );

    // Keep the runtime name referenced so the intent is clear even if docker is
    // absent on this machine (this case does not depend on it).
    let _ = test_runtime();
}
