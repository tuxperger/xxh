//! Integration test (004 T013/T015/T017/T018, US1+US2): the **real `xxh` binary**
//! runs one command on an sshd host with the client's stdio attached — exit code,
//! stdin, exact arguments, byte-exact and separate output streams — and the host
//! is clean after every run (Принцип VIII). Docker-gated; skips when absent.

mod common;

use common::{Fixture, docker_available, run_with_stdin};

const XXH: &str = env!("CARGO_BIN_EXE_xxh");

#[test]
fn one_command_runs_with_client_stdio_and_leaves_the_host_clean() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    let fx = Fixture::boot();
    fx.ssh_alias("box");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let run = |args: &[&str], stdin: &[u8]| {
        let mut c = fx.xxh(XXH);
        // The fixture image has no zsh (the default shell): use the host's sh.
        c.args(["--shell", "sh"]).args(args);
        let out = run_with_stdin(c, stdin);
        // Every run is ephemeral: nothing may be left behind, whatever the outcome.
        assert_eq!(
            rt.block_on(fx.cleanliness()),
            "CLEAN",
            "~/.xxh must be gone after `xxh {args:?}`"
        );
        out
    };
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();

    // ── US1: exit code and environment (§FR-002, §FR-004) ───────────────────
    let (code, out, err) = run(&["box", "--", "sh", "-c", "echo $XXH_SESSION"], b"");
    assert_eq!(code, Some(0), "stderr: {}", text(&err));
    assert_eq!(
        text(&out),
        "1\n",
        "the command runs inside the xxh environment"
    );
    // Stage progress stays out of a script's stderr unless asked for (§FR-012).
    assert_eq!(text(&err), "", "no xxh chatter without -v");

    let (code, out, _) = run(&["box", "--", "sh", "-c", "exit 7"], b"");
    assert_eq!(
        (code, out.len()),
        (Some(7), 0),
        "the command's code is xxh's code"
    );

    // ── US1: stdin reaches the command (§FR-003) ────────────────────────────
    let (code, out, _) = run(&["box", "--", "cat"], b"piped\ndata");
    assert_eq!((code, text(&out)), (Some(0), "piped\ndata".into()));

    // ── US1: arguments arrive exactly, never re-interpreted (§FR-006) ───────
    let (code, out, _) = run(
        &[
            "box", "--", "printf", "[%s]", "a b", "c'd", "$HOME;x", "*", "",
        ],
        b"",
    );
    assert_eq!(
        (code, text(&out)),
        (Some(0), "[a b][c'd][$HOME;x][*][]".into())
    );

    // ── US1: a command line goes through the shell (§FR-011) ────────────────
    let (code, out, _) = run(&["box", "-c", "echo a | wc -c"], b"");
    assert_eq!((code, text(&out).trim().to_string()), (Some(0), "2".into()));

    // ── US2: byte-exact stdout, separate stderr (§FR-003, §FR-005, SC-001) ──
    let (code, out, err) = run(
        &[
            "box",
            "--",
            "sh",
            "-c",
            "printf 'a\\tb\\001\\377no-newline'; printf err >&2",
        ],
        b"",
    );
    assert_eq!(code, Some(0));
    assert_eq!(
        out, b"a\tb\x01\xffno-newline",
        "stdout is the command's bytes"
    );
    assert_eq!(text(&err), "err", "stderr is the command's stderr only");

    // With -v the stages appear — on stderr; stdout does not change.
    let (code, out, err) = run(&["-v", "box", "--", "printf", "x"], b"");
    assert_eq!((code, text(&out)), (Some(0), "x".into()));
    assert!(
        text(&err).contains("xxh: ▸"),
        "stages on request: {}",
        text(&err)
    );

    // A megabyte through stdin → cat → stdout survives unchanged.
    let big: Vec<u8> = (0..1_048_576u32).map(|i| (i % 251) as u8).collect();
    let (code, out, _) = run(&["box", "--", "cat"], &big);
    assert_eq!(code, Some(0));
    assert!(
        out == big,
        "1 MiB round trip must be byte-exact ({} bytes back)",
        out.len()
    );

    // ── US2: a terminal only on request (§FR-008) ───────────────────────────
    let (code, _, _) = run(&["box", "--", "test", "-t", "1"], b"");
    assert_eq!(code, Some(1), "no terminal by default");
    let (code, _, _) = run(&["box", "-t", "--", "test", "-t", "1"], b"");
    assert_eq!(code, Some(0), "-t allocates a terminal");

    // ── US2: never prompt on a non-terminal stdin (§FR-009) ─────────────────
    // A rejected key would fall back to a password prompt; with stdin piped that
    // prompt must become a transport error instead of eating the input.
    let (code, out, err) = run(&["-l", "nobody", "box", "--", "cat"], b"not-a-password\n");
    assert_eq!(code, Some(10), "stderr: {}", text(&err));
    assert!(out.is_empty());
    assert!(
        text(&err).contains("xxh: transport:"),
        "class is named: {}",
        text(&err)
    );
    assert!(
        text(&err).contains("not a terminal"),
        "cause is named: {}",
        text(&err)
    );
}
