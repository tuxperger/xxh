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
// nixos-25.05: pkgsStatic coverage is notably better than 24.05 (e.g. htop
// fails to build statically on 24.05).
const DEFAULT_NIXPKGS_PIN: &str = "github:NixOS/nixpkgs/nixos-25.05";

pub struct NixProvider;

impl NixProvider {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self
    }
}

pub(crate) fn nixpkgs_pin() -> String {
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

pub(crate) fn nix_available() -> Result<(), String> {
    nix_available_as("nix")
}

/// [`nix_available`] for an explicit program name — the seam that lets tests
/// exercise the "no Nix on this client" path without touching `$PATH`.
pub(crate) fn nix_available_as(program: &str) -> Result<(), String> {
    let out = std::process::Command::new(program)
        .args(["--version"])
        .output();
    match out {
        Ok(o) if o.status.success() => {}
        _ => return Err("`nix` was not found on this client".into()),
    }
    let flakes = std::process::Command::new(program)
        .args(["flake", "metadata", "--help"])
        .output();
    match flakes {
        Ok(o) if o.status.success() => Ok(()),
        _ => Err("nix is present but flakes are not enabled".into()),
    }
}

/// Static audit (§FR-038, C-N1): every ELF in the artefact must be statically
/// linked — no `PT_INTERP` (dynamic loader) program header. Any hit ⇒ the
/// package is not statically self-sufficient ⇒ error class plugin ("NotStatic").
///
/// Note: a *string* mentioning `/nix/store/` inside a static binary is fine —
/// e.g. ncurses bakes its default terminfo search path in; the provider ships
/// terminfo/cacert separately and overrides via env (§FR-037), so only real
/// dynamic linkage makes an artefact non-self-sufficient.
pub fn audit_static(dir: &Path) -> Result<(), PluginError> {
    fn walk(dir: &Path, hits: &mut Vec<String>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(&path, hits)?;
            } else if elf_has_interp(&std::fs::read(&path)?).unwrap_or(false) {
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
            "NotStatic: dynamically linked ELF (has an interpreter) in artefact: {}; \
             the package is not statically self-sufficient (FR-038)",
            hits.join(", ")
        )))
    }
}

/// Self-sufficiency audit for a whole package (003 §FR-010, C-F11): besides the
/// static-linkage rule of [`audit_static`], no executable script may point its
/// shebang into `/nix/store` (absent on every target), and all ELF files must
/// share one known architecture. Returns that architecture (`x86_64` | `aarch64`
/// | `armv7l`), or `None` when the package ships no ELF at all.
pub(crate) fn audit_package(dir: &Path) -> Result<Option<&'static str>, PluginError> {
    fn executable(path: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(path)
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            true
        }
    }
    fn walk(
        base: &Path,
        dir: &Path,
        bad: &mut Vec<String>,
        archs: &mut std::collections::BTreeSet<&'static str>,
    ) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(base, &path, bad, archs)?;
                continue;
            }
            let rel = path.strip_prefix(base).unwrap_or(&path).display();
            let data = std::fs::read(&path)?;
            if let Some(elf) = elf_info(&data) {
                if elf.interp {
                    bad.push(format!("{rel} (dynamically linked)"));
                }
                match elf.machine {
                    62 => drop(archs.insert("x86_64")),
                    183 => drop(archs.insert("aarch64")),
                    40 => drop(archs.insert("armv7l")),
                    other => bad.push(format!("{rel} (unsupported architecture {other})")),
                }
            } else if data.starts_with(b"#!") && executable(&path) {
                let line = data.split(|b| *b == b'\n').next().unwrap_or_default();
                if line.windows(11).any(|w| w == b"/nix/store/") {
                    bad.push(format!("{rel} (interpreter in /nix/store)"));
                }
            }
        }
        Ok(())
    }
    let mut bad = Vec::new();
    let mut archs = std::collections::BTreeSet::new();
    walk(dir, dir, &mut bad, &mut archs)
        .map_err(|e| PluginError::Other(format!("self-sufficiency audit: {e}")))?;
    if archs.len() > 1 {
        bad.push(format!(
            "mixed architectures ({})",
            archs.iter().copied().collect::<Vec<_>>().join(", ")
        ));
    }
    if bad.is_empty() {
        return Ok(archs.into_iter().next());
    }
    bad.sort();
    Err(PluginError::Other(format!(
        "NotSelfContained: {}; a target has neither Nix nor these libraries, so the \
         flake output must be statically linked and must not reference /nix/store \
         (use a static output, e.g. `pkgsStatic.<package>`)",
        bad.join(", ")
    )))
}

/// What the audit needs from an ELF header: whether it asks for a dynamic loader
/// and which machine it was built for.
struct ElfInfo {
    interp: bool,
    machine: u16,
}

/// `Some(true)` iff `data` is an ELF with a `PT_INTERP` program header
/// (i.e. dynamically linked); `None` for non-ELF files.
fn elf_has_interp(data: &[u8]) -> Option<bool> {
    elf_info(data).map(|e| e.interp)
}

/// Parse the parts of an ELF header the audits rely on; `None` for non-ELF files.
fn elf_info(data: &[u8]) -> Option<ElfInfo> {
    const PT_INTERP: u32 = 3;
    if data.len() < 0x40 || &data[..4] != b"\x7fELF" {
        return None;
    }
    let is64 = match data[4] {
        1 => false,
        2 => true,
        _ => return None,
    };
    let le = match data[5] {
        1 => true,
        2 => false,
        _ => return None,
    };
    let u16at = |off: usize| -> Option<u64> {
        let b: [u8; 2] = data.get(off..off + 2)?.try_into().ok()?;
        Some(u64::from(if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        }))
    };
    let u32at = |off: usize| -> Option<u32> {
        let b: [u8; 4] = data.get(off..off + 4)?.try_into().ok()?;
        Some(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    let phoff = if is64 {
        let b: [u8; 8] = data.get(0x20..0x28)?.try_into().ok()?;
        if le {
            u64::from_le_bytes(b)
        } else {
            u64::from_be_bytes(b)
        }
    } else {
        u64::from(u32at(0x1c)?)
    };
    let (phentsize, phnum) = if is64 {
        (u16at(0x36)?, u16at(0x38)?)
    } else {
        (u16at(0x2a)?, u16at(0x2c)?)
    };
    let machine = u16::try_from(u16at(0x12)?).ok()?;
    let mut interp = false;
    for i in 0..phnum {
        let off = usize::try_from(phoff + i * phentsize).ok()?;
        if u32at(off)? == PT_INTERP {
            interp = true;
            break;
        }
    }
    Some(ElfInfo { interp, machine })
}

pub(crate) fn run_nix(args: &[&str]) -> Result<String, PluginError> {
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

pub(crate) fn client_cache_dir() -> Result<PathBuf, PluginError> {
    if let Some(v) = std::env::var_os("XXH_NIX_CACHE_DIR") {
        return Ok(PathBuf::from(v));
    }
    Ok(directories::BaseDirs::new()
        .ok_or_else(|| PluginError::Other("cannot determine data directory".into()))?
        .data_dir()
        .join("xxh")
        .join("nix-cache"))
}

/// Runtime data a static binary cannot embed (§FR-037, C-N2; 003 §FR-011): the CA
/// bundle and terminfo from the pinned nixpkgs are copied next to the binaries
/// and wired through `env_sh`, so nothing on the target resolves into `/nix/store`.
/// Best-effort per item: a package that cannot be built is simply left out.
pub(crate) fn add_runtime_data(staging: &Path, env_sh: &mut String) -> Result<(), PluginError> {
    let pin = nixpkgs_pin();
    // A package may have several outputs; pick the one that holds the data.
    let output_with = |attr: &str, rel: &str| -> Option<PathBuf> {
        let out = run_nix(&[
            "build",
            &format!("{pin}#{attr}"),
            "--no-link",
            "--print-out-paths",
        ])
        .ok()?;
        out.lines()
            .map(|l| PathBuf::from(l.trim()).join(rel))
            .find(|p| p.exists())
    };
    if let Some(bundle) = output_with("cacert", "etc/ssl/certs/ca-bundle.crt") {
        let dst = staging.join("etc/ssl/certs");
        std::fs::create_dir_all(&dst)
            .and_then(|_| std::fs::copy(&bundle, dst.join("ca-bundle.crt")))
            .map_err(|e| PluginError::Other(format!("runtime data: {e}")))?;
        env_sh
            .push_str("export SSL_CERT_FILE=\"$XXH_COMPONENT_DIR/etc/ssl/certs/ca-bundle.crt\"\n");
    }
    if let Some(terminfo) = output_with("ncurses", "share/terminfo") {
        copy_tree(&terminfo, &staging.join("share/terminfo"))
            .map_err(|e| PluginError::Other(format!("runtime data: {e}")))?;
        env_sh.push_str("export TERMINFO=\"$XXH_COMPONENT_DIR/share/terminfo\"\n");
    }
    Ok(())
}

pub(crate) fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
            // Store paths are read-only; the copy must be writable for cleanup —
            // by the owner only, never group/world.
            let mut perm = std::fs::metadata(&dst)?.permissions();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                perm.set_mode(perm.mode() | 0o200);
            }
            #[cfg(not(unix))]
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
        let pkg_ref = format!("{}#pkgsStatic.{attr}", nixpkgs_pin());

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
            audit_static(&staging.join("bin")).inspect_err(|_| {
                let _ = std::fs::remove_dir_all(&staging);
            })?;

            let mut env_sh = String::from(
                "# generated by xxh nix provider\n\
                 export PATH=\"$XXH_COMPONENT_DIR/bin:$PATH\"\n",
            );
            add_runtime_data(&staging, &mut env_sh)?;
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
            resolved: None,
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

    /// Fabricate a minimal ELF64-LE with the given program-header types.
    fn fake_elf(ptypes: &[u32]) -> Vec<u8> {
        let mut d = vec![0u8; 0x40 + ptypes.len() * 0x38];
        d[..4].copy_from_slice(b"\x7fELF");
        d[4] = 2; // 64-bit
        d[5] = 1; // little-endian
        d[0x20..0x28].copy_from_slice(&0x40u64.to_le_bytes()); // e_phoff
        d[0x36..0x38].copy_from_slice(&0x38u16.to_le_bytes()); // e_phentsize
        d[0x38..0x3a].copy_from_slice(&(ptypes.len() as u16).to_le_bytes()); // e_phnum
        for (i, pt) in ptypes.iter().enumerate() {
            let off = 0x40 + i * 0x38;
            d[off..off + 4].copy_from_slice(&pt.to_le_bytes());
        }
        d
    }

    #[test]
    fn static_audit_flags_only_dynamic_elves() {
        let dir = std::env::temp_dir().join(format!("xxh-audit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Static ELF (PT_LOAD only) — even with a baked /nix/store string — is fine.
        let mut ok = fake_elf(&[1]); // PT_LOAD
        ok.extend_from_slice(b"/nix/store/abc-ncurses/share/terminfo");
        std::fs::write(dir.join("ok.bin"), ok).unwrap();
        std::fs::write(dir.join("data.txt"), b"not an elf at all").unwrap();
        assert!(audit_static(&dir).is_ok());
        // A PT_INTERP header means a dynamic loader is required → NotStatic.
        std::fs::write(dir.join("bad.bin"), fake_elf(&[1, 3])).unwrap();
        let err = audit_static(&dir).unwrap_err();
        assert!(err.to_string().contains("NotStatic"), "got: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// [`fake_elf`] built for a given `e_machine`.
    fn fake_elf_for(machine: u16, ptypes: &[u32]) -> Vec<u8> {
        let mut d = fake_elf(ptypes);
        d[0x12..0x14].copy_from_slice(&machine.to_le_bytes());
        d
    }

    fn audit_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xxh-pkgaudit-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        dir
    }

    fn write_exec(path: &Path, data: &[u8]) {
        std::fs::write(path, data).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn package_audit_reports_the_elf_architecture() {
        let dir = audit_dir("arch");
        write_exec(&dir.join("bin/tool"), &fake_elf_for(62, &[1]));
        assert_eq!(audit_package(&dir).unwrap(), Some("x86_64"));
        write_exec(&dir.join("bin/tool"), &fake_elf_for(183, &[1]));
        assert_eq!(audit_package(&dir).unwrap(), Some("aarch64"));
        write_exec(&dir.join("bin/tool"), &fake_elf_for(40, &[1]));
        assert_eq!(audit_package(&dir).unwrap(), Some("armv7l"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn package_audit_accepts_portable_scripts_and_data() {
        let dir = audit_dir("script");
        write_exec(&dir.join("bin/run"), b"#!/bin/sh\necho hi\n");
        // Non-executable data may mention the store freely.
        std::fs::write(dir.join("notes.txt"), b"#!/nix/store/x/bin/sh\n").unwrap();
        assert_eq!(
            audit_package(&dir).unwrap(),
            None,
            "no ELF ⇒ no architecture"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn package_audit_rejects_what_cannot_run_on_a_target() {
        let dir = audit_dir("bad");
        // Dynamically linked ELF.
        write_exec(&dir.join("bin/dyn"), &fake_elf_for(62, &[1, 3]));
        let err = audit_package(&dir).unwrap_err().to_string();
        assert!(err.contains("NotSelfContained"), "got: {err}");
        assert!(err.contains("bin/dyn (dynamically linked)"), "got: {err}");
        assert!(
            err.contains("pkgsStatic"),
            "the hint must name a fix: {err}"
        );
        std::fs::remove_file(dir.join("bin/dyn")).unwrap();

        // Script whose interpreter lives in the store.
        write_exec(
            &dir.join("bin/wrap"),
            b"#!/nix/store/abc-bash/bin/bash\nexec x\n",
        );
        let err = audit_package(&dir).unwrap_err().to_string();
        assert!(
            err.contains("bin/wrap (interpreter in /nix/store)"),
            "got: {err}"
        );
        std::fs::remove_file(dir.join("bin/wrap")).unwrap();

        // Two architectures in one package.
        write_exec(&dir.join("bin/a"), &fake_elf_for(62, &[1]));
        write_exec(&dir.join("bin/b"), &fake_elf_for(183, &[1]));
        let err = audit_package(&dir).unwrap_err().to_string();
        assert!(err.contains("mixed architectures"), "got: {err}");
        std::fs::remove_file(dir.join("bin/b")).unwrap();

        // An architecture no supported target has.
        write_exec(&dir.join("bin/a"), &fake_elf_for(243, &[1]));
        let err = audit_package(&dir).unwrap_err().to_string();
        assert!(err.contains("unsupported architecture 243"), "got: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
