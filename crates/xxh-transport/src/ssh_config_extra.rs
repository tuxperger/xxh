//! The ssh_config keys `russh-config` does not read but the built-in client
//! honours: `IdentitiesOnly`, `IdentityAgent`, `ForwardAgent` (012 research R5).
//!
//! OpenSSH semantics where they matter here: `Host` blocks with `*`/`?` patterns
//! and `!` negation, the first value obtained for a key wins. `Match` and
//! `Include` are skipped, as `russh-config` skips them.

use std::path::PathBuf;

/// Extra per-host settings from `~/.ssh/config`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostExtras {
    pub identities_only: Option<bool>,
    /// `IdentityAgent`, when set.
    pub identity_agent: Option<AgentSetting>,
    pub forward_agent: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSetting {
    /// `IdentityAgent none`: do not use an agent.
    Disabled,
    /// A socket path.
    Socket(PathBuf),
    /// `IdentityAgent SSH_AUTH_SOCK`.
    FromEnv,
}

/// Read `~/.ssh/config` for `alias`; a missing file means no settings.
pub fn for_host(alias: &str) -> HostExtras {
    let Some(home) = std::env::var_os("HOME") else {
        return HostExtras::default();
    };
    let path = PathBuf::from(&home).join(".ssh").join("config");
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text, alias, &PathBuf::from(home)),
        Err(_) => HostExtras::default(),
    }
}

/// `*` and `?` glob match, as ssh_config patterns.
fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// A `Host` line matches when some pattern matches and no negated one does.
fn host_matches(patterns: &[&str], alias: &str) -> bool {
    let mut hit = false;
    for p in patterns {
        if let Some(neg) = p.strip_prefix('!') {
            if glob(neg, alias) {
                return false;
            }
        } else if glob(p, alias) {
            hit = true;
        }
    }
    hit
}

fn yes(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "yes" | "true" => Some(true),
        "no" | "false" => Some(false),
        _ => None,
    }
}

/// Parse ssh_config `text` for `alias`; `home` expands a leading `~`.
pub fn parse(text: &str, alias: &str, home: &std::path::Path) -> HostExtras {
    let mut out = HostExtras::default();
    // Lines before the first Host apply to every host.
    let mut active = true;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (
                k,
                v.trim_start_matches(|c: char| c.is_whitespace() || c == '='),
            ),
            None => (line, ""),
        };
        let value = value.trim().trim_matches('"');
        match key.to_ascii_lowercase().as_str() {
            "host" => {
                let patterns: Vec<&str> = value.split_whitespace().collect();
                active = host_matches(&patterns, alias);
            }
            // Conditions are not evaluated: their settings never apply here.
            "match" => active = false,
            _ if !active => {}
            "identitiesonly" => {
                if out.identities_only.is_none() {
                    out.identities_only = yes(value);
                }
            }
            "forwardagent" => {
                if out.forward_agent.is_none() {
                    out.forward_agent = yes(value);
                }
            }
            "identityagent" => {
                if out.identity_agent.is_none() {
                    out.identity_agent = Some(match value {
                        v if v.eq_ignore_ascii_case("none") => AgentSetting::Disabled,
                        "SSH_AUTH_SOCK" => AgentSetting::FromEnv,
                        v => AgentSetting::Socket(match v.strip_prefix("~/") {
                            Some(rest) => home.join(rest),
                            None => PathBuf::from(v),
                        }),
                    });
                }
            }
            _ => {}
        }
    }
    out
}

/// `Include` nesting followed when listing aliases; OpenSSH itself stops at 16.
const INCLUDE_DEPTH: usize = 4;

/// The host aliases of the user's `~/.ssh/config`, for completion (007 C-K7).
pub fn user_host_aliases() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    host_aliases(&home.join(".ssh").join("config"), &home)
}

/// Every concrete `Host` alias in `config` and the files it includes, sorted
/// and without repeats (007 T006, C-K7). Patterns (`*`, `?`, `!`) name no host
/// and are left out; an unreadable file contributes nothing. Unlike [`parse`]
/// this follows `Include`: aliases usually live in `config.d/*`.
pub fn host_aliases(config: &std::path::Path, home: &std::path::Path) -> Vec<String> {
    let mut out = std::collections::BTreeSet::new();
    let base = config.parent().unwrap_or(std::path::Path::new("."));
    collect_aliases(config, base, home, INCLUDE_DEPTH, &mut out);
    out.into_iter().collect()
}

fn collect_aliases(
    file: &std::path::Path,
    base: &std::path::Path,
    home: &std::path::Path,
    depth: usize,
    out: &mut std::collections::BTreeSet<String>,
) {
    let Ok(text) = std::fs::read_to_string(file) else {
        return;
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(|c: char| c.is_whitespace() || c == '=') else {
            continue;
        };
        let words = value
            .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
            .split_whitespace()
            .map(|w| w.trim_matches('"'))
            .filter(|w| !w.is_empty());
        match key.to_ascii_lowercase().as_str() {
            "host" => out.extend(
                words
                    .filter(|w| !w.contains(['*', '?', '!']))
                    .map(str::to_string),
            ),
            "include" if depth > 0 => {
                for word in words {
                    for path in included_files(word, base, home) {
                        collect_aliases(&path, base, home, depth - 1, out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The files one `Include` word names: `~/` is the home directory, a relative
/// path is taken from the config's directory, and the file name may be a glob.
fn included_files(word: &str, base: &std::path::Path, home: &std::path::Path) -> Vec<PathBuf> {
    let path = match word.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if std::path::Path::new(word).is_absolute() => PathBuf::from(word),
        None => base.join(word),
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let Some(Ok(entries)) = path.parent().map(std::fs::read_dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| glob(&name, &e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Aliases come from the config and its includes; patterns never do (007 T006).
    #[test]
    fn aliases_skip_patterns_and_follow_includes() {
        let home = std::env::temp_dir().join(format!(
            "xxh-aliases-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let ssh = home.join(".ssh");
        std::fs::create_dir_all(ssh.join("config.d")).unwrap();
        std::fs::write(
            ssh.join("config"),
            "Include config.d/*.conf ~/extra missing\n\
             Host web db-1 \"quoted\"\n  User deploy\n\
             Host *.internal !secret jump-?\n\
             Host=eq\nHost *\n  ForwardAgent no\n",
        )
        .unwrap();
        std::fs::write(ssh.join("config.d/a.conf"), "Host inc-a\nHost web\n").unwrap();
        std::fs::write(ssh.join("config.d/b.txt"), "Host not-matched\n").unwrap();
        std::fs::write(home.join("extra"), "host lower-key\n").unwrap();

        assert_eq!(
            host_aliases(&ssh.join("config"), &home),
            ["db-1", "eq", "inc-a", "lower-key", "quoted", "web"]
        );
        // No file at all: nothing to offer, no error.
        assert!(host_aliases(&home.join("nope"), &home).is_empty());
        std::fs::remove_dir_all(&home).unwrap();
    }

    const CONFIG: &str = "\
IdentitiesOnly no
Host bastion jump-*
  ForwardAgent yes
  IdentityAgent ~/.agent.sock
Host *.internal !secret.internal
  ForwardAgent=yes
  IdentitiesOnly yes
Host secret.internal
  IdentityAgent none
Match host foo
  ForwardAgent yes
Host *
  ForwardAgent no
  IdentityAgent SSH_AUTH_SOCK
";

    fn p(alias: &str) -> HostExtras {
        parse(CONFIG, alias, std::path::Path::new("/home/u"))
    }

    #[test]
    fn first_value_wins_across_blocks() {
        let b = p("bastion");
        assert_eq!(b.forward_agent, Some(true));
        assert_eq!(
            b.identity_agent,
            Some(AgentSetting::Socket("/home/u/.agent.sock".into()))
        );
        // The global line before any Host comes first.
        assert_eq!(b.identities_only, Some(false));
        assert_eq!(p("jump-2").forward_agent, Some(true));
        assert_eq!(p("other").forward_agent, Some(false));
        assert_eq!(p("other").identity_agent, Some(AgentSetting::FromEnv));
    }

    #[test]
    fn negation_and_match_blocks() {
        assert_eq!(p("db.internal").forward_agent, Some(true));
        let s = p("secret.internal");
        assert_eq!(s.forward_agent, Some(false), "negated out of *.internal");
        assert_eq!(s.identity_agent, Some(AgentSetting::Disabled));
        assert_eq!(
            p("foo").forward_agent,
            Some(false),
            "Match is not evaluated"
        );
    }

    #[test]
    fn globs() {
        assert!(glob("*", "x"));
        assert!(glob("jump-?", "jump-1"));
        assert!(!glob("jump-?", "jump-12"));
        assert!(glob("*.internal", "a.b.internal"));
        assert!(!glob("*.internal", "internal"));
    }
}
