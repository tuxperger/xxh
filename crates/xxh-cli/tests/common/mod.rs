//! Shared integration harness (T014): boots a minimal Alpine sshd container with a
//! generated keypair and a `$HOME` fixture (ssh config, known_hosts, identity) that
//! `RusshTransport` resolves natively. RAII teardown removes the container and all
//! fixture files even when a test panics (C-IT5).
//!
//! One `#[test]` per binary: `Fixture::boot` points `$HOME` at the fixture
//! process-wide, so tests sharing a process must not run concurrently.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use xxh_config::{CleanupMode, Effective};
use xxh_transport::{AuthPolicy, ResolvedSshTarget, RusshTransport, Transport};

pub fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

pub fn run_ok(prog: &str, args: &[&str]) -> String {
    let out = Command::new(prog).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{prog} {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

pub fn eff(shell: &str, cleanup: CleanupMode) -> Effective {
    Effective {
        shell: shell.into(),
        enabled_plugins: vec![],
        cleanup,
        transport: xxh_config::TransportBackend::Russh,
        connect_timeout_s: 10,
    }
}

/// Alpine sshd container + a `$HOME` fixture that RusshTransport resolves.
pub struct Fixture {
    name: String,
    pub home: PathBuf,
    pub port: u16,
}

impl Fixture {
    /// Boot the container, build the fixture and point `$HOME` at it.
    /// `$XXH_TEST_IMAGE` selects the distro (debian/ubuntu/alpine; default alpine —
    /// the critical musl+BusyBox case) so CI can run the full matrix (C-IT10).
    pub fn boot() -> Fixture {
        let image = std::env::var("XXH_TEST_IMAGE").unwrap_or_else(|_| "alpine".into());
        let tag = format!("xxh-test-{image}:latest");
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!("xxh-fx-{}-{nanos:x}", std::process::id());
        let home = std::env::temp_dir().join(format!("{name}-home"));
        let ssh = home.join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();

        // Client key + deterministic host key (C-IT3).
        let key = ssh.join("id_ed25519");
        run_ok(
            "ssh-keygen",
            &["-t", "ed25519", "-N", "", "-q", "-f", key.to_str().unwrap()],
        );
        let hostkey = home.join("hostkey");
        run_ok(
            "ssh-keygen",
            &[
                "-t",
                "ed25519",
                "-N",
                "",
                "-q",
                "-f",
                hostkey.to_str().unwrap(),
            ],
        );

        // Build the image with the generated testkey context.
        let img = repo_root().join("tests/images");
        let keydir = img.join(format!("testkey-{name}"));
        std::fs::create_dir_all(&keydir).unwrap();
        std::fs::copy(&hostkey, keydir.join("ssh_host_ed25519_key")).unwrap();
        std::fs::copy(
            hostkey.with_extension("pub"),
            keydir.join("ssh_host_ed25519_key.pub"),
        )
        .unwrap();
        std::fs::copy(key.with_extension("pub"), keydir.join("authorized_keys")).unwrap();
        run_ok(
            "docker",
            &[
                "build",
                "-t",
                &tag,
                "--build-arg",
                &format!("KEYDIR=testkey-{name}"),
                "-f",
                img.join(format!("{image}.Dockerfile")).to_str().unwrap(),
                img.to_str().unwrap(),
            ],
        );

        run_ok(
            "docker",
            &[
                "run",
                "-d",
                "--rm",
                "--name",
                &name,
                "-p",
                "127.0.0.1::22",
                &tag,
            ],
        );
        let port = run_ok("docker", &["port", &name, "22/tcp"])
            .lines()
            .next()
            .and_then(|l| l.rsplit(':').next())
            .and_then(|p| p.trim().parse::<u16>().ok())
            .expect("mapped port");

        // known_hosts pinned to the fixed host key (russh known_hosts check).
        let hostpub = std::fs::read_to_string(home.join("hostkey.pub")).unwrap();
        let hostpub = hostpub
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        std::fs::write(ssh.join("known_hosts"), format!("127.0.0.1 {hostpub}\n")).unwrap();

        // Point HOME at the fixture so RusshTransport picks up known_hosts + identity.
        // SAFETY: done once before any transport use; one fixture per test process.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("HOME", &home);
        }

        let fx = Fixture { name, home, port };
        fx.wait_ready();
        fx
    }

    pub fn target(&self) -> ResolvedSshTarget {
        let mut t = ResolvedSshTarget::new("127.0.0.1");
        t.port = Some(self.port);
        t.user = Some("tester".into());
        t
    }

    /// Execute a command on the host over a fresh transport (out-of-band probe).
    pub async fn host_exec(&self, cmd: &str) -> String {
        let mut probe = RusshTransport::new();
        probe
            .connect(&self.target(), &AuthPolicy::default())
            .await
            .unwrap();
        let out = probe.exec(cmd).await.unwrap();
        probe.disconnect().await.unwrap();
        out.stdout_str()
    }

    /// The mandatory cleanliness probe (Принцип VIII): "CLEAN" iff `~/.xxh` is gone.
    pub async fn cleanliness(&self) -> String {
        self.host_exec("test -e $HOME/.xxh && echo DIRTY || echo CLEAN")
            .await
            .trim()
            .to_string()
    }

    fn wait_ready(&self) {
        for _ in 0..40 {
            let ok = Command::new("ssh")
                .args([
                    "-i",
                    self.home.join(".ssh/id_ed25519").to_str().unwrap(),
                    "-o",
                    "StrictHostKeyChecking=no",
                    "-o",
                    "UserKnownHostsFile=/dev/null",
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "ConnectTimeout=3",
                    "-p",
                    &self.port.to_string(),
                    "tester@127.0.0.1",
                    "--",
                    "true",
                ])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if ok {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        panic!("sshd not ready");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.name])
            .output();
        let _ = std::fs::remove_dir_all(&self.home);
        let _ = std::fs::remove_dir_all(
            repo_root()
                .join("tests/images")
                .join(format!("testkey-{}", self.name)),
        );
    }
}
