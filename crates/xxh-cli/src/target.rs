//! Target-address parsing (002-container-targets T011, contracts/target-addressing.md).
//!
//! One positional «target» describes both SSH hosts and containers, uniformly but
//! distinguishably via a scheme prefix. Parsing is a pure function over the string
//! (no docker, no network) so the whole grammar is covered by a unit table; the
//! runtime-selector precedence (C-A3) and the ambiguous/ssh-only rejections (C-A4,
//! C-A5) are likewise pure and tested here.

use xxh_config::RuntimeSetting;
use xxh_transport::{ContainerRuntime, RuntimeSelector};

/// The transport family recovered from a target string, before config/flag
/// precedence turns it into a `xxh_transport::ResolvedTarget`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedTarget {
    /// SSH host: bare, `user@host`, or `ssh:`-prefixed. `host` keeps any `user@`.
    Ssh { host: String },
    /// Container target; the scheme fixes the runtime or defers to precedence.
    Container {
        scheme: RuntimeScheme,
        reference: String,
    },
}

/// How the container-target scheme constrains the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeScheme {
    /// `docker:` — pinned to docker (C-A4: never substituted).
    Docker,
    /// `podman:` — pinned to podman (C-A4).
    Podman,
    /// `container:` — runtime from precedence/auto-order (C-A3).
    Auto,
}

/// Target-addressing errors (C-A2/C-A4/C-A5). All map to the config exit class:
/// they are caught before any connection is attempted.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TargetError {
    #[error(
        "unknown target scheme `{scheme}:` — supported: `docker:`, `podman:`, \
         `container:`, `ssh:` (or a bare `[user@]host` for SSH)"
    )]
    UnknownScheme { scheme: String },
    #[error("empty container reference after `{scheme}:`")]
    EmptyReference { scheme: String },
    #[error("empty target")]
    Empty,
    #[error(
        "ambiguous runtime: the `{scheme}:` scheme pins the runtime but \
         `--runtime {flag}` requests a different one"
    )]
    AmbiguousRuntime {
        scheme: &'static str,
        flag: &'static str,
    },
    #[error("`{flag}` is an SSH-only option and does not apply to container targets")]
    SshOnlyFlag { flag: &'static str },
    #[error("`--runtime` selects a container runtime and does not apply to SSH targets")]
    RuntimeFlagOnSsh,
}

/// Parse a positional target string into its family and reference (pure grammar).
///
/// A scheme is a leading lowercase-identifier token before the first `:`
/// (`docker`, `podman`, `container`, `ssh`); everything else — including bare
/// hosts, `user@host`, and IPv6 literals — is an SSH target (C-A1 backward compat).
pub fn parse(raw: &str) -> Result<ParsedTarget, TargetError> {
    if raw.is_empty() {
        return Err(TargetError::Empty);
    }
    if let Some((prefix, rest)) = raw.split_once(':') {
        if is_scheme_token(prefix) {
            return match prefix {
                "ssh" => Ok(ParsedTarget::Ssh {
                    host: rest.to_string(),
                }),
                "docker" => container(RuntimeScheme::Docker, "docker", rest),
                "podman" => container(RuntimeScheme::Podman, "podman", rest),
                "container" => container(RuntimeScheme::Auto, "container", rest),
                other => Err(TargetError::UnknownScheme {
                    scheme: other.to_string(),
                }),
            };
        }
    }
    // No recognizable scheme → SSH target (bare host / user@host).
    Ok(ParsedTarget::Ssh {
        host: raw.to_string(),
    })
}

fn container(
    scheme: RuntimeScheme,
    name: &str,
    reference: &str,
) -> Result<ParsedTarget, TargetError> {
    if reference.is_empty() {
        return Err(TargetError::EmptyReference {
            scheme: name.to_string(),
        });
    }
    Ok(ParsedTarget::Container {
        scheme,
        reference: reference.to_string(),
    })
}

/// A scheme token is a non-empty lowercase-ascii identifier: it starts with a
/// letter so digit-leading IPv6 literals (`2001:db8::1`) stay SSH targets, and
/// carries no `@`/`.`/`:` so `user@host` and FQDNs never look like schemes.
fn is_scheme_token(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Resolve the concrete runtime selection for a container target (C-A3/C-A4).
///
/// `effective` is the already-merged config/flag runtime (flag over per-target
/// over global over default), consulted only for the `container:` (`Auto`)
/// scheme. An explicit `docker:`/`podman:` scheme pins the runtime and is never
/// silently overridden — a conflicting `--runtime` is a hard error (C-A4).
pub fn resolve_runtime_selector(
    scheme: RuntimeScheme,
    effective: RuntimeSetting,
    runtime_flag: Option<RuntimeSetting>,
) -> Result<RuntimeSelector, TargetError> {
    let pinned_flag = match runtime_flag {
        Some(RuntimeSetting::Docker) => Some(ContainerRuntime::Docker),
        Some(RuntimeSetting::Podman) => Some(ContainerRuntime::Podman),
        _ => None, // `--runtime auto` or unset does not pin
    };
    match scheme {
        RuntimeScheme::Docker => match pinned_flag {
            Some(ContainerRuntime::Podman) => Err(TargetError::AmbiguousRuntime {
                scheme: "docker",
                flag: "podman",
            }),
            _ => Ok(RuntimeSelector::Explicit(ContainerRuntime::Docker)),
        },
        RuntimeScheme::Podman => match pinned_flag {
            Some(ContainerRuntime::Docker) => Err(TargetError::AmbiguousRuntime {
                scheme: "podman",
                flag: "docker",
            }),
            _ => Ok(RuntimeSelector::Explicit(ContainerRuntime::Podman)),
        },
        RuntimeScheme::Auto => Ok(match effective {
            RuntimeSetting::Auto => RuntimeSelector::Auto,
            RuntimeSetting::Docker => RuntimeSelector::Explicit(ContainerRuntime::Docker),
            RuntimeSetting::Podman => RuntimeSelector::Explicit(ContainerRuntime::Podman),
        }),
    }
}

/// Which session flags were set explicitly on the command line, for the SSH-only
/// vs container-only split (C-A5).
#[derive(Debug, Clone, Copy, Default)]
pub struct CliTargetFlags {
    pub identity_set: bool,
    pub transport_set: bool,
    pub runtime_set: bool,
}

/// Reject flags that do not apply to the target's family before connecting (C-A5):
/// `-i/--identity` and `--transport` are SSH-only; `--runtime` is container-only.
pub fn validate_flags(target: &ParsedTarget, flags: &CliTargetFlags) -> Result<(), TargetError> {
    match target {
        ParsedTarget::Ssh { .. } => {
            if flags.runtime_set {
                return Err(TargetError::RuntimeFlagOnSsh);
            }
        }
        ParsedTarget::Container { .. } => {
            if flags.identity_set {
                return Err(TargetError::SshOnlyFlag { flag: "--identity" });
            }
            if flags.transport_set {
                return Err(TargetError::SshOnlyFlag {
                    flag: "--transport",
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_address_form() {
        // SSH forms (C-A1): bare host, user@host, explicit ssh:, IPv6 literal.
        assert_eq!(
            parse("myhost").unwrap(),
            ParsedTarget::Ssh {
                host: "myhost".into()
            }
        );
        assert_eq!(
            parse("deploy@10.0.0.5").unwrap(),
            ParsedTarget::Ssh {
                host: "deploy@10.0.0.5".into()
            }
        );
        assert_eq!(
            parse("ssh:deploy@web").unwrap(),
            ParsedTarget::Ssh {
                host: "deploy@web".into()
            }
        );
        assert_eq!(
            parse("2001:db8::1").unwrap(),
            ParsedTarget::Ssh {
                host: "2001:db8::1".into()
            }
        );

        // Container forms.
        assert_eq!(
            parse("docker:app1").unwrap(),
            ParsedTarget::Container {
                scheme: RuntimeScheme::Docker,
                reference: "app1".into()
            }
        );
        assert_eq!(
            parse("podman:6f0a12").unwrap(),
            ParsedTarget::Container {
                scheme: RuntimeScheme::Podman,
                reference: "6f0a12".into()
            }
        );
        assert_eq!(
            parse("container:app1").unwrap(),
            ParsedTarget::Container {
                scheme: RuntimeScheme::Auto,
                reference: "app1".into()
            }
        );
    }

    #[test]
    fn rejects_bad_schemes_and_empty_refs() {
        assert_eq!(
            parse("k8s:pod"),
            Err(TargetError::UnknownScheme {
                scheme: "k8s".into()
            })
        );
        assert_eq!(
            parse("docker:"),
            Err(TargetError::EmptyReference {
                scheme: "docker".into()
            })
        );
        assert_eq!(parse(""), Err(TargetError::Empty));
    }

    #[test]
    fn runtime_selector_precedence_and_ambiguity() {
        use ContainerRuntime::*;
        // container: defers to the effective (merged) setting.
        assert_eq!(
            resolve_runtime_selector(RuntimeScheme::Auto, RuntimeSetting::Auto, None).unwrap(),
            RuntimeSelector::Auto
        );
        assert_eq!(
            resolve_runtime_selector(RuntimeScheme::Auto, RuntimeSetting::Podman, None).unwrap(),
            RuntimeSelector::Explicit(Podman)
        );
        // docker:/podman: pin regardless of config, and tolerate a matching flag.
        assert_eq!(
            resolve_runtime_selector(RuntimeScheme::Docker, RuntimeSetting::Podman, None).unwrap(),
            RuntimeSelector::Explicit(Docker)
        );
        assert_eq!(
            resolve_runtime_selector(
                RuntimeScheme::Docker,
                RuntimeSetting::Auto,
                Some(RuntimeSetting::Docker)
            )
            .unwrap(),
            RuntimeSelector::Explicit(Docker)
        );
        // A conflicting --runtime is a hard error (C-A4: no silent substitution).
        assert_eq!(
            resolve_runtime_selector(
                RuntimeScheme::Docker,
                RuntimeSetting::Auto,
                Some(RuntimeSetting::Podman)
            ),
            Err(TargetError::AmbiguousRuntime {
                scheme: "docker",
                flag: "podman"
            })
        );
    }

    #[test]
    fn flag_family_split_is_enforced() {
        let ssh = ParsedTarget::Ssh { host: "h".into() };
        let ctr = ParsedTarget::Container {
            scheme: RuntimeScheme::Docker,
            reference: "app1".into(),
        };

        // --runtime is container-only.
        assert_eq!(
            validate_flags(
                &ssh,
                &CliTargetFlags {
                    runtime_set: true,
                    ..Default::default()
                }
            ),
            Err(TargetError::RuntimeFlagOnSsh)
        );
        // -i and --transport are SSH-only.
        assert_eq!(
            validate_flags(
                &ctr,
                &CliTargetFlags {
                    identity_set: true,
                    ..Default::default()
                }
            ),
            Err(TargetError::SshOnlyFlag { flag: "--identity" })
        );
        assert_eq!(
            validate_flags(
                &ctr,
                &CliTargetFlags {
                    transport_set: true,
                    ..Default::default()
                }
            ),
            Err(TargetError::SshOnlyFlag {
                flag: "--transport"
            })
        );
        // Matching flags are accepted.
        assert!(validate_flags(&ssh, &CliTargetFlags::default()).is_ok());
        assert!(
            validate_flags(
                &ctr,
                &CliTargetFlags {
                    runtime_set: true,
                    ..Default::default()
                }
            )
            .is_ok()
        );
    }
}
