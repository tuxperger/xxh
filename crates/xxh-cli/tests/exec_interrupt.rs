//! Integration test (004 T016, US2, §FR-013, SC-003): interrupting the `xxh` binary
//! in the middle of a one-command run stops the command **on the host**, lets the
//! cleanup run, and reports `128 + signal`. Docker-gated; skips when absent.

mod common;

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use common::{Fixture, docker_available};

const XXH: &str = env!("CARGO_BIN_EXE_xxh");

#[test]
fn interrupting_xxh_stops_the_remote_command_and_cleans_up() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    let fx = Fixture::boot();
    fx.ssh_alias("box");

    // A command line whose shell has a child: both must die, not just the shell.
    let mut child = fx
        .xxh(XXH)
        .args(["box", "-c", "echo started; sleep 61; echo survived"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn xxh");
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout
        .read_line(&mut line)
        .expect("read the command's first line");
    assert_eq!(line, "started\n");

    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let status = child.wait().expect("wait xxh");
    assert_eq!(status.code(), Some(143), "128 + SIGTERM");
    let mut rest = String::new();
    let _ = std::io::Read::read_to_string(&mut stdout, &mut rest);
    assert!(
        !rest.contains("survived"),
        "the command line must not run on"
    );

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        // `[1]` keeps the probe's own command line from matching itself.
        let left = fx
            .host_exec("grep -l 'sleep.6[1]' /proc/[0-9]*/cmdline 2>/dev/null | wc -l")
            .await;
        assert_eq!(left.trim(), "0", "the remote command must be gone");
        assert_eq!(fx.cleanliness().await, "CLEAN", "~/.xxh must be gone");
    });
}
