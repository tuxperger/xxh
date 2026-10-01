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

#[cfg(test)]
mod tests {
    use super::*;

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
