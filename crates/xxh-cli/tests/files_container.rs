//! Integration (010 T006/T007, US1+US2): the **real `xxh` binary** delivers the files
//! declared under `[files]` into a running container over the container
//! transport. Programs in the session find them through `GIT_CONFIG_GLOBAL`,
//! `XDG_CONFIG_HOME` and a user-named variable; the container's own files of the
//! same names stay as they were, nothing is left after the session and the image
//! is not changed (Принцип I, VIII). A host section replaces one entry and drops
//! another — in `config show --host` and in the session (C-F3). Runtime-gated.

mod common;

use std::process::Command;

use common::{ContainerFixture, runtime_available, test_runtime};

const XXH: &str = env!("CARGO_BIN_EXE_xxh");

#[test]
fn declared_files_reach_a_container_and_leave_nothing_behind() {
    let runtime = test_runtime();
    if !runtime_available(&runtime) {
        eprintln!("skipping: container runtime `{runtime}` not available");
        return;
    }
    let fx = ContainerFixture::boot();
    let target = format!("{runtime}:{}", fx.name);
    let scratch = std::env::temp_dir().join(format!("xxh-files-ctr-{}", std::process::id()));
    let home = scratch.join("home");

    // ── The client's files ──────────────────────────────────────────────────
    let write = |rel: &str, body: &str, mode: u32| {
        use std::os::unix::fs::PermissionsExt as _;
        let path = home.join("src").join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    write("gitconfig", "[user]\n\tname = GITMARK-client\n", 0o644);
    write("tool/conf", "CONFMARK-tool\n", 0o644);
    write("tool/private", "PRIVMARK-tool\n", 0o600);
    write("tool/sub/nested", "NESTMARK-tool\n", 0o644);
    write("myrc", "RCMARK-mine\n", 0o644);
    let cfg = scratch.join("config/xxh/config.toml");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    std::fs::write(
        &cfg,
        r#"
[files]
".gitconfig" = "~/src/gitconfig"
".config/tool" = "~/src/tool"
".myrc" = { source = "~/src/myrc", env = "MYTOOL_RC" }
"#,
    )
    .unwrap();

    let xxh = |args: &[&str]| {
        let out = Command::new(XXH)
            // Before the arguments: after `--` it would belong to the command.
            .args(["--shell", "sh"])
            .args(args)
            .env("HOME", &home)
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

    // ── The container's own files of the same names (§FR-003, SC-002) ──────
    fx.exec(
        "printf 'own-git\\n' > ~/.gitconfig && mkdir -p ~/.config/app \
         && printf own > ~/.config/app/own",
    );
    let own = "cksum ~/.gitconfig ~/.config/app/own; ls -ln ~/.gitconfig ~/.config/app/own";
    let before = fx.exec(own);

    // ── US1: programs see the delivered copies (§FR-002, §FR-007) ───────────
    let probe = r#"
        set -e
        case "$GIT_CONFIG_GLOBAL" in "$HOME"/.xxh/*) ;; *) echo "git outside root"; exit 3;; esac
        case "$XDG_CONFIG_HOME" in "$HOME"/.xxh/*) ;; *) echo "xdg outside root"; exit 3;; esac
        cat "$GIT_CONFIG_GLOBAL"
        cat "$XDG_CONFIG_HOME/tool/conf" "$XDG_CONFIG_HOME/tool/private"
        cat "$XDG_CONFIG_HOME/tool/sub/nested" "$MYTOOL_RC"
        echo "mode=$(stat -c %a "$XDG_CONFIG_HOME/tool/private")"
    "#;
    let (code, out, err) = xxh(&[&target, "--", "sh", "-c", probe]);
    assert_eq!(code, Some(0), "stdout: {out}\nstderr: {err}");
    for want in [
        "GITMARK-client",
        "CONFMARK-tool",
        "PRIVMARK-tool",
        "NESTMARK-tool",
        "RCMARK-mine",
        "mode=600",
    ] {
        assert!(out.contains(want), "`{want}` missing from:\n{out}");
    }
    assert!(!err.contains("MARK"), "no file contents on stderr: {err}");

    // ── US2: this host replaces one entry and drops another (C-F3, T007) ────
    write("gitconfig-work", "[user]\n\tname = WORKMARK-host\n", 0o644);
    let mut text = std::fs::read_to_string(&cfg).unwrap();
    text.push_str(&format!(
        "\n[hosts.{}.files]\n\".gitconfig\" = \"~/src/gitconfig-work\"\n\".config/tool\" = false\n",
        fx.name
    ));
    std::fs::write(&cfg, text).unwrap();

    let (code, out, err) = xxh(&["config", "show", "--host", &fx.name]);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(
        out.contains("files.\".gitconfig\" = ~/src/gitconfig-work\n"),
        "{out}"
    );
    assert!(
        out.contains("files.\".myrc\" = ~/src/myrc (env MYTOOL_RC)\n"),
        "{out}"
    );
    assert!(
        !out.contains("files.\".config/tool\""),
        "excluded here: {out}"
    );
    let (code, out, _) = xxh(&["config", "show"]);
    assert_eq!(code, Some(0));
    assert!(
        out.contains("files.\".gitconfig\" = ~/src/gitconfig\n"),
        "{out}"
    );
    assert!(
        out.contains("files.\".config/tool\" = ~/src/tool\n"),
        "{out}"
    );

    let probe = r#"
        cat "$GIT_CONFIG_GLOBAL" "$MYTOOL_RC"
        echo "xdg=${XDG_CONFIG_HOME:-unset}"
    "#;
    let (code, out, err) = xxh(&[&target, "--", "sh", "-c", probe]);
    assert_eq!(code, Some(0), "stdout: {out}\nstderr: {err}");
    assert!(
        out.contains("WORKMARK-host"),
        "the host's entry wins: {out}"
    );
    assert!(
        !out.contains("GITMARK"),
        "the global entry is replaced: {out}"
    );
    assert!(out.contains("RCMARK-mine"), "other entries stay: {out}");
    assert!(out.contains("xdg=unset"), "`false` drops the entry: {out}");

    // ── Own files untouched; container clean; image unchanged (C-DT3) ───────
    assert_eq!(fx.exec(own), before, "own files must not change");
    assert_eq!(fx.cleanliness(), "CLEAN", "~/.xxh must be gone");
    assert!(fx.diff_clean(), "{}", fx.diff());
    assert!(fx.image_digest_unchanged(), "the image must not change");

    let _ = std::fs::remove_dir_all(&scratch);
}
