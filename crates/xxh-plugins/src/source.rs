//! `trait PackageSource` — the provider abstraction (T032, Принцип IX).
//!
//! The core and the public plugin contract never learn how a package was
//! obtained: git, a local path, or (behind the `nix-source` feature) nixpkgs.
//! See contracts/plugin-source-trait.md (C-S1..C-S7).

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
}

impl SourceSpec {
    /// Parse a CLI source argument (`xxh plugin add <source>`):
    /// `nixpkgs:<attr>` → Nix; URLs / scp-like / `*.git` → Git (optional `#<ref>`);
    /// anything else → Local path.
    pub fn parse(arg: &str) -> Result<Self, PluginError> {
        if let Some(attr) = arg.strip_prefix("nixpkgs:") {
            if attr.is_empty() {
                return Err(PluginError::Manifest("empty nixpkgs attribute".into()));
            }
            return Ok(Self::Nix { attr: attr.into() });
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
        }
    }
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
/// (C-S1); the Nix provider exists only behind the `nix-source` feature, and its
/// absence yields a clear plugin-class message, never a broken tool (C-S2).
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
