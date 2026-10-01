//! `trait PackageSource` — the provider abstraction (T032, Принцип IX).
//!
//! The core and the public plugin contract never learn how a package was
//! obtained: git, a local path, or (behind the `nix-source` feature) nixpkgs or an
//! arbitrary flake. See contracts/plugin-source-trait.md (C-S1..C-S7) and
//! specs/003-flake-plugin-source/contracts/flake-source.md (C-F*).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use xxh_plugin_api::{Manifest, PluginError};

/// Is the provider usable in the current client environment (C-S2)?
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Available,
    Unavailable { reason: String },
}

/// Can the provider produce artefacts for the given host platform (C-S3)?
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    Supported,
    Unsupported { reason: String },
}

/// Where a plugin comes from. This is pure data (stored in the registry index so
/// `update` can re-fetch); the matching provider is looked up via [`provider_for`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SourceSpec {
    Git {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
    },
    Local {
        path: PathBuf,
    },
    /// ⭐ nixpkgs attribute (`nixpkgs:<attr>`); usable only when the client has Nix
    /// and the crate is built with `nix-source` (§FR-033, C-S2).
    Nix {
        attr: String,
    },
    /// ⭐ Output of an arbitrary flake (`flake:<ref>#<attr>`), same availability
    /// rules as [`SourceSpec::Nix`] (003 §FR-001, C-F1). `locked_url`/`revision` pin
    /// the exact flake revision chosen at install time; they change only on an
    /// explicit `plugin update` (003 §FR-014..016).
    Flake {
        reference: String,
        attr: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locked_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revision: Option<String>,
        /// User-chosen plugin name (`plugin add --name`) for wrapped programs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

impl SourceSpec {
    /// Parse a CLI source argument (`xxh plugin add <source>`):
    /// `nixpkgs:<attr>` → Nix; `flake:<ref>[#<attr>]` → Flake; URLs / scp-like /
    /// `*.git` → Git (optional `#<ref>`); anything else → Local path.
    pub fn parse(arg: &str) -> Result<Self, PluginError> {
        if let Some(attr) = arg.strip_prefix("nixpkgs:") {
            if attr.is_empty() {
                return Err(PluginError::Manifest("empty nixpkgs attribute".into()));
            }
            return Ok(Self::Nix { attr: attr.into() });
        }
        // Flakes are selected only by the explicit prefix, so every other form
        // keeps parsing exactly as before (003 §FR-003, C-F2).
        if let Some(rest) = arg.strip_prefix("flake:") {
            let (reference, attr) = rest.split_once('#').unwrap_or((rest, "default"));
            if reference.is_empty() {
                return Err(PluginError::Manifest("empty flake reference".into()));
            }
            if attr.is_empty() {
                return Err(PluginError::Manifest("empty flake output after `#`".into()));
            }
            return Ok(Self::Flake {
                reference: absolutize_flake_ref(reference),
                attr: attr.into(),
                locked_url: None,
                revision: None,
                name: None,
            });
        }
        let looks_git = arg.starts_with("http://")
            || arg.starts_with("https://")
            || arg.starts_with("git://")
            || arg.starts_with("ssh://")
            || arg.starts_with("git@")
            || arg.starts_with("file://")
            || arg.ends_with(".git");
        if looks_git {
            let (url, reference) = match arg.rsplit_once('#') {
                Some((u, r)) if !r.is_empty() => (u.to_string(), Some(r.to_string())),
                _ => (arg.to_string(), None),
            };
            return Ok(Self::Git { url, reference });
        }
        Ok(Self::Local { path: arg.into() })
    }

    /// Human-readable form for messages and the registry index.
    pub fn describe(&self) -> String {
        match self {
            Self::Git {
                url,
                reference: Some(r),
            } => format!("git {url}#{r}"),
            Self::Git {
                url,
                reference: None,
            } => format!("git {url}"),
            Self::Local { path } => format!("local {}", path.display()),
            Self::Nix { attr } => format!("nixpkgs:{attr}"),
            Self::Flake {
                reference, attr, ..
            } => format!("flake:{}#{attr}", redact_ref(reference)),
        }
    }

    /// [`describe`](Self::describe) plus the pinned revision for sources that have
    /// one (C-FC7): `flake:… @ 1a2b3c4d5e6f` or `flake:… @ unpinned`.
    pub fn label(&self) -> String {
        match self {
            Self::Flake { .. } => format!("{} @ {}", self.describe(), self.short_revision()),
            _ => self.describe(),
        }
    }

    /// The pinned revision shortened to 12 characters, or `unpinned`.
    pub fn short_revision(&self) -> String {
        match self {
            Self::Flake {
                revision: Some(r), ..
            } => r.chars().take(12).collect(),
            _ => "unpinned".into(),
        }
    }

    pub fn is_flake(&self) -> bool {
        matches!(self, Self::Flake { .. })
    }

    /// The same source with its pin dropped, so a fetch re-resolves the original
    /// reference (`plugin update`, C-F19). A no-op for sources without a pin.
    pub fn unpinned(&self) -> Self {
        match self {
            Self::Flake {
                reference,
                attr,
                name,
                ..
            } => Self::Flake {
                reference: reference.clone(),
                attr: attr.clone(),
                locked_url: None,
                revision: None,
                name: name.clone(),
            },
            other => other.clone(),
        }
    }

    /// Whether two specs name the same origin, ignoring pin and chosen name: a
    /// re-install from the same origin updates in place, anything else is a name
    /// conflict (C-F20).
    pub fn same_origin(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Flake {
                    reference: r1,
                    attr: a1,
                    ..
                },
                Self::Flake {
                    reference: r2,
                    attr: a2,
                    ..
                },
            ) => r1 == r2 && a1 == a2,
            _ => self == other,
        }
    }
}

/// Make a path-like flake reference absolute so `plugin update` works from any
/// directory (C-F3). URL-like references are returned untouched.
fn absolutize_flake_ref(reference: &str) -> String {
    let path = if let Some(rest) = reference.strip_prefix("~/") {
        match directories::BaseDirs::new() {
            Some(bd) => bd.home_dir().join(rest),
            None => return reference.to_string(),
        }
    } else if reference == "."
        || reference == ".."
        || reference.starts_with("./")
        || reference.starts_with("../")
    {
        PathBuf::from(reference)
    } else {
        return reference.to_string();
    };
    path.canonicalize()
        .or_else(|_| std::path::absolute(&path))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| reference.to_string())
}

/// Strip credentials from a source reference (or any text quoting one) before it
/// reaches output, errors or logs (003 §FR-024, C-F17): URL userinfo and the
/// values of `access_token=` / `token=` parameters become `<redacted>`.
pub fn redact_ref(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        let end = tail
            .find(|c: char| matches!(c, '/' | '?' | '#') || c.is_whitespace())
            .unwrap_or(tail.len());
        let authority = &tail[..end];
        match authority.rfind('@') {
            // A bare `git@` login is the conventional non-secret ssh user.
            Some(at) if &authority[..at] != "git" => {
                out.push_str("<redacted>");
                out.push_str(&authority[at..]);
            }
            _ => out.push_str(authority),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    redact_param(&redact_param(&out, "access_token="), "token=")
}

fn redact_param(text: &str, key: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(key) {
        let (head, tail) = rest.split_at(i + key.len());
        out.push_str(head);
        let end = tail
            .find(|c: char| matches!(c, '&' | '#' | '\'' | '"') || c.is_whitespace())
            .unwrap_or(tail.len());
        out.push_str("<redacted>");
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Result of a fetch: a local directory holding the complete package (with its
/// `plugin.toml`), plus env vars the package wants exported in the remote shell
/// init (C-S5; used by the Nix provider for TERMINFO/SSL_CERT_FILE/…).
#[derive(Debug)]
pub struct FetchedPackage {
    pub manifest: Manifest,
    pub dir: PathBuf,
    pub env: BTreeMap<String, String>,
    /// Temp dir to drop after the registry has copied the package (git clones).
    pub cleanup: Option<PathBuf>,
    /// The spec the registry must record instead of the requested one — how a
    /// provider reports a pin it resolved during the fetch (C-F18). `None` keeps
    /// the requested spec.
    pub resolved: Option<SourceSpec>,
}

/// A way of obtaining plugin packages (Принцип IX). All errors are class
/// "plugin" (C-S6).
#[async_trait::async_trait]
pub trait PackageSource: Send + Sync {
    /// Stable provider id ("git" | "local" | "nix").
    fn id(&self) -> &'static str;

    /// Whether the provider can run on this client (C-S2).
    fn availability(&self) -> Availability;

    /// Whether the provider can produce artefacts for the host platform (C-S3).
    fn supports_target(&self, os: &str, arch: &str, libc: &str) -> Support;

    /// Obtain the package described by `spec` (C-S4).
    async fn fetch(&self, spec: &SourceSpec) -> Result<FetchedPackage, PluginError>;
}

/// Look up the provider for a spec. Mandatory providers (git, local) always exist
/// (C-S1); the Nix and flake providers exist only behind the `nix-source` feature, and
/// their absence yields a clear plugin-class message, never a broken tool (C-S2).
pub fn provider_for(spec: &SourceSpec) -> Result<Box<dyn PackageSource>, PluginError> {
    match spec {
        SourceSpec::Git { .. } => Ok(Box::new(crate::sources::git::GitProvider)),
        SourceSpec::Local { .. } => Ok(Box::new(crate::sources::local::LocalProvider)),
        #[cfg(feature = "nix-source")]
        SourceSpec::Nix { .. } => Ok(Box::new(crate::sources::nix::NixProvider::new())),
        #[cfg(not(feature = "nix-source"))]
        SourceSpec::Nix { .. } => Err(PluginError::SourceUnavailable(
            "this xxh build has no Nix source support (rebuild with --features nix-source)".into(),
        )),
        #[cfg(feature = "nix-source")]
        SourceSpec::Flake { .. } => Ok(Box::new(crate::sources::flake::FlakeProvider::new())),
        #[cfg(not(feature = "nix-source"))]
        SourceSpec::Flake { .. } => Err(PluginError::SourceUnavailable(
            "this xxh build has no flake source support (rebuild with --features nix-source)"
                .into(),
        )),
    }
}

/// Read and validate a `plugin.toml` in `dir` (api-version check, C-M1).
pub fn read_manifest(dir: &std::path::Path) -> Result<Manifest, PluginError> {
    let path = dir.join("plugin.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| PluginError::Manifest(format!("{}: {e}", path.display())))?;
    let manifest = Manifest::parse(&text)?;
    manifest.check_api()?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_source_kinds() {
        assert!(matches!(
            SourceSpec::parse("https://example.com/p.git").unwrap(),
            SourceSpec::Git { .. }
        ));
        assert!(matches!(
            SourceSpec::parse("git@github.com:me/p.git#v1").unwrap(),
            SourceSpec::Git {
                reference: Some(_),
                ..
            }
        ));
        assert!(matches!(
            SourceSpec::parse("nixpkgs:ripgrep").unwrap(),
            SourceSpec::Nix { .. }
        ));
        assert!(matches!(
            SourceSpec::parse("./my-plugin").unwrap(),
            SourceSpec::Local { .. }
        ));
    }

    fn flake(reference: &str, attr: &str) -> SourceSpec {
        SourceSpec::Flake {
            reference: reference.into(),
            attr: attr.into(),
            locked_url: None,
            revision: None,
            name: None,
        }
    }

    #[test]
    fn parses_flake_refs() {
        assert_eq!(
            SourceSpec::parse("flake:github:o/r#tool").unwrap(),
            flake("github:o/r", "tool")
        );
        // No `#<attr>` selects the flake's default output (003 §FR-002).
        assert_eq!(
            SourceSpec::parse("flake:github:o/r").unwrap(),
            flake("github:o/r", "default")
        );
        // Dotted attribute paths and URL queries survive untouched.
        assert_eq!(
            SourceSpec::parse("flake:git+https://h/r?ref=main#pkgsStatic.ripgrep").unwrap(),
            flake("git+https://h/r?ref=main", "pkgsStatic.ripgrep")
        );
        assert_eq!(
            SourceSpec::parse("flake:/abs/dir#x").unwrap(),
            flake("/abs/dir", "x")
        );
    }

    #[test]
    fn rejects_empty_flake_parts() {
        for bad in ["flake:", "flake:#x", "flake:github:o/r#"] {
            assert!(
                matches!(SourceSpec::parse(bad), Err(PluginError::Manifest(_))),
                "{bad} must be rejected"
            );
        }
    }

    #[test]
    fn relative_flake_paths_become_absolute() {
        let SourceSpec::Flake { reference, .. } = SourceSpec::parse("flake:.#x").unwrap() else {
            panic!("expected a flake spec");
        };
        assert!(
            std::path::Path::new(&reference).is_absolute(),
            "got {reference}"
        );
    }

    /// The `flake:` prefix must not change how any pre-existing form parses
    /// (003 §FR-003, C-F2).
    #[test]
    fn flake_prefix_does_not_capture_existing_forms() {
        assert_eq!(
            SourceSpec::parse("https://example.com/p.git#v1").unwrap(),
            SourceSpec::Git {
                url: "https://example.com/p.git".into(),
                reference: Some("v1".into()),
            }
        );
        assert_eq!(
            SourceSpec::parse("git@github.com:me/p.git").unwrap(),
            SourceSpec::Git {
                url: "git@github.com:me/p.git".into(),
                reference: None,
            }
        );
        assert_eq!(
            SourceSpec::parse("nixpkgs:flake").unwrap(),
            SourceSpec::Nix {
                attr: "flake".into()
            }
        );
        assert_eq!(
            SourceSpec::parse("./flake:dir").unwrap(),
            SourceSpec::Local {
                path: "./flake:dir".into()
            }
        );
    }

    #[test]
    fn flake_spec_roundtrips_through_toml() {
        #[derive(Serialize, Deserialize)]
        struct Wrap {
            source: SourceSpec,
        }
        let pinned = SourceSpec::Flake {
            reference: "github:o/r".into(),
            attr: "tool".into(),
            locked_url: Some("github:o/r/abc?narHash=sha256-x".into()),
            revision: Some("abc".into()),
            name: Some("mytool".into()),
        };
        for spec in [pinned, flake("/abs", "default")] {
            let text = toml::to_string(&Wrap {
                source: spec.clone(),
            })
            .unwrap();
            assert!(text.contains("type = \"flake\""), "got: {text}");
            assert_eq!(toml::from_str::<Wrap>(&text).unwrap().source, spec);
        }
        // An unpinned spec carries no pin keys at all.
        let text = toml::to_string(&Wrap {
            source: flake("/abs", "default"),
        })
        .unwrap();
        assert!(!text.contains("revision") && !text.contains("locked_url"));
        // Indexes written before the flake source still load.
        let old: Wrap = toml::from_str("[source]\ntype = \"nix\"\nattr = \"ripgrep\"\n").unwrap();
        assert!(matches!(old.source, SourceSpec::Nix { .. }));
    }

    #[test]
    fn pin_is_ignored_by_origin_and_dropped_by_unpinned() {
        let a = SourceSpec::Flake {
            reference: "github:o/r".into(),
            attr: "tool".into(),
            locked_url: Some("github:o/r/abc".into()),
            revision: Some("abcdef0123456789".into()),
            name: None,
        };
        assert!(a.same_origin(&flake("github:o/r", "tool")));
        assert!(!a.same_origin(&flake("github:o/r", "other")));
        assert!(!a.same_origin(&SourceSpec::Local { path: "x".into() }));
        assert_eq!(a.unpinned(), flake("github:o/r", "tool"));
        assert_eq!(a.label(), "flake:github:o/r#tool @ abcdef012345");
        assert_eq!(a.unpinned().label(), "flake:github:o/r#tool @ unpinned");
    }

    #[test]
    fn credentials_never_survive_redaction() {
        assert_eq!(
            redact_ref("git+https://me:hunter2@host/r.git"),
            "git+https://<redacted>@host/r.git"
        );
        assert_eq!(
            redact_ref("https://ghp_token@github.com/o/r"),
            "https://<redacted>@github.com/o/r"
        );
        assert_eq!(
            redact_ref("git+https://h/r?access_token=s3cret&ref=main"),
            "git+https://h/r?access_token=<redacted>&ref=main"
        );
        // Nothing to hide: untouched, including the conventional ssh `git@` login.
        for plain in ["github:o/r", "git+ssh://git@github.com/o/r", "/home/me/src"] {
            assert_eq!(redact_ref(plain), plain);
        }
        // Applied to free text quoting a URL (build output), not only bare refs.
        let text = redact_ref("error: cannot fetch 'https://u:p@h/x' today");
        assert!(!text.contains("u:p"), "got: {text}");
        let shown = flake("https://u:p@h/r", "x").describe();
        assert!(!shown.contains("u:p"), "got: {shown}");
    }

    #[cfg(not(feature = "nix-source"))]
    #[test]
    fn flake_spec_without_feature_is_a_clear_plugin_error() {
        let spec = SourceSpec::parse("flake:github:o/r#x").unwrap();
        assert!(matches!(
            provider_for(&spec),
            Err(PluginError::SourceUnavailable(_))
        ));
    }

    #[cfg(not(feature = "nix-source"))]
    #[test]
    fn nix_spec_without_feature_is_a_clear_plugin_error() {
        let spec = SourceSpec::parse("nixpkgs:ripgrep").unwrap();
        assert!(matches!(
            provider_for(&spec),
            Err(PluginError::SourceUnavailable(_))
        ));
    }
}
