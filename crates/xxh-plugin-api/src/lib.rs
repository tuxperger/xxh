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
/// 1.1.0 (008): optional `builds` and the `post_fetch` hook stage.
pub const API_VERSION: Version = Version::new(1, 1, 0);

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
    /// After a shell build is unpacked on the client, before it becomes
    /// visible (008 C-B5). A failure rejects the build.
    PostFetch,
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
    /// Per-platform builds of a shell package, keyed `os-arch` (008 C-B1).
    #[serde(default)]
    pub builds: BTreeMap<String, BuildSpec>,
}

/// Where to get one platform's build of a shell package and how to check it
/// (008 contracts/shell-package-builds.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildSpec {
    /// `https://` or `file://` address of a tar archive (gzip/zstd allowed).
    pub url: String,
    /// SHA-256 of the archive, lowercase hex.
    pub sha256: String,
    /// Leading path components to drop when unpacking.
    #[serde(default)]
    pub strip: u32,
}

impl BuildSpec {
    /// C-B1: a usable scheme and a well-formed checksum.
    pub fn check(&self, platform: &str) -> Result<(), PluginError> {
        let bad = |what: &str| PluginError::Manifest(format!("builds.{platform}: {what}"));
        if !(self.url.starts_with("https://") || self.url.starts_with("file://")) {
            return Err(bad("url must start with https:// or file://"));
        }
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(bad("sha256 must be 64 lowercase hex digits"));
        }
        Ok(())
    }
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
        targets_allow(&self.targets, os, arch, libc)
    }
}

/// Standalone target matching (C-M5) for callers that carry a target list without a
/// full manifest (e.g. component filtering in the session). Empty list = any platform.
pub fn targets_allow(targets: &[String], os: &str, arch: &str, libc: &str) -> bool {
    if targets.is_empty() {
        return true;
    }
    targets.iter().any(|t| target_matches(t, os, arch, libc))
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
        let m = Manifest::parse("name = \"x\"\nversion = \"0.1.0\"\napi_version = \"2.0.0\"\n")
            .unwrap();
        assert!(matches!(
            m.check_api(),
            Err(PluginError::ApiMismatch { .. })
        ));
    }

    #[test]
    fn empty_targets_means_any_platform() {
        let m = Manifest::parse("name = \"x\"\nversion = \"0.1.0\"\napi_version = \"1.0.0\"\n")
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

    /// 008 T001: builds parse with a default `strip`, are checked, and old
    /// manifests without them still parse.
    #[test]
    fn shell_builds_are_parsed_and_checked() {
        let m = Manifest::parse(
            "name = \"zsh\"\nversion = \"5.8.0\"\napi_version = \"1.1.0\"\n\
             [builds.linux-x86_64]\nurl = \"https://e.org/z.tgz\"\n\
             sha256 = \"6df668fb6e9a12874e0d80518d582f2e99e512d4a4532fa73d938360aaddc838\"\n\
             [builds.linux-aarch64]\nurl = \"file:///tmp/z.tgz\"\nsha256 = \"00\"\nstrip = 1\n\
             [hooks.post_fetch]\nrun = \"hooks/post-fetch.sh\"\n",
        )
        .unwrap();
        m.check_api().unwrap();
        let x86 = &m.builds["linux-x86_64"];
        assert_eq!(x86.strip, 0);
        x86.check("linux-x86_64").unwrap();
        let arm = &m.builds["linux-aarch64"];
        assert_eq!(arm.strip, 1);
        assert!(arm.check("linux-aarch64").is_err(), "short sha256");
        assert!(m.hooks.contains_key(&LifecycleStage::PostFetch));

        let http = BuildSpec {
            url: "http://e.org/z.tgz".into(),
            sha256: x86.sha256.clone(),
            strip: 0,
        };
        assert!(http.check("p").is_err(), "plain http is refused");

        let old = Manifest::parse(SAMPLE).unwrap();
        assert!(old.builds.is_empty());
        old.check_api().unwrap();
    }
}
