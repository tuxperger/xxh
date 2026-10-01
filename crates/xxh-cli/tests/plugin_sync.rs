//! Integration (013 T009/T011): the **real `xxh` binary** reproduces declared
//! plugins and a shell from config + lock file on a clean client, and the result
//! works in a running container. No network: a `file://` git repository, a local
//! plugin and a shell package with a `file://` build.
//!
//! - `xxh sync` installs everything and writes the lock; again — all unchanged;
//! - a new commit upstream is not picked up while the lock pins the old one;
//! - `xxh plugin update` moves the plugin and its lock entry and says so;
//!   putting the old lock back and syncing restores the old revision (US2);
//! - a lock hash that does not match is refused with exit 30 (§FR-004);
//! - the container ends clean, its image unchanged.
//!
//! Runtime-gated; skips when none is available (and without git).

mod common;

use std::path::Path;
use std::process::Command;

use common::{ContainerFixture, runtime_available, test_runtime};

const XXH: &str = env!("CARGO_BIN_EXE_xxh");

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Commit a version of the git plugin `gp` whose env.sh exports `GP`.
fn commit_gp(repo: &Path, value: &str) -> String {
    std::fs::write(
        repo.join("plugin.toml"),
        "name = \"gp\"\nversion = \"1.0.0\"\napi_version = \"1.0.0\"\n",
    )
    .unwrap();
    std::fs::write(repo.join("env.sh"), format!("export GP={value}\n")).unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", value]);
    git(repo, &["rev-parse", "HEAD"])
}

/// A shell package `xsh` (a marked /bin/sh) with a `file://` build for both
/// Linux architectures.
fn shell_package(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let archive = dir.with_extension("tgz");
    {
        let f = std::fs::File::create(&archive).unwrap();
        let gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        let mut b = tar::Builder::new(gz);
        let body = b"#!/bin/sh\nXSH_SHELL=1 exec /bin/sh \"$@\"\n";
        let mut h = tar::Header::new_gnu();
        h.set_mode(0o755);
        h.set_size(body.len() as u64);
        h.set_entry_type(tar::EntryType::Regular);
        b.append_data(&mut h, "bin/xsh", &body[..]).unwrap();
        b.into_inner().unwrap().finish().unwrap();
    }
    let sha = Command::new("sha256sum").arg(&archive).output().unwrap();
    let sha = String::from_utf8_lossy(&sha.stdout)[..64].to_string();
    let mut text = "name = \"xsh\"\nversion = \"1.0.0\"\napi_version = \"1.1.0\"\n\
                    [provides]\nshell = \"xsh\"\n"
        .to_string();
    for p in ["linux-x86_64", "linux-aarch64"] {
        text.push_str(&format!(
            "[builds.{p}]\nurl = \"file://{}\"\nsha256 = \"{sha}\"\n",
            archive.display()
        ));
    }
    std::fs::write(dir.join("plugin.toml"), text).unwrap();
}

#[test]
fn declared_plugins_sync_and_pin() {
    let runtime = test_runtime();
    if !runtime_available(&runtime) {
        eprintln!("skipping: container runtime `{runtime}` not available");
        return;
    }
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("skipping: git not available");
        return;
    }
    let fx = ContainerFixture::boot();
    let target = format!("{runtime}:{}", fx.name);
    let scratch = std::env::temp_dir().join(format!("xxh-sync-it-{}", std::process::id()));
    let cfg_dir = scratch.join("config/xxh");
    std::fs::create_dir_all(&cfg_dir).unwrap();

    let repo = scratch.join("gp");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    let c1 = commit_gp(&repo, "v1");
    let lp = scratch.join("lp");
    std::fs::create_dir_all(&lp).unwrap();
    std::fs::write(
        lp.join("plugin.toml"),
        "name = \"lp\"\nversion = \"0.1.0\"\napi_version = \"1.0.0\"\n",
    )
    .unwrap();
    std::fs::write(lp.join("env.sh"), "export LP=yes\n").unwrap();
    let xsh = scratch.join("xsh");
    shell_package(&xsh);

    std::fs::write(
        cfg_dir.join("config.toml"),
        format!(
            "default_shell = \"xsh\"\nenabled_plugins = [\"gp\", \"lp\"]\n\
             [plugins.gp]\nsource = \"file://{}\"\n\
             [plugins.lp]\nsource = \"{}\"\n\
             [shells.xsh]\nsource = \"{}\"\n",
            repo.display(),
            lp.display(),
            xsh.display()
        ),
    )
    .unwrap();
    let lock = cfg_dir.join("xxh.lock");
    let xxh = |args: &[&str]| {
        let out = Command::new(XXH)
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
    let in_container = |line: &str| {
        let (code, out, err) = xxh(&[&target, "-c", line]);
        assert_eq!(code, Some(0), "stdout: {out}\nstderr: {err}");
        out
    };

    // A clean client: everything installed, the lock written.
    let (code, out, err) = xxh(&["sync"]);
    assert_eq!(code, Some(0), "stdout: {out}\nstderr: {err}");
    assert!(out.contains("plugin gp: installed"), "{out}");
    assert!(out.contains("plugin lp: installed"), "{out}");
    assert!(out.contains("shell xsh: installed"), "{out}");
    let text = std::fs::read_to_string(&lock).unwrap();
    assert!(text.contains(&format!("revision = \"{c1}\"")), "{text}");

    // Again: nothing to do.
    let (code, out, _) = xxh(&["sync"]);
    assert_eq!(code, Some(0));
    assert_eq!(out.matches("unchanged").count(), 3, "{out}");

    // Upstream moves on; the lock keeps the old commit.
    let c2 = commit_gp(&repo, "v2");
    let (_, out, _) = xxh(&["sync"]);
    assert!(out.contains("plugin gp: unchanged"), "{out}");
    assert_eq!(in_container("echo \"$GP $LP $XSH_SHELL\""), "v1 yes 1\n");

    // An explicit update moves plugin and lock, and says from what to what.
    let old_lock = std::fs::read_to_string(&lock).unwrap();
    let (code, out, err) = xxh(&["plugin", "update", "gp"]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(
        out.contains(&format!(
            "gp: 1.0.0 ({}) → 1.0.0 ({})",
            &c1[..12],
            &c2[..12]
        )),
        "{out}"
    );
    assert!(std::fs::read_to_string(&lock).unwrap().contains(&c2));
    assert_eq!(in_container("echo \"$GP\""), "v2\n");

    // The old lock back + sync: the old revision again.
    std::fs::write(&lock, &old_lock).unwrap();
    let (code, out, _) = xxh(&["sync"]);
    assert_eq!(code, Some(0));
    assert!(out.contains("plugin gp: updated"), "{out}");
    assert_eq!(in_container("echo \"$GP\""), "v1\n");

    // A clean registry, the local plugin changed since it was locked: the lock
    // refuses it (exit 30), everything else comes back as locked (§FR-004).
    std::fs::remove_dir_all(scratch.join("plugins")).unwrap();
    std::fs::write(lp.join("env.sh"), "export LP=changed\n").unwrap();
    let (code, out, _) = xxh(&["sync"]);
    assert_eq!(code, Some(30), "{out}");
    assert!(out.contains("plugin lp: failed"), "{out}");
    assert!(out.contains("does not match the lock file"), "{out}");
    assert!(out.contains("plugin gp: installed"), "{out}");

    let _ = std::fs::remove_dir_all(&scratch);
    assert_eq!(fx.cleanliness(), "CLEAN");
    assert!(fx.image_digest_unchanged());
}
