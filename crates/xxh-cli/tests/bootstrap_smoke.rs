//! Integration test (T014 harness + Principle VIII cleanliness gate).
//!
//! Boots a REAL minimal Alpine (musl + BusyBox) sshd container, connects over SSH
//! with a generated key, drives `bootstrap.sh`, and asserts the host is left clean
//! after an ephemeral session. This is the critical musl/BusyBox case (C-IT4) and
//! the "session opened WITHOUT a cleanliness assert = not passed" rule (C-IT7).
//!
//! The harness uses the docker CLI directly with RAII teardown; migrating to
//! testcontainers-rs is a tracked follow-up (same contract: real sshd, key auth,
//! fixed host key, guaranteed teardown).
//!
//! Requires a docker daemon + network (apk). Skips (passes) if docker is absent so
//! the suite stays green on machines without it; CI runs it for real.

use std::path::{Path, PathBuf};
use std::process::Command;

fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn repo_root() -> PathBuf {
    // crates/xxh-cli -> repo root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// A running Alpine sshd container with key auth. Torn down on drop (guaranteed
/// teardown even if the test panics — C-IT12).
struct Host {
    name: String,
    port: u16,
    workdir: PathBuf,
    key_path: PathBuf,
    known_hosts: PathBuf,
}

impl Host {
    fn boot() -> Host {
        let tag = "xxh-test-alpine:latest";
        let name = format!("xxh-it-{}", std::process::id());
        let workdir = std::env::temp_dir().join(&name);
        std::fs::create_dir_all(&workdir).unwrap();

        // Generate client key + deterministic host key (C-IT3).
        let key_path = workdir.join("client");
        run_ok("ssh-keygen", &["-t", "ed25519", "-N", "", "-q", "-f", key_path.to_str().unwrap()]);
        let hostkey = workdir.join("ssh_host_ed25519_key");
        run_ok("ssh-keygen", &["-t", "ed25519", "-N", "", "-q", "-f", hostkey.to_str().unwrap()]);

        // Build the minimal image with the generated testkey context.
        let img_ctx = repo_root().join("tests").join("images");
        let keydir = img_ctx.join("testkey");
        std::fs::create_dir_all(&keydir).unwrap();
        std::fs::copy(&hostkey, keydir.join("ssh_host_ed25519_key")).unwrap();
        std::fs::copy(
            workdir.join("ssh_host_ed25519_key.pub"),
            keydir.join("ssh_host_ed25519_key.pub"),
        )
        .unwrap();
        std::fs::copy(workdir.join("client.pub"), keydir.join("authorized_keys")).unwrap();

        run_ok(
            "docker",
            &[
                "build",
                "-t",
                tag,
                "-f",
                img_ctx.join("alpine.Dockerfile").to_str().unwrap(),
                img_ctx.to_str().unwrap(),
            ],
        );

        // Run detached with a random host port.
        let out = run_ok(
            "docker",
            &["run", "-d", "--rm", "--name", &name, "-p", "127.0.0.1::22", tag],
        );
        assert!(!out.trim().is_empty(), "docker run returned no container id");

        // Discover the mapped port.
        let port_out = run_ok("docker", &["port", &name, "22/tcp"]);
        let port = port_out
            .lines()
            .next()
            .and_then(|l| l.rsplit(':').next())
            .and_then(|p| p.trim().parse::<u16>().ok())
            .expect("could not parse mapped port");

        // Known-hosts pinned to our fixed host key for stable verification (C-IT3).
        let known_hosts = workdir.join("known_hosts");
        let hostpub = std::fs::read_to_string(workdir.join("ssh_host_ed25519_key.pub")).unwrap();
        let hostpub = hostpub.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        std::fs::write(&known_hosts, format!("[127.0.0.1]:{port} {hostpub}\n")).unwrap();

        let host = Host { name, port, workdir, key_path, known_hosts };
        host.wait_ready();
        host
    }

    fn wait_ready(&self) {
        for _ in 0..30 {
            if self.try_ssh("true").is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        panic!("sshd did not become ready");
    }

    /// Run a command on the host over SSH, returning stdout on success.
    fn try_ssh(&self, remote: &str) -> Result<String, String> {
        let out = Command::new("ssh")
            .args([
                "-i",
                self.key_path.to_str().unwrap(),
                "-o",
                &format!("UserKnownHostsFile={}", self.known_hosts.display()),
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=5",
                "-p",
                &self.port.to_string(),
                "tester@127.0.0.1",
                "--",
                remote,
            ])
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).to_string())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).to_string())
        }
    }

    fn ssh(&self, remote: &str) -> String {
        self.try_ssh(remote).expect("ssh command failed")
    }

    /// Pipe local `data` to a remote command's stdin.
    fn ssh_stdin(&self, remote: &str, data: &[u8]) -> String {
        use std::io::Write;
        use std::process::Stdio;
        let mut child = Command::new("ssh")
            .args([
                "-i",
                self.key_path.to_str().unwrap(),
                "-o",
                &format!("UserKnownHostsFile={}", self.known_hosts.display()),
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "BatchMode=yes",
                "-p",
                &self.port.to_string(),
                "tester@127.0.0.1",
                "--",
                remote,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(data).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "remote stdin cmd failed: {remote}");
        String::from_utf8_lossy(&out.stdout).to_string()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = Command::new("docker").args(["rm", "-f", &self.name]).output();
        let _ = std::fs::remove_dir_all(&self.workdir);
        let _ = std::fs::remove_dir_all(repo_root().join("tests/images/testkey"));
    }
}

fn run_ok(prog: &str, args: &[&str]) -> String {
    let out = Command::new(prog).args(args).output().unwrap_or_else(|e| {
        panic!("failed to spawn {prog}: {e}");
    });
    assert!(
        out.status.success(),
        "{prog} {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn bootstrap_deploys_then_leaves_host_clean_on_alpine() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }

    let host = Host::boot();

    // Deliver bootstrap.sh to the host (~/xxh-bootstrap.sh).
    let script = std::fs::read(repo_root().join("bootstrap/bootstrap.sh")).unwrap();
    host.ssh_stdin("cat > ~/xxh-bootstrap.sh", &script);

    // 1) Platform detection works on BusyBox/musl (§FR-007, C-IT4).
    let detect = host.ssh("sh ~/xxh-bootstrap.sh detect");
    assert!(detect.starts_with("Linux"), "detect output: {detect:?}");

    // 2) Deploy a component into the content-addressed cache.
    let tarball = make_tar_gz(b"echo hi\n");
    host.ssh_stdin("sh ~/xxh-bootstrap.sh recv deadbeef gz", &tarball);
    let listed = host.ssh("sh ~/xxh-bootstrap.sh list-cache");
    assert!(listed.contains("deadbeef"), "cache after recv: {listed:?}");
    assert_eq!(
        host.ssh("test -d ~/.xxh/cache/deadbeef && echo yes").trim(),
        "yes",
        "component should be deployed into ~/.xxh"
    );

    // 3) Ephemeral session: run then exit → the EXIT trap must remove ~/.xxh.
    host.ssh("sh ~/xxh-bootstrap.sh run sess1 0 true");

    // 4) Cleanliness assert (Принцип VIII / §SC-002): host is left clean.
    let leftover = host.ssh("test -e ~/.xxh && echo DIRTY || echo CLEAN");
    assert_eq!(leftover.trim(), "CLEAN", "~/.xxh must be gone after ephemeral exit");
}

#[test]
fn crashed_session_is_cleaned_on_next_connect_on_alpine() {
    // §FR-006 / §SC-007 (C-IT-S3): a crashed session leaves a stale marker; the next
    // connect's reconcile must sweep it and leave the host clean.
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    let host = Host::boot();
    let script = std::fs::read(repo_root().join("bootstrap/bootstrap.sh")).unwrap();
    host.ssh_stdin("cat > ~/xxh-bootstrap.sh", &script);

    // Simulate a crash: an environment left behind with a marker for a dead process.
    host.ssh("sh ~/xxh-bootstrap.sh list-cache >/dev/null"); // creates ~/.xxh
    host.ssh("echo 999999 > ~/.xxh/sessions/crashed; mkdir -p ~/.xxh/cache/stale");
    assert_eq!(
        host.ssh("test -e ~/.xxh && echo yes").trim(),
        "yes",
        "precondition: stale ~/.xxh exists"
    );

    // Next connect runs reconcile → stale marker (dead pid) swept → host clean.
    host.ssh("sh ~/xxh-bootstrap.sh reconcile");
    assert_eq!(
        host.ssh("test -e ~/.xxh && echo DIRTY || echo CLEAN").trim(),
        "CLEAN",
        "reconcile must remove crashed-session leftovers"
    );
}

/// Build a minimal tar.gz in-process (no external tools) for the recv test.
fn make_tar_gz(file_contents: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut tar = Vec::new();
    {
        let mut b = tar::Builder::new(&mut tar);
        let mut header = tar::Header::new_gnu();
        header.set_path("greet.sh").unwrap();
        header.set_size(file_contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        b.append(&header, file_contents).unwrap();
        b.finish().unwrap();
    }
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&tar).unwrap();
    gz.finish().unwrap()
}
