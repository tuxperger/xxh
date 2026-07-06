//! ⭐ NixProvider — static plugin builds from nixpkgs (T050–T052, feature `nix-source`).
//!
//! Builds a nixpkgs attribute into a **fully static** artefact via `pkgsStatic`
//! (cross targets via `pkgsCross.<t>.pkgsStatic`) and packages it as an ordinary
//! plugin: the host needs neither Nix nor root (§FR-033..040,
//! contracts/nix-provider.md). Nix is required **only on the client**; when it is
//! absent the provider degrades to `Unavailable` and everything else keeps
//! working (C-N6, Принцип IX).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use xxh_plugin_api::PluginError;

use crate::source::{Availability, FetchedPackage, PackageSource, SourceSpec, Support};

/// Pinned nixpkgs for reproducible builds (C-N4). Overridable for spikes/tests via
/// `$XXH_NIXPKGS_PIN`; updating the default is a controlled operation tied to the
/// repo's own flake pin (plan §Пиннинг).
const DEFAULT_NIXPKGS_PIN: &str = "github:NixOS/nixpkgs/nixos-24.05";

pub struct NixProvider;

impl NixProvider {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self
    }
}

fn nixpkgs_pin() -> String {
    std::env::var("XXH_NIXPKGS_PIN").unwrap_or_else(|_| DEFAULT_NIXPKGS_PIN.to_string())
}

/// Map a host platform triple to the Nix package-set expression (contract table).
/// `None` ⇒ unsupported target (non-Linux, exotic arch) — diagnosed before any
/// build (C-N1, §FR-039).
pub fn nix_target(os: &str, arch: &str) -> Option<&'static str> {
    if os != "linux" {
        return None;
    }
    match arch {
        "x86_64" => Some("pkgsStatic"),
        "aarch64" => Some("pkgsCross.aarch64-multiplatform.pkgsStatic"),
        "armv7l" | "arm" => Some("pkgsCross.armv7l-hf-multiplatform.pkgsStatic"),
        _ => None,
    }
}

fn nix_available() -> Result<(), String> {
    let out = std::process::Command::new("nix")
        .args(["--version"])
        .output();
    match out {
        Ok(o) if o.status.success() => {}
        _ => return Err("`nix` was not found on this client".into()),
    }
    let flakes = std::process::Command::new("nix")
        .args(["flake", "metadata", "--help"])
        .output();
    match flakes {
        Ok(o) if o.status.success() => Ok(()),
        _ => Err("nix is present but flakes are not enabled".into()),
    }
}

/// Static audit (§FR-038, C-N1): a self-contained artefact must carry no runtime
/// references into `/nix/store`. Any hit ⇒ the package is not statically
/// self-sufficient ⇒ error class plugin ("NotStatic").
pub fn audit_no_store_refs(dir: &Path) -> Result<(), PluginError> {
    fn walk(dir: &Path, hits: &mut Vec<String>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(&path, hits)?;
            } else if std::fs::read(&path)?
                .windows(b"/nix/store/".len())
                .any(|w| w == b"/nix/store/")
            {
                hits.push(path.display().to_string());
            }
        }
        Ok(())
    }
    let mut hits = Vec::new();
    walk(dir, &mut hits).map_err(|e| PluginError::Other(format!("static audit: {e}")))?;
    if hits.is_empty() {
        Ok(())
    } else {
        Err(PluginError::Other(format!(
            "NotStatic: artefact still references /nix/store at runtime ({}); \
             the package is not statically self-sufficient (FR-038)",
            hits.join(", ")
        )))
    }
}

fn run_nix(args: &[&str]) -> Result<String, PluginError> {
    let out = std::process::Command::new("nix")
        .args(args)
        .output()
        .map_err(|e| PluginError::Other(format!("spawning nix: {e}")))?;
    if !out.status.success() {
        return Err(PluginError::Other(format!(
            "BuildFailed: nix {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn client_cache_dir() -> Result<PathBuf, PluginError> {
    if let Some(v) = std::env::var_os("XXH_NIX_CACHE_DIR") {
        return Ok(PathBuf::from(v));
    }
    Ok(directories::BaseDirs::new()
        .ok_or_else(|| PluginError::Other("cannot determine data directory".into()))?
        .data_dir()
        .join("xxh")
        .join("nix-cache"))
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
            // Store paths are read-only; the copy must be writable for cleanup.
            let mut perm = std::fs::metadata(&dst)?.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            std::fs::set_permissions(&dst, perm)?;
        }
    }
    Ok(())
}

#[async_trait::async_trait]
impl PackageSource for NixProvider {
    fn id(&self) -> &'static str {
        "nix"
    }

    fn availability(&self) -> Availability {
        match nix_available() {
            Ok(()) => Availability::Available,
            Err(reason) => Availability::Unavailable { reason },
        }
    }

    fn supports_target(&self, os: &str, arch: &str, _libc: &str) -> Support {
        match nix_target(os, arch) {
            Some(_) => Support::Supported,
            None => Support::Unsupported {
                reason: format!(
                    "nix static builds target Linux only (pkgsStatic/musl); \
                     host {os}/{arch} is not supported (§FR-039)"
                ),
            },
        }
    }

    /// Build `nixpkgs:<attr>` into a static, relocatable plugin package.
    ///
    /// The build targets the client's native architecture (`pkgsStatic`); cross
    /// targets follow the same table via [`nix_target`] once the resolver passes
    /// the host platform through (research R11 follow-up).
    async fn fetch(&self, spec: &SourceSpec) -> Result<FetchedPackage, PluginError> {
        let SourceSpec::Nix { attr } = spec else {
            return Err(PluginError::Other("nix provider got a non-nix spec".into()));
        };
        if let Err(reason) = nix_available() {
            return Err(PluginError::SourceUnavailable(reason));
        }
        let pin = nixpkgs_pin();
        let pkg_ref = format!("{pin}#pkgsStatic.{attr}");

        // Deterministic client-side cache key for (spec, pin, target) — C-N4.
        let key = blake3::hash(format!("{pkg_ref}|x86_64").as_bytes())
            .to_hex()
            .to_string();
        let cache = client_cache_dir()?.join(&key);

        if !cache.join("plugin.toml").is_file() {
            // `nix build` with the pinned nixpkgs; --no-link keeps the client clean.
            let out_path = run_nix(&["build", &pkg_ref, "--no-link", "--print-out-paths"])?;
            let store_dir = PathBuf::from(
                out_path
                    .lines()
                    .last()
                    .ok_or_else(|| PluginError::Other("BuildFailed: no output path".into()))?,
            );

            let staging = cache.with_extension("tmp");
            let _ = std::fs::remove_dir_all(&staging);
            let bin_src = store_dir.join("bin");
            if !bin_src.is_dir() {
                return Err(PluginError::Other(format!(
                    "BuildFailed: `{attr}` produced no bin/ output"
                )));
            }
            copy_tree(&bin_src, &staging.join("bin"))
                .map_err(|e| PluginError::Other(format!("packaging nix artefact: {e}")))?;

            // The artefact must be statically self-sufficient (§FR-038).
            audit_no_store_refs(&staging.join("bin")).inspect_err(|_| {
                let _ = std::fs::remove_dir_all(&staging);
            })?;

            // Runtime data the static binary cannot embed (§FR-037, C-N2):
            // terminfo + CA bundle, wired through env in the shell init.
            let mut env_sh = String::from(
                "# generated by xxh nix provider\n\
                 export PATH=\"$XXH_COMPONENT_DIR/bin:$PATH\"\n",
            );
            if let Ok(cacert) = run_nix(&[
                "build",
                &format!("{pin}#cacert"),
                "--no-link",
                "--print-out-paths",
            ]) {
                let bundle = PathBuf::from(cacert.trim()).join("etc/ssl/certs/ca-bundle.crt");
                if bundle.is_file() {
                    let dst = staging.join("etc/ssl/certs");
                    std::fs::create_dir_all(&dst)
                        .and_then(|_| std::fs::copy(&bundle, dst.join("ca-bundle.crt")))
                        .map_err(|e| PluginError::Other(format!("runtime data: {e}")))?;
                    env_sh.push_str(
                        "export SSL_CERT_FILE=\"$XXH_COMPONENT_DIR/etc/ssl/certs/ca-bundle.crt\"\n",
                    );
                }
            }
            if let Ok(ncurses) = run_nix(&[
                "build",
                &format!("{pin}#ncurses"),
                "--no-link",
                "--print-out-paths",
            ]) {
                let terminfo = PathBuf::from(ncurses.trim()).join("share/terminfo");
                if terminfo.is_dir() {
                    copy_tree(&terminfo, &staging.join("share/terminfo"))
                        .map_err(|e| PluginError::Other(format!("runtime data: {e}")))?;
                    env_sh.push_str("export TERMINFO=\"$XXH_COMPONENT_DIR/share/terminfo\"\n");
                }
            }
            std::fs::write(staging.join("env.sh"), env_sh)
                .map_err(|e| PluginError::Other(format!("packaging nix artefact: {e}")))?;

            let name = format!("nix-{}", attr.replace('.', "-"));
            std::fs::write(
                staging.join("plugin.toml"),
                format!(
                    "name = \"{name}\"\nversion = \"0.1.0\"\napi_version = \"1.0.0\"\n\
                     targets = [\"linux\"]\n"
                ),
            )
            .map_err(|e| PluginError::Other(format!("packaging nix artefact: {e}")))?;

            std::fs::create_dir_all(cache.parent().unwrap())
                .and_then(|_| std::fs::rename(&staging, &cache))
                .map_err(|e| PluginError::Other(format!("caching nix artefact: {e}")))?;
        }

        let manifest = crate::source::read_manifest(&cache)?;
        Ok(FetchedPackage {
            manifest,
            dir: cache,
            env: BTreeMap::new(), // env is carried in env.sh (sourced on the host)
            cleanup: None,        // the client nix-cache entry is reused (C-N4)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_table_matches_contract() {
        assert_eq!(nix_target("linux", "x86_64"), Some("pkgsStatic"));
        assert_eq!(
            nix_target("linux", "aarch64"),
            Some("pkgsCross.aarch64-multiplatform.pkgsStatic")
        );
        assert_eq!(
            nix_target("linux", "armv7l"),
            Some("pkgsCross.armv7l-hf-multiplatform.pkgsStatic")
        );
        assert_eq!(nix_target("darwin", "x86_64"), None);
        assert_eq!(nix_target("freebsd", "x86_64"), None);
    }

    #[test]
    fn non_linux_is_unsupported_before_any_build() {
        let p = NixProvider::new();
        assert!(matches!(
            p.supports_target("darwin", "aarch64", "unknown"),
            Support::Unsupported { .. }
        ));
        assert!(matches!(
            p.supports_target("linux", "x86_64", "musl"),
            Support::Supported
        ));
    }

    #[test]
    fn store_ref_audit_flags_dynamic_artefacts() {
        let dir = std::env::temp_dir().join(format!("xxh-audit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ok.bin"), b"\x7fELF static content").unwrap();
        assert!(audit_no_store_refs(&dir).is_ok());
        std::fs::write(dir.join("bad.bin"), b"\x7fELF /nix/store/abc-libc.so").unwrap();
        assert!(audit_no_store_refs(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
