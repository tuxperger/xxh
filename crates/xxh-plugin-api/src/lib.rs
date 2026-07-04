//! xxh-plugin-api — public, semver-versioned plugin contract (Принцип IV).
//!
//! Defines the `plugin.toml` manifest, lifecycle stages and api-version
//! compatibility. Breaking changes bump [`API_VERSION`]'s major.
//! See contracts/plugin-manifest.md.

use std::collections::BTreeMap;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

/// The plugin-contract version this client implements. A plugin is accepted when its
/// `api_version` has the same major and a minor `<=` this one (C-M1).
pub const API_VERSION: Version = Version::new(1, 0, 0);

/// Error class for plugin problems. Maps to CLI exit code 30 (§FR-026).
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("invalid manifest: {0}")]
    Manifest(String),
    #[error("plugin `{name}` needs api {needed} but this client provides {have}")]
    ApiMismatch {
        name: String,
        needed: Version,
        have: Version,
    },
    #[error("version conflict: {0}")]
    VersionConflict(String),
    #[error("dependency cycle involving `{0}`")]
    DependencyCycle(String),
    #[error("missing dependency `{dep}` required by `{by}`")]
    MissingDependency { by: String, dep: String },
    #[error("plugin source unavailable: {0}")]
    SourceUnavailable(String),
    #[error("plugin error: {0}")]
    Other(String),
}

/// Lifecycle stage a hook can attach to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleStage {
    PreConnect,
    PostDeploy,
    PreExit,
}

/// A declared lifecycle hook (run as an isolated subprocess; C-M3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookSpec {
    /// Path to the hook program, relative to the plugin package.
    pub run: String,
    #[serde(default = "default_timeout")]
    pub timeout_s: u32,
}

fn default_timeout() -> u32 {
    30
}

/// The parsed `plugin.toml` manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub version: Version,
    pub api_version: Version,
    #[serde(default)]
    pub dependencies: BTreeMap<String, VersionReq>,
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub hooks: BTreeMap<LifecycleStage, HookSpec>,
    #[serde(default)]
    pub provides: BTreeMap<String, String>,
    #[serde(default)]
    pub priority: i32,
}

impl Manifest {
    /// Parse a `plugin.toml`. Unknown future fields are ignored (forward-compat, C-M2).
    pub fn parse(text: &str) -> Result<Self, PluginError> {
        toml::from_str(text).map_err(|e| PluginError::Manifest(e.to_string()))
    }

    /// Check api-version compatibility against this client (C-M1): same major and
    /// not newer than what this client provides.
    pub fn check_api(&self) -> Result<(), PluginError> {
        let compatible =
            self.api_version.major == API_VERSION.major && self.api_version <= API_VERSION;
        if compatible {
            Ok(())
        } else {
            Err(PluginError::ApiMismatch {
                name: self.name.clone(),
                needed: self.api_version.clone(),
                have: API_VERSION,
            })
        }
    }

    /// True if this plugin is the shell named `name` (via `provides.shell`).
    pub fn provides_shell(&self, name: &str) -> bool {
        self.provides.get("shell").map(|s| s.as_str()) == Some(name)
    }

    /// Whether the plugin is compatible with a host `os/arch/libc` triple. An empty
    /// `targets` list means "any platform" (C-M5). Patterns are `os[/arch[/libc]]`
    /// where `*` matches any segment.
    pub fn supports(&self, os: &str, arch: &str, libc: &str) -> bool {
        if self.targets.is_empty() {
            return true;
        }
        self.targets.iter().any(|t| target_matches(t, os, arch, libc))
    }
}

fn target_matches(pattern: &str, os: &str, arch: &str, libc: &str) -> bool {
    let mut segs = pattern.split('/');
    let p_os = segs.next().unwrap_or("*");
    let p_arch = segs.next().unwrap_or("*");
    let p_libc = segs.next().unwrap_or("*");
    seg_ok(p_os, os) && seg_ok(p_arch, arch) && seg_ok(p_libc, libc)
}

fn seg_ok(pattern: &str, value: &str) -> bool {
    pattern == "*" || pattern.eq_ignore_ascii_case(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        name = "syntax-highlight"
        version = "1.4.0"
        api_version = "1.0.0"
        targets = ["linux", "linux/aarch64", "darwin"]
        priority = 5

        [dependencies]
        base-theme = "^2.0"

        [provides]
        shell = "zsh"

        [hooks.post_deploy]
        run = "hooks/install.sh"
        timeout_s = 20
    "#;

    #[test]
    fn parses_full_manifest() {
        let m = Manifest::parse(SAMPLE).unwrap();
        assert_eq!(m.name, "syntax-highlight");
        assert_eq!(m.version, Version::new(1, 4, 0));
        assert_eq!(m.priority, 5);
        assert!(m.dependencies.contains_key("base-theme"));
        assert!(m.provides_shell("zsh"));
        assert_eq!(m.hooks[&LifecycleStage::PostDeploy].timeout_s, 20);
    }

    #[test]
    fn hook_timeout_defaults_to_30() {
        let m = Manifest::parse(
            "name = \"x\"\n\
             version = \"0.1.0\"\n\
             api_version = \"1.0.0\"\n\
             [hooks.pre_exit]\n\
             run = \"h.sh\"\n",
        )
        .unwrap();
        assert_eq!(m.hooks[&LifecycleStage::PreExit].timeout_s, 30);
    }

    #[test]
    fn api_major_mismatch_is_rejected() {
        let m = Manifest::parse(
            "name = \"x\"\nversion = \"0.1.0\"\napi_version = \"2.0.0\"\n",
        )
        .unwrap();
        assert!(matches!(m.check_api(), Err(PluginError::ApiMismatch { .. })));
    }

    #[test]
    fn empty_targets_means_any_platform() {
        let m = Manifest::parse(
            "name = \"x\"\nversion = \"0.1.0\"\napi_version = \"1.0.0\"\n",
        )
        .unwrap();
        assert!(m.supports("linux", "x86_64", "musl"));
        assert!(m.supports("darwin", "aarch64", "unknown"));
    }

    #[test]
    fn target_patterns_match_by_segment() {
        let m = Manifest::parse(
            "name = \"x\"\n\
             version = \"0.1.0\"\n\
             api_version = \"1.0.0\"\n\
             targets = [\"linux/aarch64\", \"linux/*/musl\"]\n",
        )
        .unwrap();
        assert!(m.supports("linux", "aarch64", "glibc")); // linux/aarch64
        assert!(m.supports("linux", "x86_64", "musl")); // linux/*/musl
        assert!(!m.supports("darwin", "x86_64", "unknown"));
        assert!(!m.supports("linux", "x86_64", "glibc"));
    }
}
