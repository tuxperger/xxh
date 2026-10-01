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
use xxh_transport::{
    AuthPolicy, ContainerRuntime, ContainerTarget, ResolvedSshTarget, ResolvedTarget,
    RuntimeSelector, RusshTransport, Transport,
};

pub fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Whether the given container runtime CLI has a live daemon/socket (C-DT8: a
/// missing runtime skips the container cell with a message, never a failure).
pub fn runtime_available(runtime: &str) -> bool {
    Command::new(runtime)
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The runtime under test (`XXH_TEST_RUNTIME`, default docker) — parameterizes the
/// dual-transport matrix's container axis (C-DT1, podman-smoke via the env var).
pub fn test_runtime() -> String {
    std::env::var("XXH_TEST_RUNTIME").unwrap_or_else(|_| "docker".into())
}

/// The base image for the container path, selected by `XXH_TEST_IMAGE` — the SAME
/// distros the SSH path targets (C-DT1); default alpine, the musl+BusyBox cell.
pub fn container_base_image() -> String {
    match std::env::var("XXH_TEST_IMAGE").as_deref() {
        Ok("debian") => "debian:12".into(),
        Ok("ubuntu") => "ubuntu:24.04".into(),
        _ => "alpine:3.20".into(),
    }
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
        user: None,
        identity: None,
        container_runtime: xxh_config::RuntimeSetting::Auto,
    }
}

/// Alpine sshd container + a `$HOME` fixture that RusshTransport resolves.
pub struct Fixture {
    name: String,
    pub home: PathBuf,
    pub port: u16,
    /// The image this fixture built — reused for containers behind it (012).
    tag: String,
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

        let fx = Fixture {
            name,
            home,
            port,
            tag,
        };
        fx.wait_ready();
        fx
    }

    pub fn target(&self) -> ResolvedTarget {
        let mut t = ResolvedSshTarget::new("127.0.0.1");
        t.port = Some(self.port);
        t.user = Some("tester".into());
        ResolvedTarget::Ssh(t)
    }

    /// The container name backing this fixture (for reaching it via the container
    /// transport in the dual-transport parity scenario, C-DT5).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The *same* container as [`Self::target`], addressed for the container
    /// transport — one image, two transports (C-DT5).
    pub fn container_target(&self) -> ResolvedTarget {
        ResolvedTarget::Container(ContainerTarget {
            reference: self.name.clone(),
            runtime: RuntimeSelector::Explicit(ContainerRuntime::Docker),
            exec_user: None,
            connect_timeout_s: 10,
        })
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
        let _ = Command::new("docker")
            .args(["network", "rm", &self.network()])
            .output();
        let _ = std::fs::remove_dir_all(&self.home);
        let _ = std::fs::remove_dir_all(
            repo_root()
                .join("tests/images")
                .join(format!("testkey-{}", self.name)),
        );
    }
}

/// A running container reached by the **container transport directly** — no sshd,
/// no keys (C-DT2). Boots a base image with `sleep infinity` and connects via
/// `ContainerCliTransport`. RAII teardown removes the container even on panic
/// (C-DT7).
pub struct ContainerFixture {
    pub name: String,
    pub runtime: String,
    pub image: String,
    /// Image id the container was created from, captured at boot for the
    /// immutability assert (C-DT3): a session must never rebuild/commit it.
    image_id: String,
}

impl ContainerFixture {
    /// Boot the container on the runtime under test. Panics if the runtime cannot
    /// start it — callers gate on [`runtime_available`] and skip when absent.
    pub fn boot() -> ContainerFixture {
        let runtime = test_runtime();
        let image = container_base_image();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!("xxh-ctr-{}-{nanos:x}", std::process::id());
        run_ok(
            &runtime,
            &[
                "run", "-d", "--rm", "--name", &name, &image, "sleep", "infinity",
            ],
        );
        let image_id = run_ok(&runtime, &["inspect", "--format", "{{.Image}}", &name])
            .trim()
            .to_string();
        let fx = ContainerFixture {
            name,
            runtime,
            image,
            image_id,
        };
        fx.wait_running();
        fx
    }

    /// A container target addressed by the resolved runtime (explicit, so tests are
    /// deterministic regardless of what else is installed).
    pub fn target(&self) -> ResolvedTarget {
        let rt = match self.runtime.as_str() {
            "podman" => ContainerRuntime::Podman,
            _ => ContainerRuntime::Docker,
        };
        ResolvedTarget::Container(ContainerTarget {
            reference: self.name.clone(),
            runtime: RuntimeSelector::Explicit(rt),
            exec_user: None,
            connect_timeout_s: 10,
        })
    }

    /// Run a command inside the container out-of-band (probe, not via xxh).
    pub fn exec(&self, cmd: &str) -> String {
        run_ok(&self.runtime, &["exec", &self.name, "sh", "-c", cmd])
    }

    /// The runtime's view of filesystem changes since image creation (C-DT3).
    pub fn diff(&self) -> String {
        run_ok(&self.runtime, &["diff", &self.name])
    }

    /// C-DT3 assert #1: the `diff` report carries no xxh artefacts. (The env root
    /// under `$HOME`/`$TMPDIR`/`/tmp` always contains `.xxh`, so that name is the
    /// tell-tale for anything xxh wrote.)
    pub fn diff_clean(&self) -> bool {
        !self.diff().contains(".xxh")
    }

    /// C-DT3 assert #2: the container still runs its original image — a session
    /// must never `commit`/`build` (the image id is immutable unless it does).
    pub fn image_digest_unchanged(&self) -> bool {
        let now = run_ok(
            &self.runtime,
            &["inspect", "--format", "{{.Image}}", &self.name],
        )
        .trim()
        .to_string();
        now == self.image_id
    }

    /// Cleanliness probe (Принцип VIII): "CLEAN" iff the xxh root under the exec
    /// user's `$HOME` is gone. `sh -lc` is avoided; use the plain `$HOME`.
    pub fn cleanliness(&self) -> String {
        self.exec("test -e \"$HOME/.xxh\" && echo DIRTY || echo CLEAN")
            .trim()
            .to_string()
    }

    fn wait_running(&self) {
        for _ in 0..40 {
            let running = Command::new(&self.runtime)
                .args([
                    "inspect",
                    "--type",
                    "container",
                    "--format",
                    "{{.State.Running}}",
                    &self.name,
                ])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "true")
                .unwrap_or(false);
            if running {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        panic!("container not running");
    }
}

impl Drop for ContainerFixture {
    fn drop(&mut self) {
        let _ = Command::new(&self.runtime)
            .args(["rm", "-f", &self.name])
            .output();
    }
}

/// Whether this client has Nix with flakes (Nix-backed scenarios skip without it).
pub fn nix_available() -> bool {
    Command::new("nix")
        .args(["flake", "metadata", "--help"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run git in `dir` with a fixed identity (test repositories never read the
/// developer's git config for authorship).
pub fn git_in(dir: &Path, args: &[&str]) -> String {
    let mut full = vec![
        "-C",
        dir.to_str().unwrap(),
        "-c",
        "user.name=xxh-test",
        "-c",
        "user.email=xxh-test@example.invalid",
        "-c",
        "commit.gpgsign=false",
    ];
    full.extend_from_slice(args);
    run_ok("git", &full)
}

/// A committed flake repository in `dir` (003 research R11) exposing, for this
/// client's system:
///   * `tool`   — a statically linked program (`pkgsStatic.hello`);
///   * `dyn`    — the same program dynamically linked (must be rejected);
///   * `plugin` — a ready-made plugin package (`plugin.toml` + `env.sh`).
///
/// nixpkgs follows the provider's pin (`XXH_NIXPKGS_PIN`), and the lock file is
/// committed so the tree is clean and the source has a real revision.
pub fn make_test_flake(dir: &Path) {
    let pin = std::env::var("XXH_NIXPKGS_PIN")
        .unwrap_or_else(|_| "github:NixOS/nixpkgs/nixos-25.05".into());
    let system = run_ok(
        "nix",
        &[
            "eval",
            "--impure",
            "--raw",
            "--expr",
            "builtins.currentSystem",
        ],
    );
    let flake = r#"{
  inputs.nixpkgs.url = "@PIN@";
  outputs = { self, nixpkgs }:
    let
      system = "@SYSTEM@";
      pkgs = nixpkgs.legacyPackages.${system};
    in {
      packages.${system} = {
        tool = pkgs.pkgsStatic.hello;
        dyn = pkgs.hello;
        plugin = pkgs.runCommandLocal "demo-plugin" { } ''
          mkdir -p $out
          printf 'name = "demo"\nversion = "2.0.0"\napi_version = "1.0.0"\n' > $out/plugin.toml
          printf 'export DEMO_FROM_FLAKE=1\n' > $out/env.sh
        '';
      };
    };
}
"#
    .replace("@PIN@", &pin)
    .replace("@SYSTEM@", system.trim());
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("flake.nix"), flake).unwrap();
    git_in(dir, &["init", "-q", "-b", "main"]);
    git_in(dir, &["add", "flake.nix"]);
    run_ok("nix", &["flake", "lock", dir.to_str().unwrap()]);
    git_in(dir, &["add", "flake.lock"]);
    git_in(dir, &["commit", "-q", "-m", "test flake"]);
}

impl Fixture {
    /// Make this host reachable by the **`xxh` binary** as `alias`: the CLI has no
    /// port flag, so the mapped port, user and key go through `~/.ssh/config` in
    /// the fixture `$HOME` (which the russh backend resolves natively).
    pub fn ssh_alias(&self, alias: &str) {
        let key = self.home.join(".ssh/id_ed25519");
        std::fs::write(
            self.home.join(".ssh/config"),
            format!(
                "Host {alias}\n  HostName 127.0.0.1\n  Port {}\n  User tester\n  IdentityFile {}\n",
                self.port,
                key.display()
            ),
        )
        .unwrap();
    }

    /// The `xxh` binary with `$HOME` pointed at this fixture.
    pub fn xxh(&self, bin: &str) -> Command {
        let mut c = Command::new(bin);
        c.env("HOME", &self.home);
        c
    }
}

/// Run `cmd` to completion, feeding `stdin`, and return (exit code, stdout, stderr).
pub fn run_with_stdin(mut cmd: Command, stdin: &[u8]) -> (Option<i32>, Vec<u8>, Vec<u8>) {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn xxh");
    let mut pipe = child.stdin.take().unwrap();
    let data = stdin.to_vec();
    // Feed from a thread: a large input must not deadlock against our own reads.
    let feeder = std::thread::spawn(move || {
        let _ = pipe.write_all(&data);
    });
    let out = child.wait_with_output().expect("wait xxh");
    let _ = feeder.join();
    (out.status.code(), out.stdout, out.stderr)
}

impl Fixture {
    fn network(&self) -> String {
        format!("{}-net", self.name)
    }

    /// Another sshd from this fixture's image — same host key, same authorized
    /// client key — on a private network shared with this fixture's container,
    /// with **no** published port: reachable only through this one, as a host
    /// behind a bastion (012 T009). Its name is its address on that network.
    pub fn hidden(&self, suffix: &str) -> Hidden {
        let net = self.network();
        // Both may already exist from an earlier `hidden` call.
        let _ = Command::new("docker")
            .args(["network", "create", &net])
            .output();
        let _ = Command::new("docker")
            .args(["network", "connect", &net, &self.name])
            .output();
        allow_tcp_forwarding(&self.name);
        let name = format!("{}-{suffix}", self.name);
        run_ok(
            "docker",
            &[
                "run",
                "-d",
                "--rm",
                "--network",
                &net,
                "--name",
                &name,
                &self.tag,
            ],
        );
        for _ in 0..40 {
            let up = Command::new("docker")
                .args(["exec", &name, "sh", "-c", "pgrep sshd >/dev/null"])
                .output()
                .is_ok_and(|o| o.status.success());
            if up {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        allow_tcp_forwarding(&name);
        Hidden { name }
    }
}

/// A bastion forwards TCP; Alpine's sshd ships with `AllowTcpForwarding no`.
/// Changed in the running container only (the image stays minimal) and sshd —
/// PID 1 — re-reads its config on HUP.
fn allow_tcp_forwarding(container: &str) {
    let _ = Command::new("docker")
        .args([
            "exec",
            container,
            "sh",
            "-c",
            "sed -i '/^AllowTcpForwarding/d' /etc/ssh/sshd_config \
             && echo 'AllowTcpForwarding yes' >> /etc/ssh/sshd_config && kill -HUP 1",
        ])
        .output();
    std::thread::sleep(std::time::Duration::from_millis(500));
}

/// A container behind a [`Fixture`]; removed on drop.
pub struct Hidden {
    pub name: String,
}

impl Hidden {
    /// Run `cmd` as the test user inside the container (out of band).
    pub fn exec(&self, cmd: &str) -> String {
        run_ok(
            "docker",
            &["exec", "-u", "tester", &self.name, "sh", "-c", cmd],
        )
    }

    /// "CLEAN" iff the test user's `~/.xxh` is gone (Принцип VIII).
    pub fn cleanliness(&self) -> String {
        self.exec("test -e \"$HOME/.xxh\" && echo DIRTY || echo CLEAN")
            .trim()
            .to_string()
    }
}

impl Drop for Hidden {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.name])
            .output();
    }
}
