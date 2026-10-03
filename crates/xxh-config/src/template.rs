//! The starting config written by `xxh config init` (009 T007, research R5).
//!
//! Every active line is the built-in default, so the file changes nothing until
//! the user edits it (§FR-005); the tests below keep it in step with the types.

/// A commented config equal to having no config at all.
pub const TEMPLATE: &str = r#"# xxh configuration. Every value below is the built-in default: this file
# changes nothing until you edit it. Check it with `xxh config validate`.
# Precedence: command-line flag > [hosts.<name>] > the global value > default.

# Shell started on the target. It must be installed as a shell package
# (`xxh shell add …`, see `xxh shell list`).
default_shell = "zsh"

# Plugins loaded into every session, in this order (`xxh plugin list`).
enabled_plugins = []

# What happens to the environment on the target when the session ends:
#   "ephemeral" - everything is removed, the target is left as it was;
#   "keep"      - the environment stays for a faster next login (as `--keep`).
cleanup = "ephemeral"

# SSH client: "russh" (built in) or "ssh" (your system's ssh).
transport = "russh"

# How long to wait for the target to answer, in seconds.
connect_timeout_s = 10

# Login user and private key for every host. Unset: ~/.ssh/config decides.
# user = "deploy"
# identity = "~/.ssh/id_ed25519"

[container]
# Runtime behind `container:<name>` targets: "auto" (docker, then podman),
# "docker" or "podman".
runtime = "auto"

# Per-host overrides. The name is what you type: an SSH alias or host name, or
# a container name. Any of these keys can be left out.
# [hosts.web]
# default_shell = "fish"
# enabled_plugins = ["prompt"]
# cleanup = "keep"
# transport = "ssh"
# connect_timeout_s = 30
# user = "www"
# identity = "~/.ssh/web_key"
# container_runtime = "podman"

# Plugins and shell packages that `xxh sync` installs, by name; the source is
# what `xxh plugin add` / `xxh shell add` accept.
# [plugins.prompt]
# source = "https://example.org/xxh-plugin-prompt.git"
# [shells.zsh]
# source = "https://example.org/xxh-shell-zsh.git"
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{self, Kind};
    use crate::{Config, validate};

    #[test]
    fn template_is_the_defaults() {
        let parsed: Config = toml::from_str(TEMPLATE).expect("the template parses");
        assert_eq!(parsed, Config::default());
        assert_eq!(validate::check(TEMPLATE), validate::Report::default());
    }

    /// With the examples uncommented the template is still a valid config
    /// without unknown keys — the examples cannot rot.
    #[test]
    fn commented_examples_are_valid_too() {
        let lines: Vec<&str> = TEMPLATE
            .lines()
            .map(|l| match l.strip_prefix("# ") {
                Some(rest) if rest.starts_with('[') || rest.contains(" = ") => rest,
                _ => l,
            })
            .collect();
        let text = lines.join("\n");
        assert!(
            text.contains("\n[hosts.web]\n"),
            "examples were uncommented"
        );
        // `user`/`identity` examples sit above [container]; as active lines they
        // would be read as globals, which is what they document.
        assert_eq!(
            validate::check(&text),
            validate::Report::default(),
            "{text}"
        );
    }

    /// A new setting must be introduced to the template as well.
    #[test]
    fn template_mentions_every_key() {
        let mentioned = |key: &str| {
            TEMPLATE
                .lines()
                .any(|l| l.trim_start_matches("# ").starts_with(&format!("{key} = ")))
        };
        let Ok(Kind::Table(top)) = keys::lookup(&[]) else {
            panic!("the config is a table");
        };
        for key in top {
            match keys::lookup(&[key.clone()]) {
                Ok(Kind::Table(_) | Kind::Map) => {
                    assert!(TEMPLATE.contains(&format!("[{key}")), "[{key}] section")
                }
                _ => assert!(mentioned(&key), "{key}"),
            }
        }
        let Ok(Kind::Table(host)) = keys::lookup(&["hosts".into(), "web".into()]) else {
            panic!("a host is a table");
        };
        for key in host {
            assert!(mentioned(&key), "hosts.<name>.{key}");
        }
    }
}
