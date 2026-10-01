//! Integration (006 T011/T014): the **real `xxh` binary** — `xxh doctor` against
//! a running container and on the client alone. Exit codes 0/1/10, the JSON
//! shape (C-D5), and a container left clean with its image unchanged (C-D3).
//! The client is isolated from the developer's config, plugins and shells.
//! Runtime-gated; skips when none is available.

mod common;

use std::path::Path;
use std::process::Command;

use common::{ContainerFixture, runtime_available, test_runtime};

const XXH: &str = env!("CARGO_BIN_EXE_xxh");

fn xxh(scratch: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(XXH)
        .args(args)
        .env("XDG_CONFIG_HOME", scratch.join("config"))
        .env("XXH_PLUGINS_DIR", scratch.join("plugins"))
        .env("XXH_SHELLS_DIR", scratch.join("shells"))
        .output()
        .expect("run xxh");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn doctor_command() {
    let runtime = test_runtime();
    if !runtime_available(&runtime) {
        eprintln!("skipping: container runtime `{runtime}` not available");
        return;
    }
    let fx = ContainerFixture::boot();
    let target = format!("{runtime}:{}", fx.name);
    let scratch = std::env::temp_dir().join(format!("xxh-doctor-cli-{}", std::process::id()));
    std::fs::create_dir_all(scratch.join("config/xxh")).unwrap();

    // A sound setup and a fit target: exit 0, nothing written (US1, SC-002).
    let (code, out, err) = xxh(&scratch, &["doctor", &target, "--shell", "sh"]);
    assert_eq!(code, Some(0), "stdout: {out}\nstderr: {err}");
    assert!(
        out.contains(&format!("target {} (linux/", fx.name)),
        "{out}"
    );
    assert!(out.contains(", 0 failed"), "{out}");
    assert_eq!(fx.cleanliness(), "CLEAN");
    assert!(fx.diff_clean(), "doctor must not write:\n{}", fx.diff());

    // JSON (C-D5).
    let (code, out, _) = xxh(&scratch, &["doctor", &target, "--shell", "sh", "--json"]);
    assert_eq!(code, Some(0));
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert_eq!(v["client"][0]["id"], "config");
    assert_eq!(v["target"]["label"], fx.name.as_str());
    assert!(
        v["target"]["platform"]
            .as_str()
            .unwrap()
            .starts_with("linux/")
    );
    for c in v["target"]["checks"].as_array().unwrap() {
        assert!(c["id"].is_string() && c["message"].is_string(), "{c}");
        assert!(["pass", "warn", "fail"].contains(&c["status"].as_str().unwrap()));
    }

    // Unreachable: exit 10, the client half is still there (C-D6).
    let gone = format!("{runtime}:xxh-no-such-container");
    let (code, out, _) = xxh(&scratch, &["doctor", &gone, "--json"]);
    assert_eq!(code, Some(10));
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert!(!v["client"].as_array().unwrap().is_empty());
    assert_eq!(v["target"]["checks"][0]["id"], "connect");
    assert_eq!(v["target"]["checks"][0]["status"], "fail");

    // US2: an enabled plugin that is not installed — exit 1, named (T014).
    std::fs::write(
        scratch.join("config/xxh/config.toml"),
        "default_shell = \"sh\"\nenabled_plugins = [\"ghost\"]\n",
    )
    .unwrap();
    let (code, out, err) = xxh(&scratch, &["doctor"]);
    assert_eq!(code, Some(1), "stdout: {out}\nstderr: {err}");
    assert!(
        out.contains("FAIL  plugin ghost is enabled but not installed"),
        "{out}"
    );
    assert!(out.contains("xxh plugin disable ghost"), "{out}");
    assert!(!out.contains("\ntarget "), "no target, no target section");

    let _ = std::fs::remove_dir_all(&scratch);
    assert_eq!(fx.cleanliness(), "CLEAN");
    assert!(fx.image_digest_unchanged());
}
