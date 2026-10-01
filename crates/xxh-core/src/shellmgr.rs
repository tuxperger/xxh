//! Managing shell packages on the client: `xxh shell add|fetch|list|update|remove`
//! (008; contracts/shell-package-builds.md, contracts/cli-shell.md).
//!
//! A shell package is an ordinary package (Принцип IV) obtained through the plugin
//! sources (`provider_for`, Принцип IX) and installed into the shells search path
//! as `<shells>/<shell>/`. Its per-platform builds are declared in the manifest
//! (`[builds.<os-arch>] url + sha256`): the archive is downloaded with `curl`,
//! checked **before** it is unpacked (§FR-005), unpacked without letting any entry
//! escape the build directory (C-B3), completed with the package's `overlay/` and
//! its isolated `post_fetch` hook, and only then renamed into `dist/<os-arch>/` —
//! an interrupted or rejected build is never visible (§FR-006, C-B6). All failures
//! are shell-class errors (§FR-010).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use xxh_plugin_api::{BuildSpec, LifecycleStage, Manifest};
use xxh_plugins::source::{SourceSpec, provider_for, read_manifest};

use crate::ShellError;
use crate::shellpkg;

/// What `xxh shell add` records next to a package it installed (data-model.md).
const STATE_FILE: &str = ".xxh-shell.toml";

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    source: Option<SourceSpec>,
    /// Platform → SHA-256 of the archive its build came from.
    #[serde(default)]
    builds: BTreeMap<String, String>,
}

fn err(msg: impl Into<String>) -> ShellError {
    ShellError::Package(msg.into())
}

/// Which builds to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Builds {
    /// Every declared build for an OS xxh supports as a target — Linux today.
    Default,
    /// Every declared build.
    All,
    /// Exactly these platforms.
    Only(Vec<String>),
    /// None.
    Skip,
}

/// An installed shell package, as `xxh shell list` shows it (C-SH3).
#[derive(Debug, Clone)]
pub struct Installed {
    pub shell: String,
    pub dir: PathBuf,
    pub manifest: Manifest,
    /// Installed by `xxh shell add` (has a recorded source); otherwise put into
    /// the search path by hand.
    pub source: Option<SourceSpec>,
    /// The package directory is a symlink placed by hand.
    pub linked: bool,
    /// Platforms with a usable build.
    pub present: Vec<String>,
    /// Declared in the manifest but not fetched.
    pub missing: Vec<String>,
}

/// What `fetch` did per platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    Downloaded(String),
    AlreadyPresent(String),
}

fn read_state(dir: &Path) -> Option<State> {
    let text = std::fs::read_to_string(dir.join(STATE_FILE)).ok()?;
    toml::from_str(&text).ok()
}

fn write_state(dir: &Path, state: &State) -> Result<(), ShellError> {
    let text = toml::to_string_pretty(state).map_err(|e| err(e.to_string()))?;
    std::fs::write(dir.join(STATE_FILE), text)
        .map_err(|e| err(format!("writing {}: {e}", dir.join(STATE_FILE).display())))
}

// Downloading, checking and unpacking live in `xxh_plugins::fetch` (013: plugins
// with builds use them too); these wrappers put failures in the shell class.
use xxh_plugins::fetch::{is_executable, unique};

async fn download(url: &str, dest: &Path) -> Result<(), ShellError> {
    xxh_plugins::fetch::download(url, dest).await.map_err(err)
}

fn sha256_file(path: &Path) -> Result<String, ShellError> {
    xxh_plugins::fetch::sha256_file(path).map_err(err)
}

/// Unpack a build archive, refusing every entry that could escape (C-B3).
pub fn unpack(archive: &Path, dest: &Path, strip: u32) -> Result<(), ShellError> {
    xxh_plugins::fetch::unpack(archive, dest, strip).map_err(err)
}

fn copy_tree(src: &Path, dst: &Path, skip: &[&str]) -> Result<(), ShellError> {
    xxh_plugins::fetch::copy_tree(src, dst, skip).map_err(err)
}
/// Remove what interrupted installs left in `dist/` (research R4).
fn sweep(dist: &Path) {
    if let Ok(entries) = std::fs::read_dir(dist) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with(".tmp-") {
                let p = e.path();
                let _ = std::fs::remove_dir_all(&p).or_else(|_| std::fs::remove_file(&p));
            }
        }
    }
}

/// Install one platform's build into `pkg/dist/<platform>` (C-B2..C-B6) and
/// return the archive's SHA-256. Nothing is visible unless every step passed.
pub async fn install_build(
    pkg: &Path,
    manifest: &Manifest,
    shell: &str,
    platform: &str,
    spec: &BuildSpec,
) -> Result<String, ShellError> {
    spec.check(platform).map_err(|e| err(e.to_string()))?;
    let dist = pkg.join("dist");
    std::fs::create_dir_all(&dist).map_err(|e| err(format!("{}: {e}", dist.display())))?;
    sweep(&dist);
    let tmp = dist.join(unique(&format!(".tmp-{platform}")));
    let archive = dist.join(unique(&format!(".tmp-{platform}-archive")));

    let result = async {
        download(&spec.url, &archive).await?;
        let got = sha256_file(&archive)?;
        if got != spec.sha256 {
            return Err(err(format!(
                "the {platform} build of {shell} failed its integrity check \
                 (sha256 {got}, the manifest says {}); it was not installed",
                spec.sha256
            )));
        }
        unpack(&archive, &tmp, spec.strip)?;
        let overlay = pkg.join("overlay");
        if overlay.is_dir() {
            copy_tree(&overlay, &tmp, &[])?;
        }
        if let Some(hook) = manifest.hooks.get(&LifecycleStage::PostFetch) {
            let env = BTreeMap::from([
                (
                    "XXH_BUILD_DIR".to_string(),
                    tmp.to_string_lossy().into_owned(),
                ),
                ("XXH_BUILD_PLATFORM".to_string(), platform.to_string()),
            ]);
            xxh_plugins::isolation::run_hook(&manifest.name, pkg, hook, &env)
                .await
                .map_err(|e| err(format!("post_fetch of {shell} for {platform} failed: {e}")))?;
        }
        if !is_executable(&tmp.join("bin").join(shell)) {
            return Err(err(format!(
                "the {platform} build of {shell} has no executable bin/{shell}"
            )));
        }
        let fin = dist.join(platform);
        if fin.exists() {
            std::fs::remove_dir_all(&fin).map_err(|e| err(format!("{}: {e}", fin.display())))?;
        }
        std::fs::rename(&tmp, &fin).map_err(|e| err(format!("{}: {e}", fin.display())))?;
        Ok(got)
    }
    .await;
    let _ = std::fs::remove_file(&archive);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    result
}

fn shell_of(manifest: &Manifest) -> Result<String, ShellError> {
    manifest.provides.get("shell").cloned().ok_or_else(|| {
        err(format!(
            "{} is not a shell package: its manifest has no provides.shell",
            manifest.name
        ))
    })
}

fn platforms(manifest: &Manifest, which: &Builds) -> Result<Vec<String>, ShellError> {
    Ok(match which {
        Builds::Skip => Vec::new(),
        Builds::All => manifest.builds.keys().cloned().collect(),
        Builds::Default => manifest
            .builds
            .keys()
            .filter(|k| k.starts_with("linux-"))
            .cloned()
            .collect(),
        Builds::Only(list) => {
            for p in list {
                if !manifest.builds.contains_key(p) {
                    let declared: Vec<&str> = manifest.builds.keys().map(String::as_str).collect();
                    return Err(err(format!(
                        "{} has no build for {p}; declared: {}",
                        manifest.name,
                        if declared.is_empty() {
                            "none".to_string()
                        } else {
                            declared.join(", ")
                        }
                    )));
                }
            }
            list.clone()
        }
    })
}

/// Fetch `which` builds of the package in `dir`. A build already present from
/// the same archive is not downloaded again (US3 scenario 2).
async fn fetch_into(
    dir: &Path,
    manifest: &Manifest,
    shell: &str,
    which: &Builds,
    state: Option<&mut State>,
) -> Result<Vec<Fetched>, ShellError> {
    let wanted = platforms(manifest, which)?;
    let mut done = Vec::new();
    let mut recorded = BTreeMap::new();
    for p in wanted {
        let spec = &manifest.builds[&p];
        let present = is_executable(&dir.join("dist").join(&p).join("bin").join(shell));
        let same = state
            .as_ref()
            .is_none_or(|s| s.builds.get(&p) == Some(&spec.sha256));
        if present && same {
            done.push(Fetched::AlreadyPresent(p));
            continue;
        }
        let sha = install_build(dir, manifest, shell, &p, spec).await?;
        recorded.insert(p.clone(), sha);
        done.push(Fetched::Downloaded(p));
    }
    if let Some(state) = state {
        state.builds.extend(recorded);
    }
    Ok(done)
}

/// The package providing `shell` in the search path (first wins, as at login).
fn find(shell: &str) -> Result<(PathBuf, Manifest), ShellError> {
    for base in shellpkg::search_dirs() {
        let dir = base.join(shell);
        if let Ok(m) = read_manifest(&dir) {
            if m.provides_shell(shell) {
                return Ok((dir, m));
            }
        }
    }
    Err(err(format!(
        "no {shell} shell package is installed; add one with `xxh shell add <source>`"
    )))
}

/// `xxh shell add` (C-SH1).
pub async fn add(spec: &SourceSpec, which: &Builds) -> Result<Installed, ShellError> {
    let provider = provider_for(spec).map_err(|e| err(e.to_string()))?;
    let fetched = provider.fetch(spec).await.map_err(|e| err(e.to_string()))?;
    let result = install_package(spec, &fetched.dir, &fetched.manifest, which).await;
    if let Some(tmp) = &fetched.cleanup {
        let _ = std::fs::remove_dir_all(tmp);
    }
    result
}

async fn install_package(
    spec: &SourceSpec,
    src: &Path,
    manifest: &Manifest,
    which: &Builds,
) -> Result<Installed, ShellError> {
    let shell = shell_of(manifest)?;
    for (p, b) in &manifest.builds {
        b.check(p).map_err(|e| err(e.to_string()))?;
    }
    let wanted = platforms(manifest, which)?;
    let root = shellpkg::install_dir()?;
    let dest = root.join(&shell);

    // An existing package of another origin is never replaced (§FR-009);
    // decided before anything is written.
    let mut state = State {
        source: Some(spec.clone()),
        builds: BTreeMap::new(),
    };
    let existing = std::fs::symlink_metadata(&dest).is_ok();
    if existing {
        match read_state(&dest) {
            Some(old) if old.source.as_ref().is_some_and(|s| s.same_origin(spec)) => {
                state.builds = old.builds;
            }
            Some(old) => {
                return Err(err(format!(
                    "{shell} is already installed from {}; remove it first with \
                     `xxh shell remove {shell}`",
                    old.source.map_or("elsewhere".into(), |s| s.describe())
                )));
            }
            None => {
                return Err(err(format!(
                    "{shell} was put into {} by hand; remove it first with \
                     `xxh shell remove {shell}`",
                    root.display()
                )));
            }
        }
    }

    let tmp = root.join(unique(&format!(".tmp-{shell}")));
    let staged = async {
        copy_tree(src, &tmp, &["dist", ".git", STATE_FILE])?;
        if existing {
            // Keep the builds; drop those the new manifest no longer declares.
            let old_dist = dest.join("dist");
            if old_dist.is_dir() {
                std::fs::rename(&old_dist, tmp.join("dist"))
                    .map_err(|e| err(format!("{}: {e}", old_dist.display())))?;
            }
            if let Ok(entries) = std::fs::read_dir(tmp.join("dist")) {
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if !manifest.builds.contains_key(&name) {
                        let _ = std::fs::remove_dir_all(e.path());
                        state.builds.remove(&name);
                    }
                }
            }
            std::fs::remove_dir_all(&dest).map_err(|e| err(format!("{}: {e}", dest.display())))?;
        }
        std::fs::rename(&tmp, &dest).map_err(|e| err(format!("{}: {e}", dest.display())))
    }
    .await;
    if let Err(e) = staged {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }

    let result = fetch_into(
        &dest,
        manifest,
        &shell,
        &Builds::Only(wanted),
        Some(&mut state),
    )
    .await;
    write_state(&dest, &state)?;
    result?;
    describe(&dest, &shell)
}

/// `xxh shell fetch` (C-SH2).
pub async fn fetch(shell: &str, which: &Builds) -> Result<Vec<Fetched>, ShellError> {
    let (dir, manifest) = find(shell)?;
    match read_state(&dir) {
        Some(mut state) => {
            let r = fetch_into(&dir, &manifest, shell, which, Some(&mut state)).await;
            write_state(&dir, &state)?;
            r
        }
        // A package placed by hand is the user's directory: builds go into its
        // dist/, but no state file is written there.
        None => fetch_into(&dir, &manifest, shell, which, None).await,
    }
}

/// `xxh shell update` (C-SH4): re-fetch managed packages from their sources and
/// rebuild the builds whose archive changed. Returns the shells skipped as
/// placed by hand.
pub async fn update(shell: Option<&str>) -> Result<(Vec<Installed>, Vec<String>), ShellError> {
    let mut updated = Vec::new();
    let mut skipped = Vec::new();
    for p in list()? {
        if shell.is_some_and(|s| s != p.shell) {
            continue;
        }
        match &p.source {
            Some(src) => {
                // Platforms dropped upstream are pruned by install_package.
                let fresh = add_keeping(src, &p.present).await?;
                updated.push(fresh);
            }
            None => skipped.push(p.shell.clone()),
        }
    }
    if let Some(s) = shell {
        if updated.is_empty() && skipped.is_empty() {
            return Err(err(format!("no {s} shell package is installed")));
        }
    }
    Ok((updated, skipped))
}

/// `add` for an update: the platforms present before stay wanted, even when the
/// new manifest dropped some (those are pruned instead of failing).
async fn add_keeping(spec: &SourceSpec, present: &[String]) -> Result<Installed, ShellError> {
    let provider = provider_for(spec).map_err(|e| err(e.to_string()))?;
    let fetched = provider.fetch(spec).await.map_err(|e| err(e.to_string()))?;
    let still: Vec<String> = present
        .iter()
        .filter(|p| fetched.manifest.builds.contains_key(*p))
        .cloned()
        .collect();
    let result = install_package(spec, &fetched.dir, &fetched.manifest, &Builds::Only(still)).await;
    if let Some(tmp) = &fetched.cleanup {
        let _ = std::fs::remove_dir_all(tmp);
    }
    result
}

/// `xxh shell remove` (C-SH5). A package linked in by hand loses only the link.
/// Returns whether it was a link.
pub fn remove(shell: &str) -> Result<bool, ShellError> {
    let (dir, _) = find(shell)?;
    let meta =
        std::fs::symlink_metadata(&dir).map_err(|e| err(format!("{}: {e}", dir.display())))?;
    if meta.file_type().is_symlink() {
        std::fs::remove_file(&dir).map_err(|e| err(format!("{}: {e}", dir.display())))?;
        Ok(true)
    } else {
        std::fs::remove_dir_all(&dir).map_err(|e| err(format!("{}: {e}", dir.display())))?;
        Ok(false)
    }
}

fn describe(dir: &Path, shell: &str) -> Result<Installed, ShellError> {
    let manifest = read_manifest(dir).map_err(|e| err(e.to_string()))?;
    let present = shellpkg::builds_in(dir, shell);
    let missing = manifest
        .builds
        .keys()
        .filter(|k| !present.contains(k))
        .cloned()
        .collect();
    Ok(Installed {
        shell: shell.to_string(),
        dir: dir.to_path_buf(),
        source: read_state(dir).and_then(|s| s.source),
        linked: std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_symlink()),
        manifest,
        present,
        missing,
    })
}

/// `xxh shell list` (C-SH3): every shell package in the search path; the first
/// provider of a shell wins, as at login.
pub fn list() -> Result<Vec<Installed>, ShellError> {
    let mut out: Vec<Installed> = Vec::new();
    for base in shellpkg::search_dirs() {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.'))
            .collect();
        names.sort();
        for name in names {
            let dir = base.join(&name);
            let Ok(m) = read_manifest(&dir) else {
                continue;
            };
            if !m.provides_shell(&name) || out.iter().any(|i| i.shell == name) {
                continue;
            }
            out.push(describe(&dir, &name)?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shellpkg::testenv;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(unique(&format!("xxh-shellmgr-{name}")));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A gzipped tar with `entries` (path, contents, mode); `None` contents = dir.
    fn archive(path: &Path, entries: &[(&str, Option<&[u8]>, u32)]) -> String {
        let f = std::fs::File::create(path).unwrap();
        let gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        let mut b = tar::Builder::new(gz);
        for (name, data, mode) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_mode(*mode);
            match data {
                Some(d) => {
                    h.set_size(d.len() as u64);
                    h.set_entry_type(tar::EntryType::Regular);
                    b.append_data(&mut h, name, *d).unwrap();
                }
                None => {
                    h.set_size(0);
                    h.set_entry_type(tar::EntryType::Directory);
                    b.append_data(&mut h, name, std::io::empty()).unwrap();
                }
            }
        }
        b.into_inner().unwrap().finish().unwrap();
        sha256_file(path).unwrap()
    }

    const XSH: &[u8] = b"#!/bin/sh\nexec /bin/sh \"$@\"\n";

    /// A shell package `xsh` in `dir` whose linux-x86_64 build is `archive`.
    fn package(dir: &Path, archive_path: &Path, sha: &str, extra: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            format!(
                "name = \"xsh\"\nversion = \"1.0.0\"\napi_version = \"1.1.0\"\n\
                 [provides]\nshell = \"xsh\"\n\
                 [builds.linux-x86_64]\nurl = \"file://{}\"\nsha256 = \"{sha}\"\n{extra}",
                archive_path.display()
            ),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn verified_build_is_installed_with_overlay() {
        let d = scratch("ok");
        let a = d.join("xsh.tgz");
        let sha = archive(
            &a,
            &[
                ("pkg/", None, 0o755),
                ("pkg/bin/", None, 0o755),
                ("pkg/bin/xsh", Some(XSH), 0o755),
            ],
        );
        let pkg = d.join("pkg");
        package(&pkg, &a, &sha, "strip = 1\n");
        std::fs::create_dir_all(pkg.join("overlay")).unwrap();
        std::fs::write(pkg.join("overlay/env.sh"), "export XSH=1\n").unwrap();
        let m = read_manifest(&pkg).unwrap();
        let got = install_build(&pkg, &m, "xsh", "linux-x86_64", &m.builds["linux-x86_64"])
            .await
            .unwrap();
        assert_eq!(got, sha);
        let build = pkg.join("dist/linux-x86_64");
        assert!(is_executable(&build.join("bin/xsh")));
        assert!(build.join("env.sh").is_file(), "overlay applied");
        let names: Vec<_> = std::fs::read_dir(pkg.join("dist"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["linux-x86_64"], "no temporary leftovers");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// §FR-005/§FR-006: a wrong checksum or a failing hook leaves nothing visible.
    #[tokio::test]
    async fn rejected_build_leaves_nothing() {
        let d = scratch("bad");
        let a = d.join("xsh.tgz");
        archive(&a, &[("bin/xsh", Some(XSH), 0o755)]);
        let pkg = d.join("pkg");
        package(&pkg, &a, &"0".repeat(64), "");
        let m = read_manifest(&pkg).unwrap();
        let e = install_build(&pkg, &m, "xsh", "linux-x86_64", &m.builds["linux-x86_64"])
            .await
            .unwrap_err();
        assert!(e.to_string().contains("integrity"), "{e}");
        let left: Vec<_> = std::fs::read_dir(pkg.join("dist")).unwrap().collect();
        assert!(left.is_empty(), "{left:?}");

        // Correct sum, failing post_fetch hook.
        let sha = sha256_file(&a).unwrap();
        let pkg2 = d.join("pkg2");
        package(
            &pkg2,
            &a,
            &sha,
            "[hooks.post_fetch]\nrun = \"hooks/pf.sh\"\n",
        );
        std::fs::create_dir_all(pkg2.join("hooks")).unwrap();
        std::fs::write(
            pkg2.join("hooks/pf.sh"),
            "test -d \"$XXH_BUILD_DIR\" && exit 3\n",
        )
        .unwrap();
        let m = read_manifest(&pkg2).unwrap();
        let e = install_build(&pkg2, &m, "xsh", "linux-x86_64", &m.builds["linux-x86_64"])
            .await
            .unwrap_err();
        assert!(e.to_string().contains("post_fetch"), "{e}");
        assert!(!pkg2.join("dist/linux-x86_64").exists());

        // No bin/<shell> in the archive.
        let empty = d.join("empty.tgz");
        let sha = archive(&empty, &[("README", Some(b"x"), 0o644)]);
        let pkg3 = d.join("pkg3");
        package(&pkg3, &empty, &sha, "");
        let m = read_manifest(&pkg3).unwrap();
        assert!(
            install_build(&pkg3, &m, "xsh", "linux-x86_64", &m.builds["linux-x86_64"])
                .await
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// add → list → fetch (no re-download) → conflicts → remove (US1–US3).
    #[tokio::test]
    async fn package_lifecycle() {
        let d = scratch("life");
        let shells = d.join("shells");
        std::fs::create_dir_all(&shells).unwrap();
        let _g = testenv::shells_dir(&shells);
        let a = d.join("xsh.tgz");
        let sha = archive(&a, &[("bin/xsh", Some(XSH), 0o755)]);
        let src = d.join("src");
        package(&src, &a, &sha, "");
        std::fs::create_dir_all(src.join("dist/junk")).unwrap();
        let spec = SourceSpec::Local { path: src.clone() };

        let inst = add(&spec, &Builds::Default).await.unwrap();
        assert_eq!(inst.shell, "xsh");
        assert_eq!(inst.present, ["linux-x86_64"]);
        assert!(inst.source.is_some() && !inst.linked);
        assert!(
            !shells.join("xsh/dist/junk").exists(),
            "source dist/ not copied"
        );

        // Visible to the login lookup.
        let x86 = crate::platform::Platform::parse_detect("Linux x86_64 | tar gzip").unwrap();
        assert!(matches!(
            shellpkg::lookup("xsh", &x86).unwrap(),
            shellpkg::ShellLookup::Found(_)
        ));

        // Fetching again downloads nothing — even with the archive gone.
        std::fs::rename(&a, d.join("moved")).unwrap();
        assert_eq!(
            fetch("xsh", &Builds::Default).await.unwrap(),
            [Fetched::AlreadyPresent("linux-x86_64".into())]
        );
        let unknown = fetch("xsh", &Builds::Only(vec!["plan9-mips".into()]))
            .await
            .unwrap_err();
        assert!(
            unknown.to_string().contains("declared: linux-x86_64"),
            "{unknown}"
        );

        // Another origin providing the same shell is refused (§FR-009).
        std::fs::rename(d.join("moved"), &a).unwrap();
        let other = d.join("other");
        package(&other, &a, &sha, "");
        let conflict = add(&SourceSpec::Local { path: other }, &Builds::Skip)
            .await
            .unwrap_err();
        assert!(
            conflict.to_string().contains("xxh shell remove xsh"),
            "{conflict}"
        );

        assert_eq!(list().unwrap().len(), 1);
        assert!(!remove("xsh").unwrap());
        assert!(list().unwrap().is_empty());
        drop(_g);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A package linked in by hand: listed as linked, fetchable, `update` skips
    /// it, `remove` drops only the link.
    #[tokio::test]
    async fn linked_package_is_never_deleted() {
        let d = scratch("link");
        let shells = d.join("shells");
        std::fs::create_dir_all(&shells).unwrap();
        let _g = testenv::shells_dir(&shells);
        let a = d.join("xsh.tgz");
        let sha = archive(&a, &[("bin/xsh", Some(XSH), 0o755)]);
        let repo = d.join("repo");
        package(&repo, &a, &sha, "");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&repo, shells.join("xsh")).unwrap();

        let l = list().unwrap();
        assert!(l[0].linked && l[0].source.is_none());
        assert_eq!(l[0].missing, ["linux-x86_64"]);
        fetch("xsh", &Builds::Default).await.unwrap();
        assert!(repo.join("dist/linux-x86_64/bin/xsh").is_file());
        assert!(
            !repo.join(STATE_FILE).exists(),
            "the user's directory gets no state"
        );
        let (updated, skipped) = update(None).await.unwrap();
        assert!(updated.is_empty());
        assert_eq!(skipped, ["xsh"]);
        let conflict = add(&SourceSpec::Local { path: repo.clone() }, &Builds::Skip)
            .await
            .unwrap_err();
        assert!(conflict.to_string().contains("by hand"), "{conflict}");

        assert!(remove("xsh").unwrap());
        assert!(
            repo.join("plugin.toml").is_file(),
            "the link's target survives"
        );
        drop(_g);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `update` refetches a build whose archive changed and drops one the
    /// manifest no longer declares (C-SH4).
    #[tokio::test]
    async fn update_follows_the_manifest() {
        let d = scratch("upd");
        let shells = d.join("shells");
        std::fs::create_dir_all(&shells).unwrap();
        let _g = testenv::shells_dir(&shells);
        let a = d.join("v1.tgz");
        let sha1 = archive(&a, &[("bin/xsh", Some(XSH), 0o755)]);
        let src = d.join("src");
        let arm = format!(
            "[builds.linux-aarch64]\nurl = \"file://{}\"\nsha256 = \"{sha1}\"\n",
            a.display()
        );
        package(&src, &a, &sha1, &arm);
        add(&SourceSpec::Local { path: src.clone() }, &Builds::Default)
            .await
            .unwrap();
        assert_eq!(
            list().unwrap()[0].present,
            ["linux-aarch64", "linux-x86_64"]
        );

        // v2: new x86_64 archive, aarch64 dropped.
        let b = d.join("v2.tgz");
        let sha2 = archive(&b, &[("bin/xsh", Some(b"#!/bin/sh\necho v2\n"), 0o755)]);
        package(&src, &b, &sha2, "");
        let (updated, _) = update(Some("xsh")).await.unwrap();
        assert_eq!(updated[0].present, ["linux-x86_64"]);
        let body = std::fs::read(shells.join("xsh/dist/linux-x86_64/bin/xsh")).unwrap();
        assert!(String::from_utf8_lossy(&body).contains("v2"));
        drop(_g);
        let _ = std::fs::remove_dir_all(&d);
    }
}
