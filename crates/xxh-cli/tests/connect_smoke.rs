//! Integration test (T021): full session over the **real `RusshTransport`** against a
//! live container. connect → detect → deploy env → run command in the env → exit →
//! assert the host is clean (Принцип VIII / §SC-002, C-IT-S1/S2).
//!
//! Drives our transport by pointing `$HOME` at a fixture with `~/.ssh/{config,
//! known_hosts, key}`, which `RusshTransport` (russh-config + known_hosts) reads
//! natively — no transport code changes needed for testability.
//!
//! Requires docker + network (apk). Skips (passes) when docker is absent.

use std::path::{Path, PathBuf};
use std::process::Command;

use xxh_config::Effective;
use xxh_core::session::{Session, minimal_env_component, silent_progress};
use xxh_transport::{ResolvedSshTarget, RusshTransport};

fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn run_ok(prog: &str, args: &[&str]) -> String {
    let out = Command::new(prog).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{prog} {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Alpine sshd container + a `$HOME` fixture that RusshTransport resolves. RAII teardown.
struct Fixture {
    name: String,
    home: PathBuf,
    port: u16,
}

impl Fixture {
    fn boot() -> Fixture {
        let tag = "xxh-test-alpine:latest";
        let name = format!("xxh-cs-{}", std::process::id());
        let home = std::env::temp_dir().join(format!("{name}-home"));
        let ssh = home.join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();

        // Client key + deterministic host key.
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
        let keydir = img.join("testkey");
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
                tag,
                "-f",
                img.join("alpine.Dockerfile").to_str().unwrap(),
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
                tag,
            ],
        );
        let port = run_ok("docker", &["port", &name, "22/tcp"])
            .lines()
            .next()
            .and_then(|l| l.rsplit(':').next())
            .and_then(|p| p.trim().parse::<u16>().ok())
            .expect("mapped port");

        // ~/.ssh/known_hosts pinned to the fixed host key (russh known_hosts_check).
        // The client identity is the default ~/.ssh/id_ed25519 that RusshTransport
        // falls back to; both are read from the fixture HOME at connect time.
        let hostpub = std::fs::read_to_string(home.join("hostkey.pub")).unwrap();
        let hostpub = hostpub
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        std::fs::write(ssh.join("known_hosts"), format!("127.0.0.1 {hostpub}\n")).unwrap();

        let fx = Fixture { name, home, port };
        fx.wait_ready(port);
        fx
    }

    fn wait_ready(&self, port: u16) {
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
                    &port.to_string(),
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
        let _ = std::fs::remove_dir_all(repo_root().join("tests/images/testkey"));
    }
}

#[test]
fn full_session_over_russh_leaves_host_clean() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }

    let fx = Fixture::boot();

    // Point HOME at the fixture so RusshTransport picks up ssh config + known_hosts.
    // SAFETY: single-threaded test process; set before any transport use.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("HOME", &fx.home);
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    let (marker, clean) = rt.block_on(async {
        let eff = Effective {
            shell: "sh".into(), // MVP: host sh; real shells are plugins (T020)
            enabled_plugins: vec![],
            cleanup: xxh_config::CleanupMode::Ephemeral,
            transport: xxh_config::TransportBackend::Russh,
            connect_timeout_s: 10,
            user: None,
            identity: None,
            container_runtime: xxh_config::RuntimeSetting::Auto,
        };
        let env = vec![minimal_env_component("gz").unwrap()];
        let mut ssh = ResolvedSshTarget::new("127.0.0.1");
        ssh.port = Some(fx.port);
        ssh.user = Some("tester".into());
        // Exercise the explicit-identity path (`-i`): the key is used exclusively.
        ssh.identity = Some(fx.home.join(".ssh/id_ed25519"));
        let target = xxh_transport::ResolvedTarget::Ssh(ssh);

        let mut session = Session::establish(
            RusshTransport::new(),
            &target,
            &eff,
            &env,
            &[],
            silent_progress(),
        )
        .await
        .expect("establish session");

        // Run a command inside the delivered environment: the env.sh sets XXH_SESSION=1.
        let code = session
            .run_command("test \"$XXH_SESSION\" = 1 && echo OK")
            .await
            .expect("run command");
        assert_eq!(code, 0, "env should be sourced (XXH_SESSION=1)");
        session.finish().await.expect("finish");

        // Cleanliness assert (Принцип VIII): reconnect and check ~/.xxh is gone.
        let mut probe = RusshTransport::new();
        use xxh_transport::{AuthPolicy, Transport};
        probe
            .connect(&target, &AuthPolicy::default())
            .await
            .unwrap();
        let out = probe
            .exec("test -e $HOME/.xxh && echo DIRTY || echo CLEAN")
            .await
            .unwrap();
        probe.disconnect().await.unwrap();
        (code, out.stdout_str())
    });

    assert_eq!(marker, 0);
    assert_eq!(
        clean, "CLEAN",
        "~/.xxh must be gone after ephemeral session"
    );
}
