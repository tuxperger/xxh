//! Locating installed shell packages (T020, §FR-008..011, Принцип IV).
//!
//! Shells are ordinary packages with `provides.shell = "<name>"` in their
//! manifest and a relocatable tree per platform under `dist/<os>-<arch>/`
//! (see the `xxh-shell-zsh` repo). The search path is `~/.local/share/xxh/shells`;
//! when `$XXH_SHELLS_DIR` (colon-separated) is set it **replaces** the search
//! path entirely — an explicit override that also keeps tests hermetic.

use std::path::PathBuf;

use xxh_plugin_api::Manifest;

use crate::ShellError;
use crate::platform::Platform;

/// A shell package resolved for a concrete host platform: `tree` is the
/// self-contained directory to deliver; `bin` is the shell path inside it.
#[derive(Debug, Clone)]
pub struct ShellPackage {
    pub manifest: Manifest,
    pub tree: PathBuf,
    pub bin_rel: String,
}

fn search_dirs() -> Vec<PathBuf> {
    if let Ok(v) = std::env::var("XXH_SHELLS_DIR") {
        return std::env::split_paths(&v).collect();
    }
    let mut dirs = Vec::new();
    if let Some(bd) = directories::BaseDirs::new() {
        dirs.push(bd.data_dir().join("xxh").join("shells"));
    }
    dirs
}

/// Find a package providing `shell` with a payload for `platform`.
///
/// `Ok(None)` means "no package" (the caller may still find the shell on the
/// host); a present-but-broken manifest is a [`ShellError`] so the user learns
/// why their package was skipped.
pub fn find(shell: &str, platform: &Platform) -> Result<Option<ShellPackage>, ShellError> {
    let target = platform.target_key();
    for base in search_dirs() {
        let dir = base.join(shell);
        let manifest_path = dir.join("manifest.toml");
        if !manifest_path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|e| ShellError::Other(format!("{}: {e}", manifest_path.display())))?;
        let manifest = Manifest::parse(&text)
            .map_err(|e| ShellError::Other(format!("{}: {e}", manifest_path.display())))?;
        if !manifest.provides_shell(shell) {
            continue;
        }
        let tree = dir.join("dist").join(&target);
        let bin = tree.join("bin").join(shell);
        if bin.is_file() {
            return Ok(Some(ShellPackage {
                manifest,
                tree,
                bin_rel: format!("bin/{shell}"),
            }));
        }
        tracing::debug!(
            shell,
            target,
            "shell package found but has no payload for this platform (run its fetch.sh)"
        );
    }
    Ok(None)
}

/// Test-only serialisation of `XXH_SHELLS_DIR`: the variable is process-wide,
/// so every test that sets it — or resolves shells through [`find`] — must hold
/// this guard, including the session tests in `crate::session`.
#[cfg(test)]
pub(crate) mod testenv {
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) struct ShellsDirGuard(#[allow(dead_code)] MutexGuard<'static, ()>);

    /// Point `XXH_SHELLS_DIR` at `dir` for the guard's lifetime.
    pub(crate) fn shells_dir(dir: &Path) -> ShellsDirGuard {
        let g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: serialised by ENV_LOCK; tests only.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("XXH_SHELLS_DIR", dir);
        }
        ShellsDirGuard(g)
    }

    impl Drop for ShellsDirGuard {
        fn drop(&mut self) {
            #[allow(unsafe_code)]
            unsafe {
                std::env::remove_var("XXH_SHELLS_DIR");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;

    fn linux_x86() -> Platform {
        Platform::parse_detect("Linux x86_64 | tar gzip").unwrap()
    }

    fn make_pkg(base: &std::path::Path, shell: &str, with_payload: bool) {
        let dir = base.join(shell);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.toml"),
            format!(
                "name = \"{shell}\"\nversion = \"1.0.0\"\napi_version = \"1.0.0\"\n\
                 [provides]\nshell = \"{shell}\"\n"
            ),
        )
        .unwrap();
        if with_payload {
            let bin = dir.join("dist/linux-x86_64/bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(bin.join(shell), b"#!/bin/sh\n").unwrap();
        }
    }

    #[test]
    fn finds_package_with_platform_payload() {
        let base = std::env::temp_dir().join(format!("xxh-shellpkg-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        make_pkg(&base, "zsh", true);
        let found = {
            let _g = super::testenv::shells_dir(&base);
            find("zsh", &linux_x86()).unwrap()
        };
        let pkg = found.expect("package should be found");
        assert_eq!(pkg.bin_rel, "bin/zsh");
        assert!(pkg.tree.join("bin/zsh").is_file());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn package_without_payload_for_platform_is_skipped() {
        let base = std::env::temp_dir().join(format!("xxh-shellpkg-np-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        make_pkg(&base, "fish", false);
        let found = {
            let _g = super::testenv::shells_dir(&base);
            find("fish", &linux_x86()).unwrap()
        };
        assert!(found.is_none());
        let _ = std::fs::remove_dir_all(&base);
    }
}
