//! Integration (007 T005/T010/T013): the **real `xxh` binary** — `xxh completions`,
//! the `__complete` protocol behind the stubs, the stubs themselves in real
//! bash / zsh / fish, and `xxh man` (contracts/completions-and-man.md C-K*).
//!
//! Nothing here touches a target: the client is isolated from the developer's
//! config, ssh_config, plugins and shells, and the container runtime is a
//! stand-in script on `PATH`. A shell that is not installed skips its part.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

const XXH: &str = env!("CARGO_BIN_EXE_xxh");

/// An isolated client: its own home, config, registry and `PATH`.
struct Client {
    root: PathBuf,
}

impl Client {
    fn new(tag: &str) -> Client {
        let root = std::env::temp_dir().join(format!(
            "xxh-completions-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        for dir in ["home/.ssh", "config/xxh", "bin"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::os::unix::fs::symlink(XXH, root.join("bin/xxh")).unwrap();
        Client { root }
    }

    /// Put an executable script on the client's `PATH`.
    fn script(&self, name: &str, body: &str) {
        let path = self.root.join("bin").join(name);
        write_script(&path, body);
    }

    /// `program` with only the client's environment: `PATH` holds `xxh` and the
    /// stand-ins, nothing of the developer's machine.
    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut cmd = Command::new(program);
        cmd.env_clear()
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XXH_PLUGINS_DIR", self.root.join("plugins"))
            .env("XXH_SHELLS_DIR", self.root.join("shells"))
            .env("PATH", self.root.join("bin"))
            .env("TERM", "dumb")
            .current_dir(&self.root);
        cmd
    }

    fn xxh(&self, args: &[&str]) -> (Option<i32>, String, String) {
        output(self.command(XXH).args(args))
    }

    /// What the stubs get for the last of `words`: the candidate values.
    fn complete(&self, words: &[&str]) -> Vec<String> {
        let index = (words.len() - 1).to_string();
        let mut args = vec!["__complete", "bash", index.as_str(), "--"];
        args.extend(words);
        let (code, out, err) = self.xxh(&args);
        assert_eq!(code, Some(0), "completion never fails (C-K5)");
        assert_eq!(err, "", "completion is silent (C-K5)");
        out.lines()
            .map(|l| l.split('\t').next().unwrap_or_default().to_string())
            .collect()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Write an executable script through a child `sh`. This process never holds a
/// write descriptor to it, so a concurrent `fork` in another test cannot make
/// the kernel refuse to run it ("text file busy").
fn write_script(path: &std::path::Path, body: &str) {
    use std::io::Write as _;
    let mut child = std::process::Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("sh");
    let mut stdin = child.stdin.take().expect("stdin");
    stdin
        .write_all(format!("#!/bin/sh\n{body}\n").as_bytes())
        .expect("write script");
    drop(stdin);
    assert!(child.wait().expect("sh").success());
}

fn output(cmd: &mut Command) -> (Option<i32>, String, String) {
    let out = cmd.output().expect("run");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A program of the machine running the tests, by absolute path (the client's
/// own `PATH` does not have it).
fn host_programs(name: &str) -> Vec<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .filter(|p| p.is_file())
        .collect()
}

fn host_program(name: &str) -> Option<PathBuf> {
    host_programs(name).into_iter().next()
}

#[test]
fn scripts_and_usage_errors() {
    let c = Client::new("scripts");
    for shell in ["bash", "zsh", "fish"] {
        let (code, out, err) = c.xxh(&["completions", shell]);
        assert_eq!(code, Some(0), "{shell}: {err}");
        assert!(out.contains("__complete"), "{shell}: a stub (C-K1)");
        // Nothing tied to this build or machine (C-K1).
        assert!(!out.contains("/nix/store"), "{shell}");
        assert!(!out.contains(c.root.to_str().unwrap()), "{shell}");
    }

    // An unsupported shell is a usage error that names the supported ones (C-K2).
    let (code, out, err) = c.xxh(&["completions", "tcsh"]);
    assert_eq!(code, Some(2));
    assert_eq!(out, "");
    assert!(err.contains("bash, zsh, fish"), "{err}");
    let (code, _, _) = c.xxh(&["completions"]);
    assert_eq!(code, Some(2));

    // Whatever a stale or foreign stub sends, the prompt sees nothing (C-K5).
    for args in [
        &["__complete"][..],
        &["__complete", "tcsh"],
        &["__complete", "bash", "x", "--", "xxh"],
        &["__complete", "bash", "9", "--", "xxh"],
        &["__complete", "bash", "1", "xxh", "pl"],
        &[
            "__complete",
            "elvish",
            "2",
            "--",
            "xxh",
            "--no-such",
            "--zz",
        ],
    ] {
        let (code, out, err) = c.xxh(args);
        assert_eq!(
            (code, out.as_str(), err.as_str()),
            (Some(0), "", ""),
            "{args:?}"
        );
    }
    // A shell this build has no stub for still gets answers from a newer stub.
    let (_, out, _) = c.xxh(&["__complete", "elvish", "1", "--", "xxh", "pl"]);
    assert!(out.starts_with("plugin\t"), "{out}");

    // A broken config must not surface in the prompt either.
    std::fs::write(c.root.join("config/xxh/config.toml"), "not = [toml").unwrap();
    assert!(c.complete(&["xxh", "w"]).is_empty());
    assert_eq!(c.complete(&["xxh", "plugin", "enable", ""]), [""; 0]);
    assert_eq!(c.complete(&["xxh", "pl"]), ["plugin"]);
}

#[test]
fn targets_and_plugins() {
    let c = Client::new("targets");
    std::fs::write(
        c.root.join("config/xxh/config.toml"),
        "[hosts.web]\nuser = \"deploy\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(c.root.join("home/.ssh/conf.d")).unwrap();
    std::fs::write(
        c.root.join("home/.ssh/config"),
        "Include conf.d/*\nHost web-ssh web\n  IdentityFile ~/.ssh/secret_key\nHost *\n  User nobody\n",
    )
    .unwrap();
    std::fs::write(c.root.join("home/.ssh/conf.d/work"), "Host wiki\n").unwrap();

    // Hosts of both configs, no patterns, no repeats (C-K7).
    assert_eq!(c.complete(&["xxh", "w"]), ["web", "web-ssh", "wiki"]);
    assert_eq!(c.complete(&["xxh", "status", "wi"]), ["wiki"]);
    assert_eq!(c.complete(&["xxh", "doctor", "wi"]), ["wiki"]);
    assert_eq!(c.complete(&["xxh", "clean", "wi"]), ["wiki"]);
    assert_eq!(
        c.complete(&["xxh", "deploy@w"]),
        ["deploy@web", "deploy@web-ssh", "deploy@wiki"]
    );
    let first = c.complete(&["xxh", ""]);
    for want in ["web", "docker:", "container:", "plugin", "completions"] {
        assert!(first.iter().any(|v| v == want), "{want} in {first:?}");
    }
    // Only aliases leave ssh_config: no pattern, user or key path (Принцип V).
    let (_, raw, _) = c.xxh(&["__complete", "bash", "1", "--", "xxh", ""]);
    for leak in ["*", "nobody", "secret_key"] {
        assert!(!raw.contains(leak), "{leak} leaked:\n{raw}");
    }

    // Running containers of a (stand-in) runtime (C-K8).
    c.script(
        "docker",
        "[ \"$1\" = ps ] || { echo exec >> \"$HOME/not-ps\"; exit 1; }\necho app1\necho api",
    );
    assert_eq!(
        c.complete(&["xxh", "docker:"]),
        ["docker:api", "docker:app1"]
    );
    assert_eq!(
        c.complete(&["xxh", "status", "docker:app"]),
        ["docker:app1"]
    );
    assert_eq!(
        c.complete(&["xxh", "container:"]),
        ["container:api", "container:app1"],
        "auto picks the installed runtime"
    );
    assert!(c.complete(&["xxh", "podman:"]).is_empty(), "not installed");
    assert!(
        !c.root.join("home/not-ps").exists(),
        "only `ps` is run (C-K9)"
    );

    // A runtime on another machine is not asked at all (C-K9).
    c.script("docker", "echo called >> \"$HOME/called\"; echo remote1");
    let (code, out, err) = output(
        c.command(XXH)
            .env("DOCKER_HOST", "tcp://10.0.0.5:2376")
            .args(["__complete", "bash", "1", "--", "xxh", "docker:"]),
    );
    assert_eq!((code, out.as_str(), err.as_str()), (Some(0), "", ""));
    assert!(!c.root.join("home/called").exists());

    // A hung runtime costs the budget, not the prompt (SC-003, §FR-006).
    // (A builtin loop: the client's PATH has no `sleep`.)
    c.script("docker", "while :; do :; done");
    let started = Instant::now();
    assert!(c.complete(&["xxh", "docker:"]).is_empty());
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "took {:?}",
        started.elapsed()
    );
    assert!(
        started.elapsed() >= Duration::from_millis(100),
        "it did hang"
    );
    // Hosts never wait for a runtime.
    let started = Instant::now();
    assert_eq!(c.complete(&["xxh", "wik"]), ["wiki"]);
    assert!(started.elapsed() < Duration::from_millis(200));

    // Plugins by state (C-K10).
    let plugin = c.root.join("lp");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        "name = \"lp\"\nversion = \"0.1.0\"\napi_version = \"1.0.0\"\n",
    )
    .unwrap();
    std::fs::write(plugin.join("env.sh"), "export LP=yes\n").unwrap();
    let (code, _, err) = c.xxh(&["plugin", "add", plugin.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{err}");
    let (_, list, _) = c.xxh(&["plugin", "list", "--enabled"]);
    if list.contains("lp") {
        // `plugin add` enabled it; start from the disabled state.
        assert_eq!(c.xxh(&["plugin", "disable", "lp"]).0, Some(0));
    }
    assert_eq!(c.complete(&["xxh", "plugin", "enable", ""]), ["lp"]);
    assert_eq!(c.complete(&["xxh", "plugin", "remove", "l"]), ["lp"]);
    assert_eq!(c.complete(&["xxh", "plugin", "update", ""]), ["lp"]);
    assert!(c.complete(&["xxh", "plugin", "disable", ""]).is_empty());
    assert_eq!(c.xxh(&["plugin", "enable", "lp"]).0, Some(0));
    assert!(c.complete(&["xxh", "plugin", "enable", ""]).is_empty());
    assert_eq!(c.complete(&["xxh", "plugin", "disable", ""]), ["lp"]);
}

/// The bash stub, driven the way bash drives it: `COMP_WORDS` cut at `:`.
#[test]
fn bash_stub() {
    // A bash built without readline (as in the Nix stdenv) has no `complete`.
    let interactive = |bash: &PathBuf| {
        Command::new(bash)
            .args(["--norc", "--noprofile", "-c", "type complete compopt"])
            .output()
            .is_ok_and(|o| o.status.success())
    };
    let Some(bash) = host_programs("bash").into_iter().find(interactive) else {
        eprintln!("skipping: no bash with programmable completion available");
        return;
    };
    let c = Client::new("bash");
    std::fs::write(
        c.root.join("home/.ssh/config"),
        "Host web web-ssh\nHost *\n",
    )
    .unwrap();
    c.script("docker", "echo app1; echo api");
    let script = r#"
eval "$(xxh completions bash)" || exit 1
complete -p xxh
try() {
    COMP_LINE=$1 COMP_POINT=${#1} COMP_WORDS=()
    local w parts
    read -ra parts <<<"$1"
    for w in "${parts[@]}"; do
        while [[ $w == *:* ]]; do
            [[ -n ${w%%:*} ]] && COMP_WORDS+=("${w%%:*}")
            COMP_WORDS+=(":")
            w=${w#*:}
        done
        [[ -n $w ]] && COMP_WORDS+=("$w")
    done
    [[ $1 == *" " ]] && COMP_WORDS+=("")
    COMP_CWORD=$((${#COMP_WORDS[@]} - 1))
    _xxh
    echo "$1=> ${COMPREPLY[*]}"
}
try "xxh pl"
try "xxh --transport "
try "xxh w"
try "xxh deploy@w"
try "xxh docker:"
try "xxh status docker:app"
try "xxh docker:api "
try "xxh nothing-like-this"
PATH=/nonexistent
try "xxh pl"
"#;
    let (code, out, err) = output(
        c.command(&bash)
            .args(["--norc", "--noprofile", "-c", script]),
    );
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(err, "", "the stub prints nothing of its own");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines,
        [
            "complete -F _xxh xxh",
            "xxh pl=> plugin",
            "xxh --transport => russh ssh",
            "xxh w=> web web-ssh",
            "xxh deploy@w=> deploy@web deploy@web-ssh",
            // Bash replaces only what follows the `:`.
            "xxh docker:=> api app1",
            "xxh status docker:app=> app1",
            // A finished target is a word of its own, not glued to the next.
            "xxh docker:api => config plugin shell sync status doctor completions man clean help",
            "xxh nothing-like-this=> ",
            // No xxh to ask (or one that fails): no candidates, no noise.
            "xxh pl=> ",
        ]
    );
}

#[test]
fn zsh_stub() {
    let Some(zsh) = host_program("zsh") else {
        eprintln!("skipping: zsh not available");
        return;
    };
    let c = Client::new("zsh");
    let (_, stub, _) = c.xxh(&["completions", "zsh"]);
    let file = c.root.join("_xxh");
    std::fs::write(&file, stub).unwrap();

    let (code, _, err) = output(c.command(&zsh).arg("-n").arg(&file));
    assert_eq!((code, err.as_str()), (Some(0), ""), "syntax");

    // Sourced after compinit it registers itself; before compinit it stays quiet.
    let script = format!(
        "source {f}; print before=$?; autoload -Uz compinit && compinit -u -D; \
         source {f}; print -r -- registered=$_comps[xxh]",
        f = file.display()
    );
    let (code, out, err) = output(c.command(&zsh).args(["-f", "-c", &script]));
    assert_eq!(code, Some(0), "{err}");
    assert!(out.contains("before=0"), "{out}\n{err}");
    assert!(out.contains("registered=_xxh"), "{out}\n{err}");
}

#[test]
fn fish_stub() {
    let Some(fish) = host_program("fish") else {
        eprintln!("skipping: fish not available");
        return;
    };
    let c = Client::new("fish");
    std::fs::write(c.root.join("home/.ssh/config"), "Host web\n").unwrap();
    c.script("docker", "echo app1");
    let script = "xxh completions fish | source; \
                  complete -C 'xxh pl'; echo ===; complete -C 'xxh w'; echo ===; \
                  complete -C 'xxh docker:'; echo ===; complete -C 'xxh --transport '";
    let (code, out, err) = output(c.command(&fish).args(["--no-config", "-c", script]));
    assert_eq!(code, Some(0), "{err}");
    let parts: Vec<Vec<&str>> = out
        .split("===\n")
        .map(|p| {
            p.lines()
                .map(|l| l.split('\t').next().unwrap_or_default())
                .collect()
        })
        .collect();
    assert_eq!(parts[0], ["plugin"], "{out}\n{err}");
    assert_eq!(parts[1], ["web"], "{out}");
    assert_eq!(parts[2], ["docker:app1"], "{out}");
    assert_eq!(parts[3], ["russh", "ssh"], "{out}");
}

#[test]
fn man_pages() {
    let c = Client::new("man");
    let (code, page, err) = c.xxh(&["man"]);
    assert_eq!(code, Some(0), "{err}");
    assert!(page.contains(".TH xxh 1"), "roff (C-K13)");

    let dir = c.root.join("out/man1");
    let (code, out, err) = c.xxh(&["man", "--dir", dir.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{err}");
    assert!(out.contains("manual pages"), "{out}");
    for name in [
        "xxh.1",
        "xxh-plugin.1",
        "xxh-plugin-add.1",
        "xxh-shell-fetch.1",
        "xxh-config-show.1",
        "xxh-sync.1",
        "xxh-status.1",
        "xxh-doctor.1",
        "xxh-clean.1",
        "xxh-completions.1",
        "xxh-man.1",
    ] {
        assert!(dir.join(name).is_file(), "{name} (C-K14)");
    }
    assert_eq!(std::fs::read_to_string(dir.join("xxh.1")).unwrap(), page);
    assert!(!dir.join("xxh-__complete.1").exists());

    // Read with the real viewer where there is one that takes a file.
    let viewed = host_program("man").map(|man| {
        output(
            c.command(man)
                .env("MANWIDTH", "200")
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .arg("-l")
                .arg(dir.join("xxh.1")),
        )
    });
    match viewed {
        Some((Some(0), text, _)) => {
            for want in [
                "EXIT STATUS",
                "FILES",
                "config.toml",
                "--transport",
                "doctor",
            ] {
                assert!(text.contains(want), "{want} in:\n{text}");
            }
        }
        _ => eprintln!("skipping: no `man -l` to read the page with"),
    }

    // A directory that cannot be: usage error with the path, nothing else (C-K16).
    let blocker = c.root.join("file");
    std::fs::write(&blocker, "").unwrap();
    let target = blocker.join("man1");
    let (code, out, err) = c.xxh(&["man", "--dir", target.to_str().unwrap()]);
    assert_eq!(code, Some(2));
    assert_eq!(out, "");
    assert!(err.starts_with("xxh: man: "), "{err}");
    assert!(err.contains(target.to_str().unwrap()), "{err}");
}
