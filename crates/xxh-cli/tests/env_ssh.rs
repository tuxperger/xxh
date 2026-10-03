//! Integration (011 T006, US1): the **real `xxh` binary** sets session variables
//! from `-e/--env` on an sshd host. Values arrive byte for byte in `--` and `-c`
//! runs, a bare name passes this machine's value, a name not set here is skipped
//! with a warning, a reserved or malformed name is a config error before
//! anything happens on the target. No value shows in any command line on the
//! target, in xxh's `-vv` output, or in a file once the command runs; a kept
//! environment keeps none of it (Принцип I, V, VIII). Docker-gated.

mod common;

use common::{Fixture, docker_available, run_with_stdin};

const XXH: &str = env!("CARGO_BIN_EXE_xxh");

/// Every byte class a shell could get wrong.
const VALUE: &str = "s3cr3t 'single' \"double\" $HOME `id` \\ * ;\nsecond line\n";

#[test]
fn session_variables_arrive_exactly_and_leave_no_trace() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    let fx = Fixture::boot();
    fx.ssh_alias("box");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let scratch = fx.home.join("scratch");
    let run = |args: &[&str]| {
        let mut c = fx.xxh(XXH);
        c.env("XDG_CONFIG_HOME", scratch.join("config"))
            .env("XXH_PLUGINS_DIR", scratch.join("plugins"))
            .env("XXH_SHELLS_DIR", scratch.join("shells"))
            .env("XXH_PACK_CACHE_DIR", scratch.join("packed"))
            .env("FROM_CLIENT", "client value")
            .env_remove("NOT_SET_HERE_9F2C")
            // The fixture image has no zsh (the default shell): use the host's sh.
            .args(["--shell", "sh"])
            .args(args);
        let (code, out, err) = run_with_stdin(c, b"");
        (
            code,
            String::from_utf8_lossy(&out).into_owned(),
            String::from_utf8_lossy(&err).into_owned(),
        )
    };
    let secret = format!("SECRET={VALUE}");

    // ── US1: exact values, `--` and `-c` alike (§FR-001, §FR-004, §FR-009) ─
    let (code, out, err) = run(&[
        "box",
        "-e",
        &secret,
        "-e",
        "EMPTY=",
        "-e",
        "A=1",
        "-e",
        "A=2",
        "--",
        "sh",
        "-c",
        "printf '[%s][%s][%s]' \"$SECRET\" \"${EMPTY-unset}\" \"$A\"",
    ]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert_eq!(
        out,
        format!("[{VALUE}][][2]"),
        "byte for byte, last -e wins"
    );
    assert_eq!(err, "", "nothing on stderr without -v");
    assert_eq!(rt.block_on(fx.cleanliness()), "CLEAN");

    let (code, out, err) = run(&["box", "-e", &secret, "-c", "printf '[%s]' \"$SECRET\""]);
    assert_eq!(
        (code, out),
        (Some(0), format!("[{VALUE}]")),
        "stderr: {err}"
    );

    // ── US1/AC3: a bare name passes this machine's value (§FR-005, C-E4) ────
    let (code, out, err) = run(&[
        "box",
        "-e",
        "FROM_CLIENT",
        "-e",
        "NOT_SET_HERE_9F2C",
        "--",
        "sh",
        "-c",
        "printf '[%s][%s]' \"$FROM_CLIENT\" \"${NOT_SET_HERE_9F2C-unset}\"",
    ]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert_eq!(out, "[client value][unset]");
    assert!(
        err.contains("xxh: warning: env: NOT_SET_HERE_9F2C is not set here — skipped"),
        "{err}"
    );

    // ── Reserved and malformed names: config error, target untouched (C-E2) ─
    for bad in ["XXH_ROOT=/tmp/x", "1BAD=v", "A-B=v"] {
        let (code, _, err) = run(&["box", "-e", bad, "--", "true"]);
        assert_eq!(code, Some(40), "`{bad}`: {err}");
        assert!(err.starts_with("xxh: config: env:"), "{err}");
        assert!(!err.contains("/tmp/x"), "no value in the error: {err}");
        assert_eq!(
            rt.block_on(fx.cleanliness()),
            "CLEAN",
            "nothing for `{bad}`"
        );
    }

    // ── §FR-008: no command line on the target and no file holds a value ───
    let probe = r#"
        for f in /proc/[0-9]*/cmdline; do tr '\0' ' ' < "$f" 2>/dev/null; echo; done
        echo "files=$(ls "$HOME"/.xxh/run/*/env 2>/dev/null | wc -l)"
    "#;
    let (code, out, err) = run(&["box", "-e", &secret, "--", "sh", "-c", probe]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(
        out.contains("/boot.sh run "),
        "the probe saw the session: {out}"
    );
    assert!(!out.contains("s3cr3t"), "a value on a command line:\n{out}");
    assert!(
        out.contains("files=0"),
        "the file is gone once sourced: {out}"
    );

    // ── SC-002: nothing at -vv either ───────────────────────────────────────
    let (code, _, err) = run(&["-vv", "box", "-e", &secret, "--", "true"]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(err.contains("environment: 1 variable(s)"), "{err}");
    assert!(!err.contains("s3cr3t"), "a value in the log:\n{err}");

    // ── A kept environment keeps no value (C-E9) ────────────────────────────
    let (code, _, err) = run(&["--keep", "box", "-e", &secret, "--", "true"]);
    assert_eq!(code, Some(0), "stderr: {err}");
    let left = rt.block_on(fx.host_exec(
        "ls -A ~/.xxh/run 2>/dev/null | wc -l; grep -rl s3cr3t ~/.xxh 2>/dev/null | wc -l",
    ));
    assert_eq!(
        left.split_whitespace().collect::<Vec<_>>(),
        ["0", "0"],
        "{left}"
    );
    let (code, _, err) = run(&["clean", "box"]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert_eq!(
        rt.block_on(fx.cleanliness()),
        "CLEAN",
        "~/.xxh must be gone"
    );
}
