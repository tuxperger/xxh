//! `xxh __complete` — the answering side of shell completion (007 T002/T008/T009,
//! contracts/completions-and-man.md C-K4..C-K12).
//!
//! The scripts printed by `xxh completions` are thin stubs: on Tab they pass the
//! command line here and print what comes back. Subcommands, flags and enumerated
//! values come from the same clap tree that parses arguments, so the two cannot
//! drift (SC-002); targets, containers, plugins and shells are looked up live.
//!
//! Completion runs inside the user's prompt: it never fails visibly, never opens a
//! connection, and bounds the one external call it makes (§FR-006, C-K5/C-K9).

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::time::Duration;

use clap_complete::engine::CompletionCandidate;
use xxh_config::{Config, RuntimeSetting};
use xxh_transport::ContainerRuntime;

/// How long a container runtime may take to list its containers (C-K9); the
/// whole answer has to stay under 200 ms (SC-003).
const RUNTIME_BUDGET: Duration = Duration::from_millis(150);

/// Target schemes offered before anything is typed, with what they select.
const SCHEMES: [(&str, &str); 4] = [
    ("container:", "running container (docker or podman)"),
    ("docker:", "running docker container"),
    ("podman:", "running podman container"),
    ("ssh:", "SSH host"),
];

/// One completion: the text to insert and an optional description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub value: String,
    pub help: Option<String>,
}

impl Candidate {
    fn plain(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            help: None,
        }
    }
}

/// `xxh __complete <shell> <index> -- <words…>` (C-K4). Every failure — bad
/// arguments, an unreadable config, a panic — is an empty answer, exit 0 and a
/// silent stderr (C-K5): the caller is the user's prompt.
pub fn run(args: &[OsString], cmd: fn() -> clap::Command) {
    std::panic::set_hook(Box::new(|_| {}));
    let answer = std::panic::catch_unwind(move || {
        let mut cmd = cmd();
        // `<shell>` is part of the protocol for the stubs' sake; the answer is
        // the same for every shell.
        let index = args.get(1)?.to_str()?.parse::<usize>().ok()?;
        if args.get(2)? != "--" {
            return None;
        }
        Some(render(&candidates(&mut cmd, args[3..].to_vec(), index)))
    });
    if let Ok(Some(text)) = answer {
        print!("{text}");
    }
}

/// The candidates for word `index` of `words` (the command line, starting with
/// the command name).
pub fn candidates(cmd: &mut clap::Command, words: Vec<OsString>, index: usize) -> Vec<Candidate> {
    let Some(current) = words.get(index).map(|w| w.to_string_lossy().into_owned()) else {
        return Vec::new();
    };
    let Ok(found) = clap_complete::engine::complete(cmd, words, index, None) else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    found
        .into_iter()
        .filter(|c| !c.is_hide_set())
        .map(|c| Candidate {
            value: c.get_value().to_string_lossy().into_owned(),
            help: c
                .get_help()
                .map(|h| h.to_string())
                .and_then(|h| h.lines().next().map(|l| l.replace('\t', " ")))
                .filter(|h| !h.is_empty()),
        })
        // The engine offers flags wherever one may stand; listing them next to
        // hosts and subcommands is noise until the user types a dash.
        .filter(|c| current.starts_with('-') || !c.value.starts_with('-'))
        .filter(|c| !c.value.contains(['\n', '\t']) && seen.insert(c.value.clone()))
        .collect()
}

/// The wire format (C-K4): `value` or `value<TAB>description`, one per line.
pub fn render(candidates: &[Candidate]) -> String {
    let mut out = String::new();
    for c in candidates {
        out.push_str(&c.value);
        if let Some(help) = &c.help {
            out.push('\t');
            out.push_str(help);
        }
        out.push('\n');
    }
    out
}

fn to_engine(candidates: Vec<Candidate>) -> Vec<CompletionCandidate> {
    candidates
        .into_iter()
        .map(|c| CompletionCandidate::new(c.value).help(c.help.map(Into::into)))
        .collect()
}

/// The runtime a container scheme names; `container:` follows the config and,
/// on `auto`, the first installed runtime (C-K8).
fn containers(scheme: &str) -> Vec<String> {
    let setting = match scheme {
        "docker" => RuntimeSetting::Docker,
        "podman" => RuntimeSetting::Podman,
        _ => Config::load_default().map_or(RuntimeSetting::Auto, |c| c.container.runtime),
    };
    match setting {
        RuntimeSetting::Docker => {
            xxh_transport::running_containers(ContainerRuntime::Docker, RUNTIME_BUDGET)
        }
        RuntimeSetting::Podman => {
            xxh_transport::running_containers(ContainerRuntime::Podman, RUNTIME_BUDGET)
        }
        RuntimeSetting::Auto => xxh_transport::running_containers_auto(RUNTIME_BUDGET),
    }
}

/// Hosts the user has named: `[hosts.*]` of the xxh config and the aliases of
/// `~/.ssh/config` (C-K7).
fn known_hosts() -> Vec<String> {
    let mut hosts: Vec<String> = Config::load_default()
        .map(|c| c.hosts.into_keys().collect())
        .unwrap_or_default();
    hosts.extend(xxh_transport::ssh_config_extra::user_host_aliases());
    hosts
}

/// Candidates for a target word (C-K7/C-K8). `hosts` and `containers` are asked
/// only when the word calls for them, so a host never costs a runtime call.
fn target_candidates(
    current: &str,
    hosts: &dyn Fn() -> Vec<String>,
    containers: &dyn Fn(&str) -> Vec<String>,
) -> Vec<Candidate> {
    if let Some((scheme, rest)) = current.split_once(':') {
        if matches!(scheme, "docker" | "podman" | "container") {
            return containers(scheme)
                .into_iter()
                .filter(|name| name.starts_with(rest))
                .map(|name| Candidate::plain(format!("{scheme}:{name}")))
                .collect();
        }
    }
    // `ssh:` and `user@` are kept as typed; the host after them is completed.
    let (scheme, rest) = match current.strip_prefix("ssh:") {
        Some(rest) => ("ssh:", rest),
        None => ("", current),
    };
    let (user, partial) = match rest.rfind('@') {
        Some(at) => rest.split_at(at + 1),
        None => ("", rest),
    };
    let names: BTreeSet<String> = hosts()
        .into_iter()
        .filter(|h| h.starts_with(partial))
        .collect();
    let mut out: Vec<Candidate> = names
        .into_iter()
        .map(|h| Candidate::plain(format!("{scheme}{user}{h}")))
        .collect();
    if scheme.is_empty() && user.is_empty() {
        out.extend(
            SCHEMES
                .iter()
                .filter(|(s, _)| s.starts_with(current))
                .map(|(s, help)| Candidate {
                    value: (*s).to_string(),
                    help: Some((*help).to_string()),
                }),
        );
    }
    out
}

/// Which plugins a `xxh plugin …` argument can name (C-K10).
#[derive(Debug, Clone, Copy)]
enum PluginState {
    Installed,
    /// Installed and not in `enabled_plugins`.
    Disabled,
    Enabled,
}

fn plugin_candidates(
    current: &str,
    installed: &[String],
    enabled: &[String],
    state: PluginState,
) -> Vec<Candidate> {
    let pool: Vec<&String> = match state {
        PluginState::Installed => installed.iter().collect(),
        PluginState::Disabled => installed.iter().filter(|p| !enabled.contains(p)).collect(),
        PluginState::Enabled => enabled.iter().collect(),
    };
    let names: BTreeSet<&String> = pool
        .into_iter()
        .filter(|p| p.starts_with(current))
        .collect();
    names.into_iter().map(Candidate::plain).collect()
}

fn plugins(current: &OsStr, state: PluginState) -> Vec<CompletionCandidate> {
    let installed: Vec<String> = xxh_plugins::registry::Registry::open_default()
        .and_then(|r| r.list())
        .map(|l| l.into_keys().collect())
        .unwrap_or_default();
    let enabled = Config::load_default()
        .map(|c| c.enabled_plugins)
        .unwrap_or_default();
    to_engine(plugin_candidates(
        &current.to_string_lossy(),
        &installed,
        &enabled,
        state,
    ))
}

/// Completer for a target argument.
pub fn targets(current: &OsStr) -> Vec<CompletionCandidate> {
    to_engine(target_candidates(
        &current.to_string_lossy(),
        &known_hosts,
        &containers,
    ))
}

/// Completer for `plugin remove|update`.
pub fn installed_plugins(current: &OsStr) -> Vec<CompletionCandidate> {
    plugins(current, PluginState::Installed)
}

/// Completer for `plugin enable`.
pub fn disabled_plugins(current: &OsStr) -> Vec<CompletionCandidate> {
    plugins(current, PluginState::Disabled)
}

/// Completer for `plugin disable`.
pub fn enabled_plugins(current: &OsStr) -> Vec<CompletionCandidate> {
    plugins(current, PluginState::Enabled)
}

/// Completer for a shell name: the installed shell packages.
pub fn shells(current: &OsStr) -> Vec<CompletionCandidate> {
    let current = current.to_string_lossy();
    let names: BTreeSet<String> = xxh_core::shellmgr::list()
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.shell)
        .filter(|s| s.starts_with(&*current))
        .collect();
    to_engine(names.into_iter().map(Candidate::plain).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    fn values(words: &[&str]) -> Vec<String> {
        let words: Vec<OsString> = words.iter().map(OsString::from).collect();
        let index = words.len() - 1;
        candidates(&mut crate::Cli::command(), words, index)
            .into_iter()
            .map(|c| c.value)
            .collect()
    }

    fn has(words: &[&str], value: &str) -> bool {
        values(words).iter().any(|v| v == value)
    }

    #[test]
    fn subcommands_flags_and_values_come_from_the_cli() {
        assert_eq!(values(&["xxh", "pl"]), ["plugin"]);
        assert!(has(&["xxh", "plugin", ""], "enable"));
        assert!(has(&["xxh", "--tr"], "--transport"));
        // Global flags are offered after a subcommand too (C-K6).
        assert!(has(&["xxh", "status", "web", "--"], "--verbose"));
        assert!(has(&["xxh", "status", "web", "--"], "--json"));
        assert_eq!(values(&["xxh", "--transport", ""]), ["russh", "ssh"]);
        assert_eq!(values(&["xxh", "--runtime", "p"]), ["podman"]);
        assert_eq!(values(&["xxh", "completions", ""]), ["bash", "zsh", "fish"]);
    }

    #[test]
    fn flags_stay_out_until_a_dash_is_typed() {
        let first = values(&["xxh", ""]);
        assert!(first.iter().any(|v| v == "plugin"));
        assert!(first.iter().all(|v| !v.starts_with('-')), "{first:?}");
        assert!(has(&["xxh", "-"], "--keep"));
    }

    #[test]
    fn nothing_is_offered_where_nothing_fits() {
        // Out of range, the value of `-c`, and a command after `--` (C-K11).
        let words = vec![OsString::from("xxh")];
        assert!(candidates(&mut crate::Cli::command(), words, 5).is_empty());
        assert!(values(&["xxh", "web", "-c", ""]).is_empty());
        assert!(values(&["xxh", "web", "--", "l"]).is_empty());
        assert!(values(&["xxh", "--no-such-flag", "--zz"]).is_empty());
    }

    /// SC-002: every visible subcommand and every long flag is offered.
    #[test]
    fn the_whole_command_tree_is_completable() {
        fn walk(cmd: &clap::Command, path: &mut Vec<String>) {
            for arg in cmd.get_arguments().filter(|a| !a.is_hide_set()) {
                let Some(long) = arg.get_long() else { continue };
                let mut words: Vec<&str> = path.iter().map(String::as_str).collect();
                words.push("--");
                let flag = format!("--{long}");
                assert!(has(&words, &flag), "{flag} after {path:?}");
            }
            for sub in cmd.get_subcommands().filter(|s| !s.is_hide_set()) {
                let mut words: Vec<&str> = path.iter().map(String::as_str).collect();
                words.push("");
                assert!(
                    has(&words, sub.get_name()),
                    "{} after {path:?}",
                    sub.get_name()
                );
                path.push(sub.get_name().to_string());
                walk(sub, path);
                path.pop();
            }
        }
        let mut cmd = crate::Cli::command();
        cmd.build();
        walk(&cmd, &mut vec!["xxh".to_string()]);
    }

    #[test]
    fn rendering_is_one_line_per_candidate() {
        let cs = [
            Candidate::plain("web"),
            Candidate {
                value: "docker:".into(),
                help: Some("running docker container".into()),
            },
        ];
        assert_eq!(render(&cs), "web\ndocker:\trunning docker container\n");
    }

    fn target(current: &str) -> Vec<String> {
        let hosts = || ["web", "db", "web", "web-ssh"].map(String::from).to_vec();
        let containers = |scheme: &str| match scheme {
            "docker" => vec!["app".to_string(), "api".to_string()],
            "container" => vec!["auto1".to_string()],
            _ => Vec::new(),
        };
        target_candidates(current, &hosts, &containers)
            .into_iter()
            .map(|c| c.value)
            .collect()
    }

    #[test]
    fn targets_are_hosts_schemes_and_containers() {
        // Hosts without repeats, in order, then the schemes (C-K7).
        assert_eq!(
            target(""),
            [
                "db",
                "web",
                "web-ssh",
                "container:",
                "docker:",
                "podman:",
                "ssh:"
            ]
        );
        assert_eq!(target("w"), ["web", "web-ssh"]);
        assert_eq!(target("d"), ["db", "docker:"]);
        // The typed prefix is kept.
        assert_eq!(target("deploy@w"), ["deploy@web", "deploy@web-ssh"]);
        assert_eq!(target("ssh:d"), ["ssh:db"]);
        assert_eq!(target("ssh:me@d"), ["ssh:me@db"]);
        assert!(target("me@").iter().all(|v| !v.contains(':')));
        // Containers of the scheme's runtime (C-K8).
        assert_eq!(target("docker:"), ["docker:app", "docker:api"]);
        assert_eq!(target("docker:ap"), ["docker:app", "docker:api"]);
        assert_eq!(target("docker:app"), ["docker:app"]);
        assert_eq!(target("container:"), ["container:auto1"]);
        assert!(target("podman:").is_empty());
        assert!(target("zzz").is_empty());
    }

    #[test]
    fn plugins_are_offered_by_state() {
        let installed = ["git", "nvim", "prompt"].map(String::from);
        let enabled = ["nvim", "gone"].map(String::from);
        let names = |current: &str, state| -> Vec<String> {
            plugin_candidates(current, &installed, &enabled, state)
                .into_iter()
                .map(|c| c.value)
                .collect()
        };
        assert_eq!(names("", PluginState::Installed), ["git", "nvim", "prompt"]);
        assert_eq!(names("", PluginState::Disabled), ["git", "prompt"]);
        // Whatever the config enables can be disabled, installed or not.
        assert_eq!(names("", PluginState::Enabled), ["gone", "nvim"]);
        assert_eq!(names("p", PluginState::Disabled), ["prompt"]);
    }
}
