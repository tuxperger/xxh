//! Integration (008 T009/T012/T015): the **real `xxh` binary** manages a shell
//! package end to end against a running container — no network: the package's
//! build is a `file://` archive made by the test.
//!
//! - `xxh shell add --no-builds` → `list` shows the declared builds as not fetched;
//! - a login with that shell fails with a shell error naming
//!   `xxh shell fetch … --platform …` (§FR-007);
//! - `xxh shell fetch` → the login runs inside the delivered shell, with the
//!   package's overlay `env.sh` in effect (US1);
//! - a build whose checksum does not match is refused (exit 20) and never listed
//!   (§FR-005/§FR-006);
//! - `xxh shell remove` empties the list; the container ends clean and its image
//!   unchanged.
//!
//! Runtime-gated; skips when none is available.

mod common;

use std::path::Path;
use std::process::Command;

use common::{ContainerFixture, runtime_available, test_runtime};

const XXH: &str = env!("CARGO_BIN_EXE_xxh");

/// A tar.gz with `bin/<shell>` — a POSIX sh wrapper that marks itself — and its
/// SHA-256.
fn build_archive(path: &Path, shell: &str) -> String {
    let f = std::fs::File::create(path).unwrap();
    let gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    let mut b = tar::Builder::new(gz);
    let body = b"#!/bin/sh\nXSH_SHELL=1 exec /bin/sh \"$@\"\n";
    let mut h = tar::Header::new_gnu();
    h.set_mode(0o755);
    h.set_size(body.len() as u64);
    h.set_entry_type(tar::EntryType::Regular);
    b.append_data(&mut h, format!("bin/{shell}"), &body[..])
        .unwrap();
    b.into_inner().unwrap().finish().unwrap();
    let out = Command::new("sha256sum").arg(path).output().unwrap();
    String::from_utf8_lossy(&out.stdout)[..64].to_string()
}

fn package(dir: &Path, shell: &str, archive: &Path, sha: &str) {
    std::fs::create_dir_all(dir.join("overlay")).unwrap();
    std::fs::write(dir.join("overlay/env.sh"), "export XSH_MARK=overlay\n").unwrap();
    let mut text = format!(
        "name = \"{shell}\"\nversion = \"1.0.0\"\napi_version = \"1.1.0\"\n\
         [provides]\nshell = \"{shell}\"\n"
    );
    for p in ["linux-x86_64", "linux-aarch64"] {
        text.push_str(&format!(
            "[builds.{p}]\nurl = \"file://{}\"\nsha256 = \"{sha}\"\n",
            archive.display()
        ));
    }
    std::fs::write(dir.join("plugin.toml"), text).unwrap();
}

#[test]
fn shell_package_lifecycle() {
    let runtime = test_runtime();
    if !runtime_available(&runtime) {
        eprintln!("skipping: container runtime `{runtime}` not available");
        return;
    }
    let fx = ContainerFixture::boot();
    let target = format!("{runtime}:{}", fx.name);
    let scratch = std::env::temp_dir().join(format!("xxh-shellpkg-it-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
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

    let archive = scratch.join("xsh.tgz");
    let sha = build_archive(&archive, "xsh");
    let pkg = scratch.join("xsh-pkg");
    package(&pkg, "xsh", &archive, &sha);

    // Installed without builds: listed as not fetched.
    let (code, _, err) = xxh(&["shell", "add", pkg.to_str().unwrap(), "--no-builds"]);
    assert_eq!(code, Some(0), "stderr: {err}");
    let (_, list, _) = xxh(&["shell", "list"]);
    assert!(list.contains("xsh 1.0.0"), "{list}");
    assert!(list.contains("builds: none"), "{list}");
    assert!(
        list.contains("not fetched: linux-aarch64, linux-x86_64"),
        "{list}"
    );

    // No build for the target: the error names the command (§FR-007).
    let (code, _, err) = xxh(&["--shell", "xsh", &target, "--", "true"]);
    assert_eq!(code, Some(20), "stderr: {err}");
    assert!(
        err.contains("xxh shell fetch xsh --platform linux-"),
        "stderr: {err}"
    );
    assert_eq!(fx.cleanliness(), "CLEAN");

    // Fetch, then log in: the delivered shell runs, with its overlay env.sh.
    let (code, out, err) = xxh(&["shell", "fetch", "xsh"]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(out.contains("xsh: fetched linux-x86_64"), "{out}");
    let (code, out, _) = xxh(&["shell", "fetch", "xsh"]);
    assert_eq!(code, Some(0));
    assert!(out.contains("linux-x86_64 already present"), "{out}");
    let (code, out, err) = xxh(&[
        "--shell",
        "xsh",
        &target,
        "-c",
        "echo \"$XSH_SHELL $XSH_MARK\"",
    ]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert_eq!(out, "1 overlay\n", "the packaged shell ran the line");
    assert_eq!(fx.cleanliness(), "CLEAN");

    // A build whose checksum does not match is refused and never listed.
    let bad = scratch.join("ysh-pkg");
    let ysh_archive = scratch.join("ysh.tgz");
    build_archive(&ysh_archive, "ysh");
    package(&bad, "ysh", &ysh_archive, &"0".repeat(64));
    let (code, _, err) = xxh(&["shell", "add", bad.to_str().unwrap()]);
    assert_eq!(code, Some(20), "stderr: {err}");
    assert!(err.contains("integrity"), "stderr: {err}");
    let (_, list, _) = xxh(&["shell", "list"]);
    assert!(list.contains("ysh 1.0.0"), "{list}");
    let ysh = list.split("ysh 1.0.0").nth(1).unwrap();
    assert!(ysh.contains("builds: none"), "{list}");

    for s in ["xsh", "ysh"] {
        let (code, _, err) = xxh(&["shell", "remove", s]);
        assert_eq!(code, Some(0), "stderr: {err}");
    }
    let (_, list, _) = xxh(&["shell", "list"]);
    assert_eq!(list, "no shell packages installed\n");

    let _ = std::fs::remove_dir_all(&scratch);
    assert_eq!(fx.cleanliness(), "CLEAN");
    assert!(fx.diff_clean(), "{}", fx.diff());
    assert!(fx.image_digest_unchanged());
}
