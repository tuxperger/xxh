//! Integration (010 T005/T009, US1+US3): the **real `xxh` binary** delivers the files
//! declared under `[files]` to an sshd host. Programs in the session find them
//! through `GIT_CONFIG_GLOBAL`, `XDG_CONFIG_HOME` and a variable the user named,
//! with their permission bits; a missing source and a secret are reported and
//! left out; the target's own files of the same names stay byte-for-byte as they
//! were, and nothing is left after the session (Принцип I, VIII). File contents
//! never reach xxh's own output, even at `-vv` (C-F12). A kept environment
//! is not sent again, and a changed entry goes alone (C-F4). Docker-gated.

mod common;

use common::{Fixture, docker_available, run_with_stdin};

const XXH: &str = env!("CARGO_BIN_EXE_xxh");

#[test]
fn declared_files_are_visible_in_the_session_and_leave_nothing_behind() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    let fx = Fixture::boot();
    fx.ssh_alias("box");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let scratch = fx.home.join("scratch");

    // ── The client's files (sources live in the fixture `$HOME`) ────────────
    let src = fx.home.join("src");
    let write = |rel: &str, body: &str, mode: u32| {
        use std::os::unix::fs::PermissionsExt as _;
        let path = src.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    write("gitconfig", "[user]\n\tname = GITMARK-client\n", 0o644);
    write("tool/conf", "CONFMARK-tool\n", 0o644);
    write("tool/private", "PRIVMARK-tool\n", 0o600);
    write("tool/sub/nested", "NESTMARK-tool\n", 0o644);
    write("myrc", "RCMARK-mine\n", 0o644);
    write("api_token", "SECRETMARK-token\n", 0o600);
    let cfg = scratch.join("config/xxh/config.toml");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    std::fs::write(
        &cfg,
        r#"
[files]
".gitconfig" = "~/src/gitconfig"
".config/tool" = "~/src/tool"
".myrc" = { source = "~/src/myrc", env = "MYTOOL_RC" }
".inputrc" = "~/src/no-such-inputrc"
".apitoken" = { source = "~/src/api_token", env = "API_TOKEN_FILE" }
"#,
    )
    .unwrap();

    let run = |args: &[&str]| {
        let mut c = fx.xxh(XXH);
        c.env("XDG_CONFIG_HOME", scratch.join("config"))
            .env("XXH_PLUGINS_DIR", scratch.join("plugins"))
            .env("XXH_SHELLS_DIR", scratch.join("shells"))
            .env("XXH_PACK_CACHE_DIR", scratch.join("packed"))
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

    // ── The target's own files of the same names (§FR-003, SC-002) ──────────
    rt.block_on(fx.host_exec(
        "printf 'own-git\\n' > ~/.gitconfig && chmod 0640 ~/.gitconfig \
         && mkdir -p ~/.config/app && printf own > ~/.config/app/own",
    ));
    let own = "cksum ~/.gitconfig ~/.config/app/own; ls -ln ~/.gitconfig ~/.config/app/own; \
               ls -a ~ ~/.config";
    let before = rt.block_on(fx.host_exec(own));

    // ── US1: programs see the delivered copies (§FR-001, §FR-002, §FR-007) ──
    let probe = r#"
        set -e
        case "$GIT_CONFIG_GLOBAL" in "$HOME"/.xxh/*) ;; *) echo "git outside root"; exit 3;; esac
        case "$XDG_CONFIG_HOME" in "$HOME"/.xxh/*) ;; *) echo "xdg outside root"; exit 3;; esac
        cat "$GIT_CONFIG_GLOBAL"
        cat "$XDG_CONFIG_HOME/tool/conf" "$XDG_CONFIG_HOME/tool/private"
        cat "$XDG_CONFIG_HOME/tool/sub/nested" "$MYTOOL_RC"
        echo "mode=$(stat -c %a "$XDG_CONFIG_HOME/tool/private")"
        echo "token=${API_TOKEN_FILE:-unset}"
        if grep -rq "SECRET""MARK" "$HOME/.xxh"; then echo LEAK; else echo NOLEAK; fi
    "#;
    let (code, out, err) = run(&["box", "--", "sh", "-c", probe]);
    assert_eq!(code, Some(0), "stdout: {out}\nstderr: {err}");
    for want in [
        "GITMARK-client",
        "CONFMARK-tool",
        "PRIVMARK-tool",
        "NESTMARK-tool",
        "RCMARK-mine",
        "mode=600",
        "token=unset",
        "NOLEAK",
    ] {
        assert!(out.contains(want), "`{want}` missing from:\n{out}");
    }

    // The missing source and the secret are named, before connecting (C-F8, C-F9).
    assert!(
        err.contains("xxh: warning: files: `.inputrc`") && err.contains("does not exist"),
        "missing source is reported: {err}"
    );
    assert!(
        err.contains("xxh: warning: files: `.apitoken` is not delivered")
            && err.contains("secret = true"),
        "secret is reported: {err}"
    );
    assert!(!err.contains("MARK"), "no file contents in warnings: {err}");

    // ── Nothing of the files leaks into xxh's own log, at any level (C-F12) ─
    let (code, out, err) = run(&["-vv", "box", "--", "true"]);
    assert_eq!(code, Some(0), "stdout: {out}\nstderr: {err}");
    assert!(err.contains("xxh: ▸"), "-vv shows the stages: {err}");
    assert!(
        !err.contains("MARK"),
        "file contents in the -vv log:\n{err}"
    );
    assert_eq!(rt.block_on(fx.cleanliness()), "CLEAN");

    // ── US3: a kept environment is not sent again (§FR-006, SC-004, T009) ───
    let (code, _, err) = run(&["-v", "--keep", "box", "--", "true"]);
    assert_eq!(code, Some(0), "stderr: {err}");
    let (code, _, err) = run(&["-v", "--keep", "box", "--", "true"]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(
        err.contains("sending 0,"),
        "nothing changed, nothing sent: {err}"
    );

    // One entry outside `.config/` changes: only it goes (C-F4).
    write("myrc", "RCMARK-changed\n", 0o644);
    let (code, out, err) = run(&[
        "-v",
        "--keep",
        "box",
        "--",
        "sh",
        "-c",
        "cat \"$MYTOOL_RC\"",
    ]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(err.contains("sending 1,"), "only the changed entry: {err}");
    assert_eq!(out, "RCMARK-changed\n");

    // `status` knows the files' components and finds them current (C-F5).
    let (code, out, err) = run(&["status", "box", "--json"]);
    assert_eq!(code, Some(0), "stderr: {err}");
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let comps = v["envs"][0]["components"].as_array().unwrap();
    let labels: Vec<&str> = comps.iter().filter_map(|c| c["label"].as_str()).collect();
    for want in ["files .gitconfig", "files .myrc", "files .config"] {
        assert!(labels.contains(&want), "`{want}` in {labels:?}");
    }
    let current = |label: &str| {
        comps
            .iter()
            .filter(|c| c["label"] == label)
            .any(|c| c["state"] == "current")
    };
    assert!(current("files .myrc"), "the changed entry is current: {v}");
    assert!(current("files .gitconfig"), "{v}");

    // `clean` removes everything, files included.
    let (code, out, err) = run(&["clean", "box"]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(out.contains("removed "), "{out}");

    // ── The target's own files untouched; the host clean (§FR-003/4, SC-003) ─
    assert_eq!(
        rt.block_on(fx.host_exec(own)),
        before,
        "own files must not change"
    );
    assert_eq!(
        rt.block_on(fx.cleanliness()),
        "CLEAN",
        "~/.xxh must be gone"
    );
}
