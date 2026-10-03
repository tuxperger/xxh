//! Integration (011 T007, US1): the **real `xxh` binary** sets session variables
//! from `-e/--env` inside a running container over the container transport —
//! byte for byte, with a terminal too, and winning over what a component sets
//! (C-E6: a declared `.gitconfig` exports `GIT_CONFIG_GLOBAL`, `-e` beats it).
//! The container is clean afterwards and its image unchanged (Принцип I, VIII).
//! Runtime-gated; skips when none is available.

mod common;

use std::process::Command;

use common::{ContainerFixture, runtime_available, test_runtime};

const XXH: &str = env!("CARGO_BIN_EXE_xxh");

const VALUE: &str = "s3cr3t 'single' \"double\" $HOME `id` \\ * ;\nsecond line\n";

#[test]
fn session_variables_reach_a_container_and_leave_nothing_behind() {
    let runtime = test_runtime();
    if !runtime_available(&runtime) {
        eprintln!("skipping: container runtime `{runtime}` not available");
        return;
    }
    let fx = ContainerFixture::boot();
    let target = format!("{runtime}:{}", fx.name);
    let scratch = std::env::temp_dir().join(format!("xxh-env-ctr-{}", std::process::id()));
    let home = scratch.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("gitconfig"), "[user]\n\tname = x\n").unwrap();
    let cfg = scratch.join("config/xxh/config.toml");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    std::fs::write(&cfg, "[files]\n\".gitconfig\" = \"~/gitconfig\"\n").unwrap();

    let xxh = |args: &[&str]| {
        let out = Command::new(XXH)
            // Before the arguments: after `--` it would belong to the command.
            .args(["--shell", "sh"])
            .args(args)
            .env("HOME", &home)
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
    let secret = format!("SECRET={VALUE}");

    // ── Exact values over the container transport (§FR-004, SC-001) ────────
    let (code, out, err) = xxh(&[
        &target,
        "-e",
        &secret,
        "--",
        "sh",
        "-c",
        "printf '[%s][%s]' \"$SECRET\" \"$GIT_CONFIG_GLOBAL\"",
    ]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(out.starts_with(&format!("[{VALUE}][")), "{out}");
    assert!(
        out.contains("/.xxh/cache/"),
        "the file's variable without -e: {out}"
    );
    assert!(!err.contains("s3cr3t"), "{err}");

    // ── The user's variable wins over a component's (C-E6) ─────────────────
    let (code, out, err) = xxh(&[
        &target,
        "-e",
        "GIT_CONFIG_GLOBAL=/mine",
        "--",
        "printenv",
        "GIT_CONFIG_GLOBAL",
    ]);
    assert_eq!((code, out.as_str()), (Some(0), "/mine\n"), "stderr: {err}");

    // ── With a terminal, as an interactive session gets it (§FR-009) ───────
    let (code, out, err) = xxh(&[
        &target,
        "-t",
        "-e",
        "EDITOR=vi-in-a-tty",
        "--",
        "printenv",
        "EDITOR",
    ]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(out.contains("vi-in-a-tty"), "{out}");

    // ── Clean; image unchanged (C-DT3) ──────────────────────────────────────
    assert_eq!(fx.cleanliness(), "CLEAN", "~/.xxh must be gone");
    assert!(fx.diff_clean(), "{}", fx.diff());
    assert!(fx.image_digest_unchanged(), "the image must not change");

    let _ = std::fs::remove_dir_all(&scratch);
}
