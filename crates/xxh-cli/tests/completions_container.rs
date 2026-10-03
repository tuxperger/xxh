//! Integration (007 T011): completion of container targets against a **real
//! runtime** — a running container is offered, a stopped one is not, and asking
//! leaves the container untouched (C-K8/C-K9, Принципы I и VIII).
//! Runtime-gated; skips when none is available.

mod common;

use std::path::Path;
use std::process::Command;

use common::{ContainerFixture, run_ok, runtime_available, test_runtime};

const XXH: &str = env!("CARGO_BIN_EXE_xxh");

/// The candidates for `word` as a target; the client is isolated from the
/// developer's config but keeps `PATH`, where the runtime is.
fn complete(scratch: &Path, word: &str) -> Vec<String> {
    let out = Command::new(XXH)
        .args(["__complete", "bash", "1", "--", "xxh", word])
        .env("HOME", scratch)
        .env("XDG_CONFIG_HOME", scratch.join("config"))
        .env("XXH_PLUGINS_DIR", scratch.join("plugins"))
        .env("XXH_SHELLS_DIR", scratch.join("shells"))
        .output()
        .expect("run xxh");
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stderr.is_empty(), "completion is silent (C-K5)");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

/// Completion gives up on a slow runtime by design (C-K9); on a loaded machine
/// the first answers may be empty, so ask a few times before judging.
fn complete_until(scratch: &Path, word: &str, want: &str) -> Vec<String> {
    let mut got = Vec::new();
    for _ in 0..10 {
        got = complete(scratch, word);
        if got.iter().any(|c| c == want) {
            break;
        }
    }
    got
}

#[test]
fn running_containers_are_offered() {
    let runtime = test_runtime();
    if !runtime_available(&runtime) {
        eprintln!("skipping: container runtime `{runtime}` not available");
        return;
    }
    let fx = ContainerFixture::boot();
    let other = ContainerFixture::boot();
    let scratch = std::env::temp_dir().join(format!("xxh-compl-ctr-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();

    let ours = format!("{runtime}:{}", fx.name);
    let theirs = format!("{runtime}:{}", other.name);

    // Both running containers, addressed with the scheme that was typed (C-K8).
    let got = complete_until(&scratch, &format!("{runtime}:"), &theirs);
    assert!(got.contains(&ours), "{ours} in {got:?}");
    assert!(got.contains(&theirs), "{theirs} in {got:?}");
    // The typed part narrows the list down.
    let got = complete_until(&scratch, &ours, &ours);
    assert_eq!(got, [ours.clone()]);
    // `container:` resolves the runtime the way a login would.
    if runtime == "docker" {
        let auto = format!("container:{}", fx.name);
        let got = complete_until(&scratch, "container:", &auto);
        assert!(got.contains(&auto), "{auto} in {got:?}");
    }

    // A container that no longer runs is not a target.
    run_ok(&runtime, &["stop", "-t", "0", &other.name]);
    let got = complete_until(&scratch, &format!("{runtime}:"), &ours);
    assert!(got.contains(&ours), "{got:?}");
    assert!(!got.contains(&theirs), "{theirs} still in {got:?}");

    // Asking executed nothing in the container and changed nothing (Принцип I).
    assert_eq!(fx.cleanliness(), "CLEAN");
    assert!(fx.diff_clean(), "completion must not write:\n{}", fx.diff());
    assert!(fx.image_digest_unchanged());

    std::fs::remove_dir_all(&scratch).unwrap();
}
