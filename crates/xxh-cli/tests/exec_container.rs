//! Integration test (004 T014, US1): the **real `xxh` binary** runs one command in
//! a running container through the runtime's exec — same exit-code, stdin and
//! argument guarantees as over SSH — and leaves the container clean and its image
//! untouched (C-DT3). Runtime-gated; skips when none is available.

mod common;

use std::process::Command;

use common::{ContainerFixture, run_with_stdin, runtime_available, test_runtime};

const XXH: &str = env!("CARGO_BIN_EXE_xxh");

#[test]
fn one_command_runs_in_a_container_and_leaves_it_clean() {
    let runtime = test_runtime();
    if !runtime_available(&runtime) {
        eprintln!("skipping: container runtime `{runtime}` not available");
        return;
    }
    let fx = ContainerFixture::boot();
    let target = format!("{runtime}:{}", fx.name);
    let run = |args: &[&str], stdin: &[u8]| {
        let mut c = Command::new(XXH);
        // The bare base image has no zsh (the default shell): use the host's sh.
        c.args(["--shell", "sh"]).arg(&target).args(args);
        let out = run_with_stdin(c, stdin);
        assert_eq!(
            fx.cleanliness(),
            "CLEAN",
            "xxh root must be gone after {args:?}"
        );
        out
    };
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();

    let (code, out, err) = run(&["--", "sh", "-c", "echo $XXH_SESSION; exit 7"], b"");
    assert_eq!(code, Some(7), "stderr: {}", text(&err));
    assert_eq!(text(&out), "1\n");
    assert_eq!(text(&err), "", "no xxh chatter without -v");

    let (code, out, _) = run(&["--", "cat"], b"piped\ndata");
    assert_eq!((code, text(&out)), (Some(0), "piped\ndata".into()));

    let (code, out, _) = run(&["--", "printf", "[%s]", "a b", "c'd", "$HOME;x", ""], b"");
    assert_eq!(
        (code, text(&out)),
        (Some(0), "[a b][c'd][$HOME;x][]".into())
    );

    let (code, out, err) = run(&["-c", "printf out; printf err >&2"], b"");
    assert_eq!(
        (code, text(&out), text(&err)),
        (Some(0), "out".into(), "err".into())
    );

    assert!(
        fx.image_digest_unchanged(),
        "the image must not be modified"
    );
}
