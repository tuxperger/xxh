//! ⭐ FlakeProvider — plugins built from an arbitrary flake output (003, feature
//! `nix-source`).
//!
//! `flake:<ref>#<attr>` is built **on the client**; what travels to a target is an
//! ordinary plugin package, so the host needs neither Nix nor root (003 §FR-013,
//! contracts/flake-source.md C-F*). Two output shapes are accepted (C-F9):
//!
//! * a ready-made plugin — `plugin.toml` at the output root, used as is;
//! * a program — a `bin/` directory, wrapped into a generated plugin.
//!
//! The flake revision is pinned at install time and only moves on an explicit
//! `plugin update` (§FR-014..016). Like the nixpkgs provider, a client without Nix
//! merely loses this source (C-F5, Принцип IX).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use semver::Version;
use serde::Deserialize;
use xxh_plugin_api::PluginError;

use crate::source::{
    Availability, FetchedPackage, PackageSource, SourceSpec, Support, read_manifest, redact_ref,
};
use crate::sources::nix::{
    add_runtime_data, audit_package, client_cache_dir, copy_tree, nix_available_as,
};

/// How many trailing lines of Nix's stderr an error carries (C-F16, §FR-025).
const STDERR_TAIL_LINES: usize = 20;

pub struct FlakeProvider {
    nix_bin: String,
}

impl FlakeProvider {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            nix_bin: "nix".into(),
        }
    }

    /// Use another `nix` executable — lets tests reach the "no Nix on this client"
    /// path without mutating the process environment.
    pub fn with_nix_bin(nix_bin: impl Into<String>) -> Self {
        Self {
            nix_bin: nix_bin.into(),
        }
    }

    /// Run `nix <cmd…> <rest…>`; a failure becomes a plugin-class error tagged
    /// `kind`, carrying the redacted tail of Nix's stderr.
    fn nix(&self, kind: &str, cmd: &[&str], rest: &[&str]) -> Result<String, PluginError> {
        let out = std::process::Command::new(&self.nix_bin)
            .args(flake_nix_args(cmd, rest))
            .output()
            .map_err(|e| PluginError::Other(format!("spawning nix: {e}")))?;
        if !out.status.success() {
            return Err(PluginError::Other(format!(
                "{kind}: nix {} failed:\n{}",
                cmd.join(" "),
                stderr_tail(&String::from_utf8_lossy(&out.stderr))
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Resolve `reference` to its pin (C-F7): `(locked_url, revision)`, both `None`
    /// for a source with no fixed revision (a plain directory, a dirty tree).
    fn resolve_pin(
        &self,
        reference: &str,
    ) -> Result<(Option<String>, Option<String>), PluginError> {
        let json = self.nix(
            "FlakeUnavailable",
            &["flake", "metadata"],
            &["--json", reference],
        )?;
        parse_metadata(&json)
    }

    /// The package's `version` attribute as semver, `0.1.0` when it has none.
    fn eval_version(&self, installable: &str) -> Version {
        self.nix(
            "EvalFailed",
            &["eval"],
            &["--raw", &format!("{installable}.version")],
        )
        .map(|v| normalize_version(&v))
        .unwrap_or_else(|_| Version::new(0, 1, 0))
    }
}

/// The full argument vector for a `nix` call. Every call refuses the flake's own
/// `nixConfig`, so a foreign flake can never add substituters or trusted keys to
/// the client (003 §FR-023, C-F8) — and never prompts.
fn flake_nix_args(cmd: &[&str], rest: &[&str]) -> Vec<String> {
    cmd.iter()
        .chain(["--option", "accept-flake-config", "false"].iter())
        .chain(rest)
        .map(|s| s.to_string())
        .collect()
}

fn stderr_tail(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.trim().lines().collect();
    let tail = &lines[lines.len().saturating_sub(STDERR_TAIL_LINES)..];
    redact_ref(&tail.join("\n"))
}

#[derive(Deserialize)]
struct FlakeMetadata {
    url: Option<String>,
    revision: Option<String>,
}

/// Extract the pin from `nix flake metadata --json`. Only a source with a
/// `revision` counts as pinned: a `narHash`-only lock (plain directory) stops
/// building the moment the directory changes, which is breakage, not
/// reproducibility (research R2).
fn parse_metadata(json: &str) -> Result<(Option<String>, Option<String>), PluginError> {
    let meta: FlakeMetadata = serde_json::from_str(json)
        .map_err(|e| PluginError::Other(format!("FlakeUnavailable: unreadable metadata: {e}")))?;
    Ok(match (meta.url, meta.revision) {
        (Some(url), Some(rev)) => (Some(url), Some(rev)),
        _ => (None, None),
    })
}

/// Deterministic plugin name for a wrapped program (C-F14, research R7): the
/// explicit override, else the last attribute segment, else — for the default
/// output — the repository or directory name.
fn derive_name(reference: &str, attr: &str, name: Option<&str>) -> Result<String, PluginError> {
    if let Some(n) = name {
        let ok = !n.is_empty()
            && n.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        return if ok {
            Ok(n.to_string())
        } else {
            Err(PluginError::Manifest(format!(
                "invalid plugin name `{n}` (allowed: letters, digits, `-`, `_`, `.`)"
            )))
        };
    }
    let last = attr.rsplit('.').next().unwrap_or(attr);
    let raw = if last != "default" {
        last
    } else {
        let path = reference.split(['?', '#']).next().unwrap_or(reference);
        let path = path.trim_end_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        // `github:owner/repo[/ref]` and friends name the repo second, not last.
        let forge = ["github:", "gitlab:", "sourcehut:"]
            .iter()
            .find_map(|p| path.strip_prefix(p));
        match forge {
            Some(rest) => rest.split('/').nth(1).unwrap_or(rest),
            None => path.rsplit(['/', ':']).next().unwrap_or(path),
        }
    };
    let clean: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let clean = clean.trim_matches('-');
    Ok(if clean.is_empty() {
        "flake-plugin".into()
    } else {
        clean.to_string()
    })
}

/// Coerce a package version into semver: `v1.2.3` → `1.2.3`, `2.42` → `2.42.0`,
/// `7` → `7.0.0`; anything else → `0.1.0`.
fn normalize_version(raw: &str) -> Version {
    let v = raw.trim().trim_start_matches('v');
    [v.to_string(), format!("{v}.0"), format!("{v}.0.0")]
        .iter()
        .find_map(|c| Version::parse(c).ok())
        .unwrap_or_else(|| Version::new(0, 1, 0))
}

fn has_entries(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut d| d.next().is_some())
        .unwrap_or(false)
}

fn io_err(what: &str) -> impl Fn(std::io::Error) -> PluginError + '_ {
    move |e| PluginError::Other(format!("{what}: {e}"))
}

/// Wrap a program output (`bin/`) into a generated plugin (C-F9, C-F12, C-F13).
fn package_program(
    store_dir: &Path,
    staging: &Path,
    name: &str,
    version: &Version,
) -> Result<(), PluginError> {
    copy_tree(&store_dir.join("bin"), &staging.join("bin"))
        .map_err(io_err("packaging flake output"))?;
    // Only bin/ exists at this point; auditing from the root makes the reported
    // paths match the package layout (`bin/<program>`).
    let arch = audit_package(staging)?;

    let mut env_sh = String::from(
        "# generated by xxh flake provider\n\
         export PATH=\"$XXH_COMPONENT_DIR/bin:$PATH\"\n",
    );
    add_runtime_data(staging, &mut env_sh)?;
    std::fs::write(staging.join("env.sh"), env_sh).map_err(io_err("packaging flake output"))?;

    // Mark the platform the binaries were built for, so the session skips the
    // plugin on any other target instead of shipping an unusable binary (§FR-012).
    let target = match arch {
        Some(a) => format!("linux/{a}"),
        None => "linux".into(),
    };
    std::fs::write(
        staging.join("plugin.toml"),
        format!(
            "name = \"{name}\"\nversion = \"{version}\"\napi_version = \"1.0.0\"\n\
             targets = [\"{target}\"]\n"
        ),
    )
    .map_err(io_err("packaging flake output"))
}

/// Take a ready-made plugin output as is (C-F9, C-F10, C-F12).
fn package_plugin(store_dir: &Path, staging: &Path) -> Result<(), PluginError> {
    copy_tree(store_dir, staging).map_err(io_err("packaging flake output"))?;
    read_manifest(staging)?;
    if let Some(arch) = audit_package(staging)? {
        let path = staging.join("plugin.toml");
        let text = std::fs::read_to_string(&path).map_err(io_err("reading plugin.toml"))?;
        if let Some(pinned) = with_default_targets(&text, arch)? {
            std::fs::write(&path, pinned).map_err(io_err("writing plugin.toml"))?;
        }
    }
    Ok(())
}

/// A manifest that ships binaries but declares no `targets` gets the binaries'
/// platform; every other field — including ones this client does not know — is
/// preserved. `None` when the author already declared targets.
fn with_default_targets(manifest: &str, arch: &str) -> Result<Option<String>, PluginError> {
    let mut table: toml::Table =
        toml::from_str(manifest).map_err(|e| PluginError::Manifest(e.to_string()))?;
    if table.contains_key("targets") {
        return Ok(None);
    }
    table.insert(
        "targets".into(),
        toml::Value::Array(vec![toml::Value::String(format!("linux/{arch}"))]),
    );
    toml::to_string(&table)
        .map(Some)
        .map_err(|e| PluginError::Manifest(e.to_string()))
}

#[async_trait::async_trait]
impl PackageSource for FlakeProvider {
    fn id(&self) -> &'static str {
        "flake"
    }

    fn availability(&self) -> Availability {
        match nix_available_as(&self.nix_bin) {
            Ok(()) => Availability::Available,
            Err(reason) => Availability::Unavailable { reason },
        }
    }

    fn supports_target(&self, os: &str, arch: &str, _libc: &str) -> Support {
        if os == "linux" {
            Support::Supported
        } else {
            Support::Unsupported {
                reason: format!(
                    "flake plugins are delivered as static Linux artefacts; \
                     host {os}/{arch} is not supported"
                ),
            }
        }
    }

    async fn fetch(&self, spec: &SourceSpec) -> Result<FetchedPackage, PluginError> {
        let SourceSpec::Flake {
            reference,
            attr,
            locked_url,
            revision,
            name,
        } = spec
        else {
            return Err(PluginError::Other(
                "flake provider got a non-flake spec".into(),
            ));
        };
        if let Err(reason) = nix_available_as(&self.nix_bin) {
            return Err(PluginError::SourceUnavailable(reason));
        }

        // A spec that already carries a pin builds exactly that revision; only an
        // unpinned one (first install, `plugin update`) is resolved (C-F7, C-F19).
        let (locked_url, revision) = match locked_url {
            Some(url) => (Some(url.clone()), revision.clone()),
            None => self.resolve_pin(reference)?,
        };
        let installable = format!("{}#{attr}", locked_url.as_deref().unwrap_or(reference));

        // --no-link keeps the client's working directory clean. An output that is
        // already built costs an evaluation only, so re-installs stay cheap (§FR-019).
        let out = self.nix(
            "BuildFailed",
            &["build"],
            &[&installable, "--no-link", "--print-out-paths"],
        )?;
        let outputs: Vec<PathBuf> = out.lines().map(|l| PathBuf::from(l.trim())).collect();
        let (store_dir, is_plugin) = outputs
            .iter()
            .find(|p| p.join("plugin.toml").is_file())
            .map(|p| (p, true))
            .or_else(|| {
                outputs
                    .iter()
                    .find(|p| has_entries(&p.join("bin")))
                    .map(|p| (p, false))
            })
            .ok_or_else(|| {
                PluginError::Other(format!(
                    "BadOutput: `{}#{attr}` has neither a plugin.toml at its root nor \
                     programs under bin/; point at an output that is a plugin package \
                     or a program",
                    redact_ref(reference)
                ))
            })?;
        if is_plugin && name.is_some() {
            return Err(PluginError::Other(format!(
                "`{}#{attr}` is a ready-made plugin: its name comes from its plugin.toml, \
                 --name does not apply",
                redact_ref(reference)
            )));
        }

        let plugin_name = if is_plugin {
            String::new()
        } else {
            derive_name(reference, attr, name.as_deref())?
        };
        let shape = if is_plugin { "plugin" } else { "program" };
        let key = blake3::hash(format!("{}|{shape}|{plugin_name}", store_dir.display()).as_bytes())
            .to_hex()
            .to_string();
        let cache = client_cache_dir()?.join(&key);

        if !cache.join("plugin.toml").is_file() {
            let staging = cache.with_extension("tmp");
            let _ = std::fs::remove_dir_all(&staging);
            let packaged = if is_plugin {
                package_plugin(store_dir, &staging)
            } else {
                package_program(
                    store_dir,
                    &staging,
                    &plugin_name,
                    &self.eval_version(&installable),
                )
            };
            // Nothing reaches the cache (or the registry) unless the package is
            // complete and passed the audit (§FR-022, C-F11, C-F15).
            packaged
                .and_then(|()| {
                    std::fs::create_dir_all(cache.parent().unwrap_or(&cache))
                        .and_then(|_| std::fs::rename(&staging, &cache))
                        .map_err(io_err("caching flake package"))
                })
                .inspect_err(|_| {
                    let _ = std::fs::remove_dir_all(&staging);
                })?;
        }

        let manifest = read_manifest(&cache)?;
        Ok(FetchedPackage {
            manifest,
            dir: cache,
            env: BTreeMap::new(), // env is carried in env.sh (sourced on the host)
            cleanup: None,        // the client cache entry is reused
            resolved: Some(SourceSpec::Flake {
                reference: reference.clone(),
                attr: attr.clone(),
                locked_url,
                revision,
                name: name.clone(),
            }),
            revision: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_nix_call_refuses_flake_config() {
        for (cmd, rest) in [
            (&["flake", "metadata"][..], &["--json", "github:o/r"][..]),
            (&["build"][..], &["github:o/r#x", "--no-link"][..]),
            (&["eval"][..], &["--raw", "github:o/r#x.version"][..]),
        ] {
            let args = flake_nix_args(cmd, rest);
            let at = args
                .iter()
                .position(|a| a == "--option")
                .expect("--option present");
            assert_eq!(args[at + 1], "accept-flake-config");
            assert_eq!(args[at + 2], "false");
            // The subcommand stays first, the user-controlled part stays last.
            assert_eq!(&args[..cmd.len()], cmd);
            assert_eq!(&args[args.len() - rest.len()..], rest);
        }
    }

    #[test]
    fn names_are_derived_deterministically() {
        let n = |r, a| derive_name(r, a, None).unwrap();
        assert_eq!(n("github:NixOS/nixpkgs", "pkgsStatic.ripgrep"), "ripgrep");
        assert_eq!(n("github:o/my-tool", "default"), "my-tool");
        assert_eq!(n("github:o/my-tool/some-branch", "default"), "my-tool");
        assert_eq!(n("git+https://h/o/Tool.git?ref=main", "default"), "tool");
        assert_eq!(n("/home/me/src/my_flake/", "default"), "my-flake");
        assert_eq!(n("github:o/r", "packages.x86_64-linux.default"), "r");
        assert_eq!(
            derive_name("github:o/r", "x", Some("mine")).unwrap(),
            "mine"
        );
        assert!(derive_name("github:o/r", "x", Some("bad name")).is_err());
        assert!(derive_name("github:o/r", "x", Some("")).is_err());
    }

    #[test]
    fn versions_are_coerced_to_semver() {
        assert_eq!(normalize_version("14.1.1"), Version::new(14, 1, 1));
        assert_eq!(normalize_version("v1.2.3"), Version::new(1, 2, 3));
        assert_eq!(normalize_version("2.42"), Version::new(2, 42, 0));
        assert_eq!(normalize_version("7"), Version::new(7, 0, 0));
        assert_eq!(
            normalize_version("unstable-2024-01-01"),
            Version::new(0, 1, 0)
        );
        assert_eq!(normalize_version(""), Version::new(0, 1, 0));
    }

    #[test]
    fn only_a_revision_makes_a_pin() {
        // A git source: pinned by revision.
        let (url, rev) = parse_metadata(
            r#"{"url":"git+file:///r?ref=refs/heads/main&rev=abc","revision":"abc","locked":{}}"#,
        )
        .unwrap();
        assert_eq!(
            url.as_deref(),
            Some("git+file:///r?ref=refs/heads/main&rev=abc")
        );
        assert_eq!(rev.as_deref(), Some("abc"));
        // A plain directory or a dirty tree: a narHash-only lock is not a pin.
        for unpinned in [
            r#"{"url":"path:/r?narHash=sha256-x"}"#,
            r#"{"url":"git+file:///r","revision":null,"dirtyRevision":"abc-dirty"}"#,
        ] {
            assert_eq!(parse_metadata(unpinned).unwrap(), (None, None));
        }
        assert!(parse_metadata("not json").is_err());
    }

    #[test]
    fn default_targets_keep_the_rest_of_the_manifest() {
        let manifest = "name = \"demo\"\nversion = \"2.0.0\"\napi_version = \"1.0.0\"\n\
                        future_field = \"kept\"\n\n[hooks.post_deploy]\nrun = \"h.sh\"\n";
        let out = with_default_targets(manifest, "aarch64").unwrap().unwrap();
        let m = xxh_plugin_api::Manifest::parse(&out).unwrap();
        assert_eq!(m.targets, vec!["linux/aarch64".to_string()]);
        assert_eq!(m.name, "demo");
        assert!(
            m.hooks
                .contains_key(&xxh_plugin_api::LifecycleStage::PostDeploy)
        );
        assert!(
            out.contains("future_field"),
            "unknown fields survive: {out}"
        );
        // The author's own targets are never overridden.
        let own = format!("targets = [\"linux\"]\n{manifest}");
        assert!(with_default_targets(&own, "aarch64").unwrap().is_none());
    }

    #[test]
    fn stderr_tail_is_bounded_and_redacted() {
        let noise = (0..50)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let tail = stderr_tail(&format!("{noise}\nerror: cannot fetch https://u:p@h/x\n"));
        assert_eq!(tail.lines().count(), STDERR_TAIL_LINES);
        assert!(
            tail.contains("cannot fetch") && !tail.contains("u:p"),
            "got: {tail}"
        );
        assert!(!tail.contains("line 0\n"));
    }

    /// No Nix on the client disables only this source (C-F5, 003 §FR-020).
    #[tokio::test]
    async fn missing_nix_is_source_unavailable() {
        let p = FlakeProvider::with_nix_bin("xxh-test-no-such-nix");
        assert!(matches!(p.availability(), Availability::Unavailable { .. }));
        let spec = SourceSpec::parse("flake:github:o/r#x").unwrap();
        assert!(matches!(
            p.fetch(&spec).await,
            Err(PluginError::SourceUnavailable(_))
        ));
    }

    #[test]
    fn only_linux_targets_are_supported() {
        let p = FlakeProvider::new();
        assert_eq!(
            p.supports_target("linux", "aarch64", "musl"),
            Support::Supported
        );
        assert!(matches!(
            p.supports_target("darwin", "aarch64", "unknown"),
            Support::Unsupported { .. }
        ));
    }
}
