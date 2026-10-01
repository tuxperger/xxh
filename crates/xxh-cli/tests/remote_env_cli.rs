//! Integration (005 T016): the **real `xxh` binary** — `xxh status` and
//! `xxh clean` against a running container: exit codes, the "nothing there"
//! message, JSON on stdout, and a clean container at the end (C-S1..C-S3,
//! C-C1). The client side is isolated from the developer's config, plugins and
//! caches. Runtime-gated; skips when none is available.

mod common;

use std::process::Command;

use common::{ContainerFixture, runtime_available, test_runtime};

const XXH: &str = env!("CARGO_BIN_EXE_xxh");

#[test]
fn status_and_clean_commands() {
    let runtime = test_runtime();
    if !runtime_available(&runtime) {
        eprintln!("skipping: container runtime `{runtime}` not available");
        return;
    }
    let fx = ContainerFixture::boot();
    let target = format!("{runtime}:{}", fx.name);
    let scratch = std::env::temp_dir().join(format!("xxh-renv-cli-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let xxh = |args: &[&str]| {
        let out = Command::new(XXH)
            // Before the arguments: after `--` it would belong to the command.
            .args(["--shell", "sh"])
            .args(args)
            .env("XDG_CONFIG_HOME", scratch.join("config"))
            .env("XXH_PLUGINS_DIR", scratch.join("plugins"))
            .env("XXH_SHELLS_DIR", scratch.join("shells"))
            .env("XXH_PACK_CACHE_DIR", scratch.join("packed"))
            .output()
            .expect("run xxh");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };

    // Fresh container: nothing there, and nothing written by looking (§FR-012).
    let (code, out, err) = xxh(&["status", &target]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(
        out.starts_with(&format!("nothing from xxh on {}", fx.name)),
        "{out}"
    );
    assert_eq!(fx.cleanliness(), "CLEAN");
    assert!(fx.diff_clean(), "status must not write:\n{}", fx.diff());

    // Kept environment → JSON with one kept root whose components are current.
    let (code, _, err) = xxh(&[&target, "--keep", "--", "true"]);
    assert_eq!(code, Some(0), "stderr: {err}");
    let (code, out, err) = xxh(&["status", &target, "--json"]);
    assert_eq!(code, Some(0), "stderr: {err}");
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert_eq!(v["target"], fx.name.as_str());
    assert_eq!(v["envs"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["envs"][0]["kept"], true);
    let comps = v["envs"][0]["components"].as_array().unwrap();
    assert!(!comps.is_empty());
    assert!(comps.iter().all(|c| c["state"] == "current"), "{v}");
    assert_eq!(v["plan"]["deliver"].as_array().unwrap().len(), 0, "{v}");

    // Clean: removed, then nothing left to clean.
    let (code, out, err) = xxh(&["clean", &target]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(out.contains("removed ") && out.contains("freed "), "{out}");
    let (code, out, _) = xxh(&["clean", &target]);
    assert_eq!(code, Some(0));
    assert!(out.starts_with("nothing to clean"), "{out}");

    // An unreachable target is a transport error, as for a login.
    let (code, _, err) = xxh(&["status", &format!("{runtime}:xxh-no-such-container")]);
    assert_eq!(code, Some(10), "stderr: {err}");

    let _ = std::fs::remove_dir_all(&scratch);
    assert_eq!(fx.cleanliness(), "CLEAN");
    assert!(fx.diff_clean(), "{}", fx.diff());
    assert!(fx.image_digest_unchanged());
}
