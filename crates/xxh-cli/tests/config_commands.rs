//! Integration (009 T006/T009/T013): the **real `xxh` binary** — `xxh config
//! validate|init|get|set|unset|edit` (contracts/config-commands.md C-G*).
//!
//! No target is involved: these commands only read and write the client's
//! config file, so each test runs against its own isolated config directory.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const XXH: &str = env!("CARGO_BIN_EXE_xxh");

/// An isolated client: its own config directory, registry and shells.
struct Client {
    root: PathBuf,
}

impl Client {
    fn new(tag: &str) -> Client {
        let root = std::env::temp_dir().join(format!(
            "xxh-config-cmd-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&root).unwrap();
        Client { root }
    }

    fn config(&self) -> PathBuf {
        self.root.join("config/xxh/config.toml")
    }

    fn write_config(&self, text: &str) {
        std::fs::create_dir_all(self.config().parent().unwrap()).unwrap();
        std::fs::write(self.config(), text).unwrap();
    }

    fn read_config(&self) -> String {
        std::fs::read_to_string(self.config()).unwrap()
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(XXH);
        cmd.env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XXH_PLUGINS_DIR", self.root.join("plugins"))
            .env("XXH_SHELLS_DIR", self.root.join("shells"))
            .env_remove("VISUAL")
            .env_remove("EDITOR")
            // Never a terminal: `edit` must not wait for an answer.
            .stdin(Stdio::null());
        cmd
    }

    fn xxh(&self, args: &[&str]) -> (Option<i32>, String, String) {
        output(self.command().args(args))
    }

    /// `xxh config edit` with a script as the editor; `$1` is the file to edit.
    fn edit_with(&self, body: &str) -> (Option<i32>, String, String) {
        let editor = self.root.join("editor.sh");
        std::fs::write(&editor, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();
        output(
            self.command()
                .env("EDITOR", &editor)
                .args(["config", "edit"]),
        )
    }

    /// Files in the config directory, to catch leftovers.
    fn config_dir_entries(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.config().parent().unwrap())
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn output(cmd: &mut Command) -> (Option<i32>, String, String) {
    let out = cmd.output().expect("run xxh");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

#[test]
fn validate() {
    let c = Client::new("validate");

    // No config is a valid setup (C-G3).
    let (code, out, err) = c.xxh(&["config", "validate"]);
    assert_eq!(code, Some(0), "{err}");
    assert!(out.contains("built-in defaults"), "{out}");

    c.write_config("default_shell = \"fish\"\n\n[hosts.web]\ncleanup = \"keep\"\n");
    let (code, out, err) = c.xxh(&["config", "validate"]);
    assert_eq!((code, err.as_str()), (Some(0), ""));
    assert_eq!(out.trim(), format!("{}: ok", c.config().display()));

    // A bad value: config class, with the key, the value, the choices and the
    // place (C-G4, §FR-002).
    c.write_config("default_shell = \"fish\"\n\n[hosts.web]\ncleanup = \"sometimes\"\n");
    let (code, out, err) = c.xxh(&["config", "validate"]);
    assert_eq!(code, Some(40));
    assert_eq!(out, "");
    assert!(err.starts_with("xxh: config: invalid config"), "{err}");
    assert!(err.contains(c.config().to_str().unwrap()), "{err}");
    assert!(err.contains("line 4"), "{err}");
    assert!(err.contains("cleanup = \"sometimes\""), "{err}");
    assert!(
        err.contains("`ephemeral`") && err.contains("`keep`"),
        "{err}"
    );
    // What validate refuses is what a login would have refused (SC-001).
    let (code, _, _) = c.xxh(&["config", "show"]);
    assert_eq!(code, Some(40));

    // A typo is a warning with the nearest key; strict mode fails on it (C-G5).
    c.write_config("# mine\ndefualt_shell = \"fish\"\n");
    let (code, out, err) = c.xxh(&["config", "validate"]);
    assert_eq!(code, Some(0), "{err}");
    assert!(out.contains("ok (1 warning(s))"), "{out}");
    assert_eq!(
        err.trim(),
        format!(
            "warning: {}:2: unknown key `defualt_shell` (did you mean `default_shell`?)",
            c.config().display()
        )
    );
    let (code, out, err) = c.xxh(&["config", "validate", "--strict"]);
    assert_eq!(code, Some(40));
    assert_eq!(out, "");
    assert!(err.contains("--strict"), "{err}");

    // A given file is checked instead of the default one (C-G3).
    let other = c.root.join("other.toml");
    write(&other, "transport = \"telnet\"\n");
    let (code, _, err) = c.xxh(&["config", "validate", other.to_str().unwrap()]);
    assert_eq!(code, Some(40));
    assert!(
        err.contains("other.toml") && err.contains("telnet"),
        "{err}"
    );
    assert!(err.contains("`russh`") && err.contains("`ssh`"), "{err}");
    write(&other, "transport = \"ssh\"\n");
    let (code, out, _) = c.xxh(&["config", "validate", "--strict", other.to_str().unwrap()]);
    assert_eq!(code, Some(0));
    assert!(out.contains("other.toml: ok"), "{out}");
    let (code, _, err) = c.xxh(&["config", "validate", "/nonexistent/x.toml"]);
    assert_eq!(code, Some(40));
    assert!(err.contains("/nonexistent/x.toml"), "{err}");

    // A source `plugin add` would refuse is an error here too.
    write(&other, "[plugins.broken]\nsource = \"nixpkgs:\"\n");
    let (code, _, err) = c.xxh(&["config", "validate", other.to_str().unwrap()]);
    assert_eq!(code, Some(40));
    assert!(err.contains("plugins.broken.source"), "{err}");
}

#[test]
fn init() {
    let c = Client::new("init");
    let (_, defaults, _) = c.xxh(&["config", "show"]);

    // Creates the directory and a commented file (C-G7).
    let (code, out, err) = c.xxh(&["config", "init"]);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(out.trim(), c.config().to_str().unwrap());
    let text = c.read_config();
    assert!(text.lines().filter(|l| l.starts_with('#')).count() > 10);

    // It is valid, warning-free, and changes nothing (§FR-005).
    let (code, out, err) = c.xxh(&["config", "validate", "--strict"]);
    assert_eq!((code, err.as_str()), (Some(0), ""), "{out}");
    assert_eq!(c.xxh(&["config", "show"]).1, defaults);

    // An existing file is kept unless asked otherwise (C-G8).
    c.write_config("default_shell = \"fish\"\n");
    let (code, out, err) = c.xxh(&["config", "init"]);
    assert_eq!(code, Some(40));
    assert_eq!(out, "");
    assert!(
        err.contains("already exists") && err.contains("--force"),
        "{err}"
    );
    assert_eq!(c.read_config(), "default_shell = \"fish\"\n");
    assert_eq!(c.xxh(&["config", "init", "--force"]).0, Some(0));
    assert_eq!(c.read_config(), text);

    // A config managed elsewhere is never replaced (C-G15).
    let managed = c.root.join("store-config.toml");
    write(&managed, "default_shell = \"sh\"\n");
    std::fs::remove_file(c.config()).unwrap();
    std::os::unix::fs::symlink(&managed, c.config()).unwrap();
    let (code, _, err) = c.xxh(&["config", "init", "--force"]);
    assert_eq!(code, Some(40));
    assert!(err.contains("managed elsewhere"), "{err}");
    assert_eq!(
        std::fs::read_to_string(&managed).unwrap(),
        "default_shell = \"sh\"\n"
    );
    assert!(c.config().is_symlink());
}

const COMMENTED: &str = "\
# My xxh config.
default_shell = \"zsh\"   # the one I like

# plugins, in order
enabled_plugins = []

[hosts.web]
# web is slow
connect_timeout_s = 30
";

#[test]
fn get_set_unset() {
    let c = Client::new("set");
    c.write_config(COMMENTED);

    // The story of the spec: set for a host, see it in `show` and `get`.
    let (code, out, err) = c.xxh(&["config", "set", "hosts.web.default_shell", "fish"]);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(out.trim(), "hosts.web.default_shell = fish");
    let (_, shown, _) = c.xxh(&["config", "show", "--host", "web"]);
    assert!(shown.contains("shell             = fish"), "{shown}");
    let (code, out, _) = c.xxh(&["config", "get", "hosts.web.default_shell"]);
    assert_eq!((code, out.trim()), (Some(0), "fish"));
    // Only that line was added (C-G11, SC-003).
    assert_eq!(
        c.read_config(),
        format!("{COMMENTED}default_shell = \"fish\"\n")
    );
    let (code, out, _) = c.xxh(&["config", "unset", "hosts.web.default_shell"]);
    assert_eq!(
        (code, out.trim()),
        (Some(0), "hosts.web.default_shell unset")
    );
    assert_eq!(c.read_config(), COMMENTED);

    // A value in place: the comment after it stays.
    assert_eq!(
        c.xxh(&["config", "set", "default_shell", "fish"]).0,
        Some(0)
    );
    assert_eq!(c.read_config(), COMMENTED.replace("\"zsh\"", "\"fish\""));
    c.write_config(COMMENTED);

    // Refusals leave the file byte for byte (C-G12, §FR-007).
    for (args, want) in [
        (
            &["config", "set", "cleanup", "sometimes"][..],
            "`ephemeral`, `keep`",
        ),
        (
            &["config", "set", "connect_timeout_s", "soon"],
            "whole number",
        ),
        (
            &["config", "set", "defualt_shell", "fish"],
            "did you mean `default_shell`",
        ),
        (&["config", "set", "hosts.web", "x"], "hosts.web.cleanup"),
        (&["config", "unset", "nope"], "unknown config key"),
        (
            &["config", "get", "cleanup"],
            "not set (default: ephemeral)",
        ),
        (&["config", "get", "hosts.web"], "is a table"),
    ] {
        let (code, out, err) = c.xxh(args);
        assert_eq!(code, Some(40), "{args:?}");
        assert_eq!(out, "", "{args:?}");
        assert!(err.starts_with("xxh: config: "), "{err}");
        assert!(err.contains(want), "{args:?}: {err}");
        assert_eq!(c.read_config(), COMMENTED, "{args:?}");
    }
    assert_eq!(c.config_dir_entries(), ["config.toml"], "no leftovers");

    // Lists, and a config created by the first `set` (C-G10).
    assert_eq!(
        c.xxh(&["config", "set", "enabled_plugins", "a,b"]).0,
        Some(0)
    );
    let (_, out, _) = c.xxh(&["config", "get", "enabled_plugins"]);
    assert_eq!(out, "a\nb\n");
    std::fs::remove_file(c.config()).unwrap();
    assert_eq!(
        c.xxh(&["config", "set", "container.runtime", "podman"]).0,
        Some(0)
    );
    assert_eq!(c.read_config(), "[container]\nruntime = \"podman\"\n");

    // A managed config is refused before anything changes (C-G15, §FR-009).
    let managed = c.root.join("store-config.toml");
    write(&managed, COMMENTED);
    std::fs::remove_file(c.config()).unwrap();
    std::os::unix::fs::symlink(&managed, c.config()).unwrap();
    for args in [
        &["config", "set", "default_shell", "fish"][..],
        &["config", "unset", "default_shell"],
    ] {
        let (code, _, err) = c.xxh(args);
        assert_eq!(code, Some(40), "{args:?}");
        assert!(err.contains("managed elsewhere"), "{err}");
        assert!(err.contains("store-config.toml"), "names the source: {err}");
    }
    assert_eq!(std::fs::read_to_string(&managed).unwrap(), COMMENTED);
    assert!(c.config().is_symlink());
    // Reading works as ever.
    assert_eq!(c.xxh(&["config", "get", "default_shell"]).1.trim(), "zsh");

    // Keys are completed, with the hosts the config names (C-G20).
    let (code, out, err) = c.xxh(&[
        "__complete",
        "bash",
        "3",
        "--",
        "xxh",
        "config",
        "set",
        "hosts.w",
    ]);
    assert_eq!((code, err.as_str()), (Some(0), ""));
    assert!(out.lines().any(|l| l == "hosts.web.cleanup"), "{out}");
    let (_, out, _) = c.xxh(&[
        "__complete",
        "bash",
        "3",
        "--",
        "xxh",
        "config",
        "get",
        "def",
    ]);
    assert_eq!(out, "default_shell\n");
}

#[test]
fn edit() {
    let c = Client::new("edit");
    c.write_config(COMMENTED);

    // No editor: say how to set one (C-G16).
    let (code, _, err) = c.xxh(&["config", "edit"]);
    assert_eq!(code, Some(40));
    assert!(err.contains("$EDITOR"), "{err}");

    // A valid edit is saved (C-G17).
    let (code, out, err) = c.edit_with("printf 'default_shell = \"fish\"\\n# kept\\n' > \"$1\"");
    assert_eq!(code, Some(0), "{err}");
    assert!(out.contains("saved"), "{out}");
    assert_eq!(c.read_config(), "default_shell = \"fish\"\n# kept\n");

    // The editor sees the current config; leaving it as is saves nothing.
    let seen = c.root.join("seen");
    let (code, out, _) = c.edit_with(&format!("cat \"$1\" > {}", seen.display()));
    assert_eq!(code, Some(0));
    assert!(out.contains("unchanged"), "{out}");
    assert_eq!(std::fs::read_to_string(&seen).unwrap(), c.read_config());

    // A broken edit is shown and not saved; nothing is left behind.
    let (code, out, err) = c.edit_with("printf 'cleanup = \"sometimes\"\\n' > \"$1\"");
    assert_eq!(code, Some(40));
    assert_eq!(out, "");
    assert!(err.contains("`ephemeral`"), "the error is shown: {err}");
    assert!(err.contains("not saved"), "{err}");
    assert_eq!(c.read_config(), "default_shell = \"fish\"\n# kept\n");

    // A failing editor changes nothing either (C-G18).
    let (code, _, err) = c.edit_with("printf 'default_shell = \"sh\"\\n' > \"$1\"; exit 3");
    assert_eq!(code, Some(40));
    assert!(err.contains("unchanged"), "{err}");
    assert_eq!(c.read_config(), "default_shell = \"fish\"\n# kept\n");
    assert_eq!(
        c.config_dir_entries(),
        ["config.toml"],
        "no working copy left"
    );

    // Without a config the editor starts from the `init` template.
    std::fs::remove_file(c.config()).unwrap();
    let (code, _, _) = c.edit_with(&format!("cat \"$1\" > {}; echo >> \"$1\"", seen.display()));
    assert_eq!(code, Some(0));
    let template = std::fs::read_to_string(&seen).unwrap();
    assert!(template.contains("# xxh configuration."), "{template}");
    assert_eq!(c.read_config(), format!("{template}\n"));
}

/// `plugin enable|disable` change the config the same way: one list, the rest
/// kept (C-G19).
#[test]
fn plugin_enable_keeps_the_file() {
    let c = Client::new("plugin");
    c.write_config(COMMENTED);
    let plugin = c.root.join("lp");
    std::fs::create_dir_all(&plugin).unwrap();
    write(
        &plugin.join("plugin.toml"),
        "name = \"lp\"\nversion = \"0.1.0\"\napi_version = \"1.0.0\"\n",
    );
    write(&plugin.join("env.sh"), "export LP=yes\n");
    let (code, _, err) = c.xxh(&["plugin", "add", plugin.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(c.xxh(&["plugin", "disable", "lp"]).0, Some(0));
    assert_eq!(c.read_config(), COMMENTED);

    assert_eq!(c.xxh(&["plugin", "enable", "lp"]).0, Some(0));
    assert_eq!(
        c.read_config(),
        COMMENTED.replace("enabled_plugins = []", "enabled_plugins = [\"lp\"]")
    );
    assert_eq!(c.xxh(&["plugin", "disable", "lp"]).0, Some(0));
    assert_eq!(c.read_config(), COMMENTED);

    // A managed config: the reason, not a bare I/O error.
    let managed = c.root.join("store-config.toml");
    write(&managed, COMMENTED);
    std::fs::remove_file(c.config()).unwrap();
    std::os::unix::fs::symlink(&managed, c.config()).unwrap();
    let (code, _, err) = c.xxh(&["plugin", "enable", "lp"]);
    assert_eq!(code, Some(40));
    assert!(err.contains("managed elsewhere"), "{err}");
    // Nothing to change is not a reason to refuse.
    assert_eq!(c.xxh(&["plugin", "disable", "lp"]).0, Some(0));
    assert_eq!(std::fs::read_to_string(&managed).unwrap(), COMMENTED);
}
