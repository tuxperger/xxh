//! Local plugin registry — content-addressed storage + index (T035).
//!
//! Layout under `~/.local/share/xxh/plugins` (override: `$XXH_PLUGINS_DIR`, used
//! by tests):
//!
//! ```text
//! index.toml            # name -> { source spec, content hash, version }
//! packages/<blake3>/    # immutable package trees (plugin.toml at the root)
//! ```
//!
//! The content hash addresses the package on the client and (via delivery
//! components) on the host, so unchanged plugins are never re-transferred
//! (§FR-013, Принцип VI).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use xxh_plugin_api::{Manifest, PluginError};

use crate::source::{FetchedPackage, SourceSpec, provider_for};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub source: SourceSpec,
    pub hash: String,
    pub version: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    plugins: BTreeMap<String, IndexEntry>,
}

pub struct Registry {
    root: PathBuf,
}

impl Registry {
    /// Open the default registry location (respecting `$XXH_PLUGINS_DIR`).
    pub fn open_default() -> Result<Self, PluginError> {
        let root = match std::env::var_os("XXH_PLUGINS_DIR") {
            Some(v) => PathBuf::from(v),
            None => directories::BaseDirs::new()
                .ok_or_else(|| PluginError::Other("cannot determine data directory".into()))?
                .data_dir()
                .join("xxh")
                .join("plugins"),
        };
        Ok(Self { root })
    }

    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.toml")
    }

    fn load_index(&self) -> Result<Index, PluginError> {
        match std::fs::read_to_string(self.index_path()) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| PluginError::Other(format!("corrupt registry index: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Index::default()),
            Err(e) => Err(PluginError::Other(format!("reading registry index: {e}"))),
        }
    }

    fn save_index(&self, index: &Index) -> Result<(), PluginError> {
        std::fs::create_dir_all(&self.root)
            .map_err(|e| PluginError::Other(format!("creating registry: {e}")))?;
        let text = toml::to_string_pretty(index).expect("index always serializes");
        std::fs::write(self.index_path(), text)
            .map_err(|e| PluginError::Other(format!("writing registry index: {e}")))
    }

    /// Install (or refresh) a plugin from `spec`. Returns its manifest.
    pub async fn install(&self, spec: &SourceSpec) -> Result<Manifest, PluginError> {
        Ok(self.install_pinned(spec, None, None).await?.manifest)
    }

    /// Install `spec`, a git source at commit `pin` when given, and refuse the
    /// result unless its content hash is `expect_hash` (013 C-L5, §FR-004). The
    /// index keeps `spec` itself, so `plugin update` still moves forward; the
    /// lock file is what holds the pin.
    pub async fn install_pinned(
        &self,
        spec: &SourceSpec,
        pin: Option<&str>,
        expect_hash: Option<&str>,
    ) -> Result<Installed, PluginError> {
        let fetch_spec = match (spec, pin) {
            (SourceSpec::Git { url, .. }, Some(rev)) => SourceSpec::Git {
                url: url.clone(),
                reference: Some(rev.to_string()),
            },
            _ => spec.clone(),
        };
        let provider = provider_for(&fetch_spec)?;
        let fetched = provider.fetch(&fetch_spec).await?;
        let result = self.prepare_and_store(spec, &fetched, expect_hash).await;
        if let Some(tmp) = &fetched.cleanup {
            let _ = std::fs::remove_dir_all(tmp);
        }
        result.map(|hash| Installed {
            manifest: fetched.manifest.clone(),
            hash,
            revision: fetched.revision.clone(),
        })
    }

    /// Copy the fetched package aside, add its declared build for this client
    /// (C-L11), hash it, check the expected hash, then store it.
    async fn prepare_and_store(
        &self,
        spec: &SourceSpec,
        fetched: &FetchedPackage,
        expect_hash: Option<&str>,
    ) -> Result<String, PluginError> {
        let packages = self.root.join("packages");
        let work = packages.join(crate::fetch::unique(".work"));
        let io = |e: String| PluginError::Other(format!("preparing package: {e}"));
        let result = async {
            crate::fetch::copy_tree(&fetched.dir, &work, &[]).map_err(io)?;
            add_client_build(&fetched.manifest, &work).await?;
            let hash = hash_dir(&work)?;
            if let Some(want) = expect_hash.filter(|w| *w != hash) {
                return Err(PluginError::Other(format!(
                    "`{}` does not match the lock file (expected content {want}, got {hash}); \
                     it was not installed",
                    fetched.manifest.name
                )));
            }
            self.store(spec, fetched, &work, &hash)?;
            Ok(hash)
        }
        .await;
        let _ = std::fs::remove_dir_all(&work);
        result
    }

    fn store(
        &self,
        spec: &SourceSpec,
        fetched: &FetchedPackage,
        work: &Path,
        hash: &str,
    ) -> Result<(), PluginError> {
        // A provider that resolved a pin during the fetch reports the spec to
        // record; everything else is recorded as requested (C-F18).
        let spec = fetched.resolved.as_ref().unwrap_or(spec);
        let name = &fetched.manifest.name;
        let mut index = self.load_index()?;

        // A flake plugin never silently replaces — or is replaced by — a plugin of
        // another origin that happens to share its name (003 §FR-009, C-F20).
        // Checked before anything is written.
        if let Some(existing) = index.plugins.get(name) {
            let flake_involved = existing.source.is_flake() || spec.is_flake();
            if flake_involved && !existing.source.same_origin(spec) {
                // Only a flake program can be renamed at install time.
                let rename = if spec.is_flake() {
                    "choose another name with `xxh plugin add --name <name> …` or "
                } else {
                    ""
                };
                return Err(PluginError::Other(format!(
                    "NameConflict: a plugin named `{name}` is already installed from {}; \
                     {rename}remove it first with `xxh plugin remove {name}`",
                    existing.source.describe()
                )));
            }
        }

        let hash = hash.to_string();
        let dest = self.root.join("packages").join(&hash);
        if !dest.is_dir() {
            std::fs::rename(work, &dest)
                .map_err(|e| PluginError::Other(format!("storing package: {e}")))?;
        }

        let old = index.plugins.insert(
            fetched.manifest.name.clone(),
            IndexEntry {
                source: spec.clone(),
                hash: hash.clone(),
                version: fetched.manifest.version.to_string(),
            },
        );
        self.save_index(&index)?;
        // Garbage-collect the previous content if nothing references it any more.
        if let Some(prev) = old {
            if prev.hash != hash && !index.plugins.values().any(|e| e.hash == prev.hash) {
                let _ = std::fs::remove_dir_all(self.root.join("packages").join(&prev.hash));
            }
        }
        Ok(())
    }

    /// Re-fetch a plugin from its recorded source (T035 update). A recorded pin is
    /// dropped first, so the source is resolved afresh — the only operation that
    /// moves a pinned plugin to a new revision (003 §FR-016, C-F19).
    pub async fn update(&self, name: &str) -> Result<Manifest, PluginError> {
        let entry = self.entry(name)?;
        self.install(&entry.source.unpinned()).await
    }

    /// Remove a plugin and its content (if unshared).
    pub fn remove(&self, name: &str) -> Result<(), PluginError> {
        let mut index = self.load_index()?;
        let Some(entry) = index.plugins.remove(name) else {
            return Err(PluginError::Other(format!(
                "plugin `{name}` is not installed"
            )));
        };
        self.save_index(&index)?;
        if !index.plugins.values().any(|e| e.hash == entry.hash) {
            let _ = std::fs::remove_dir_all(self.root.join("packages").join(&entry.hash));
        }
        Ok(())
    }

    /// All installed plugins (name → entry), deterministic order.
    pub fn list(&self) -> Result<BTreeMap<String, IndexEntry>, PluginError> {
        Ok(self.load_index()?.plugins)
    }

    /// The index record of an installed plugin (source, content hash, version).
    pub fn entry(&self, name: &str) -> Result<IndexEntry, PluginError> {
        self.load_index()?
            .plugins
            .get(name)
            .cloned()
            .ok_or_else(|| PluginError::Other(format!("plugin `{name}` is not installed")))
    }

    /// Immutable package directory of an installed plugin.
    pub fn package_dir(&self, name: &str) -> Result<PathBuf, PluginError> {
        Ok(self.root.join("packages").join(self.entry(name)?.hash))
    }

    /// Parsed, api-checked manifest of an installed plugin.
    pub fn manifest(&self, name: &str) -> Result<Manifest, PluginError> {
        crate::source::read_manifest(&self.package_dir(name)?)
    }
}

/// What an install produced (013): the manifest, the content hash the registry
/// addresses it by, and the git commit it came from.
#[derive(Debug, Clone)]
pub struct Installed {
    pub manifest: Manifest,
    pub hash: String,
    pub revision: Option<String>,
}

/// This client's platform in xxh's `os-arch` naming (`linux-x86_64`, …).
pub fn client_platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "arm" => "armv7l",
        other => other,
    };
    format!("{os}-{arch}")
}

/// A plugin whose manifest declares `[builds]` gets the build for this client
/// unpacked into `dist/` — checked against its SHA-256 first (013 C-L11; the
/// archive rules of 008 C-B2..C-B3). Without `[builds]`, nothing to do.
async fn add_client_build(manifest: &Manifest, dir: &Path) -> Result<(), PluginError> {
    if manifest.builds.is_empty() {
        return Ok(());
    }
    let platform = client_platform();
    let Some(build) = manifest.builds.get(&platform) else {
        let have: Vec<&str> = manifest.builds.keys().map(String::as_str).collect();
        return Err(PluginError::Other(format!(
            "`{}` has no build for {platform} (has {})",
            manifest.name,
            have.join(", ")
        )));
    };
    build.check(&platform)?;
    let fail = |e: String| PluginError::Other(format!("build of `{}`: {e}", manifest.name));
    let archive = dir.with_extension(crate::fetch::unique("archive"));
    let result = async {
        crate::fetch::download(&build.url, &archive)
            .await
            .map_err(fail)?;
        let got = crate::fetch::sha256_file(&archive).map_err(fail)?;
        if got != build.sha256 {
            return Err(fail(format!(
                "integrity check failed (sha256 {got}, the manifest says {})",
                build.sha256
            )));
        }
        let dist = dir.join("dist");
        let _ = std::fs::remove_dir_all(&dist);
        crate::fetch::unpack(&archive, &dist, build.strip).map_err(fail)
    }
    .await;
    let _ = std::fs::remove_file(&archive);
    result
}

/// Deterministic blake3 of a directory tree: sorted relative paths + contents.
fn hash_dir(dir: &Path) -> Result<String, PluginError> {
    let mut files = Vec::new();
    walk(dir, dir, &mut files).map_err(|e| PluginError::Other(format!("hashing package: {e}")))?;
    files.sort();
    let mut hasher = blake3::Hasher::new();
    for rel in files {
        hasher.update(rel.as_bytes());
        hasher.update(&[0]);
        let data = std::fs::read(dir.join(&rel))
            .map_err(|e| PluginError::Other(format!("hashing package: {e}")))?;
        hasher.update(&data);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk(base, &path, out)?;
        } else {
            out.push(
                path.strip_prefix(base)
                    .expect("child of base")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin_dir(name: &str, version: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xxh-reg-src-{name}-{}-{version}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            format!("name = \"{name}\"\nversion = \"{version}\"\napi_version = \"1.0.0\"\n"),
        )
        .unwrap();
        std::fs::write(dir.join("env.sh"), b"export FROM_PLUGIN=1\n").unwrap();
        dir
    }

    fn tmp_registry() -> (Registry, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "xxh-reg-{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        (Registry::open(&root), root)
    }

    #[tokio::test]
    async fn install_list_remove_roundtrip() {
        let (reg, root) = tmp_registry();
        let src = plugin_dir("demo", "1.0.0");

        let spec = SourceSpec::Local { path: src.clone() };
        let m = reg.install(&spec).await.unwrap();
        assert_eq!(m.name, "demo");

        let listed = reg.list().unwrap();
        assert_eq!(listed["demo"].version, "1.0.0");
        assert!(reg.package_dir("demo").unwrap().join("env.sh").is_file());
        assert_eq!(reg.manifest("demo").unwrap().name, "demo");

        reg.remove("demo").unwrap();
        assert!(reg.list().unwrap().is_empty());
        assert!(!root.join("packages").join(&listed["demo"].hash).exists());

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn install_is_content_addressed_and_idempotent() {
        let (reg, root) = tmp_registry();
        let src = plugin_dir("twice", "0.2.0");
        let spec = SourceSpec::Local { path: src.clone() };

        reg.install(&spec).await.unwrap();
        let h1 = reg.list().unwrap()["twice"].hash.clone();
        reg.install(&spec).await.unwrap();
        let h2 = reg.list().unwrap()["twice"].hash.clone();
        assert_eq!(h1, h2, "same content ⇒ same address");

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn flake_spec(reference: &str) -> SourceSpec {
        SourceSpec::Flake {
            reference: reference.into(),
            attr: "default".into(),
            locked_url: None,
            revision: None,
            name: None,
        }
    }

    /// Store a hand-built fetch the way an install does, minus the provider.
    fn store_now(reg: &Registry, spec: &SourceSpec, f: &FetchedPackage) -> Result<(), PluginError> {
        let work = reg
            .root
            .join("packages")
            .join(crate::fetch::unique(".work"));
        crate::fetch::copy_tree(&f.dir, &work, &[]).unwrap();
        let hash = hash_dir(&work).unwrap();
        let r = reg.store(spec, f, &work, &hash);
        let _ = std::fs::remove_dir_all(&work);
        r
    }

    /// What a provider hands back, built by hand so the registry rules can be
    /// tested without Nix.
    fn fetched(dir: &Path, resolved: Option<SourceSpec>) -> FetchedPackage {
        FetchedPackage {
            manifest: crate::source::read_manifest(dir).unwrap(),
            dir: dir.to_path_buf(),
            env: BTreeMap::new(),
            cleanup: None,
            resolved,
            revision: None,
        }
    }

    #[test]
    fn resolved_spec_is_what_gets_recorded() {
        let (reg, root) = tmp_registry();
        let src = plugin_dir("pinned", "1.0.0");
        let pinned = SourceSpec::Flake {
            reference: "github:o/r".into(),
            attr: "default".into(),
            locked_url: Some("github:o/r/abc".into()),
            revision: Some("abc".into()),
            name: None,
        };
        store_now(
            &reg,
            &flake_spec("github:o/r"),
            &fetched(&src, Some(pinned.clone())),
        )
        .unwrap();
        assert_eq!(reg.entry("pinned").unwrap().source, pinned);
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn flake_never_silently_replaces_another_origin() {
        let (reg, root) = tmp_registry();
        let src = plugin_dir("clash", "1.0.0");
        let local = SourceSpec::Local { path: src.clone() };
        reg.install(&local).await.unwrap();
        let before = std::fs::read_to_string(root.join("index.toml")).unwrap();

        // flake over local: rejected, registry untouched.
        let err = store_now(&reg, &flake_spec("github:o/r"), &fetched(&src, None))
            .unwrap_err()
            .to_string();
        assert!(err.contains("NameConflict"), "got: {err}");
        assert!(
            err.contains("--name"),
            "the message must name a way out: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("index.toml")).unwrap(),
            before
        );

        // local over local keeps the pre-existing behaviour: refreshed in place.
        reg.install(&local).await.unwrap();

        // The same flake origin again updates in place, pin or no pin…
        reg.remove("clash").unwrap();
        store_now(&reg, &flake_spec("github:o/r"), &fetched(&src, None)).unwrap();
        store_now(&reg, &flake_spec("github:o/r"), &fetched(&src, None)).unwrap();
        // …while another flake of the same name, or a local plugin, is a conflict.
        assert!(store_now(&reg, &flake_spec("github:x/y"), &fetched(&src, None)).is_err());
        assert!(reg.install(&local).await.is_err());

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn failed_install_leaves_the_registry_untouched() {
        let (reg, root) = tmp_registry();
        let src = plugin_dir("stable", "1.0.0");
        reg.install(&SourceSpec::Local { path: src.clone() })
            .await
            .unwrap();
        let before = std::fs::read_to_string(root.join("index.toml")).unwrap();

        let missing = SourceSpec::Local {
            path: root.join("no-such-plugin"),
        };
        assert!(reg.install(&missing).await.is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("index.toml")).unwrap(),
            before
        );
        assert!(
            reg.package_dir("stable")
                .unwrap()
                .join("plugin.toml")
                .is_file()
        );

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A gzipped tar holding `top/bin/tool`, and its SHA-256.
    fn tool_archive(path: &Path) -> String {
        let f = std::fs::File::create(path).unwrap();
        let gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        let mut b = tar::Builder::new(gz);
        let body = b"#!/bin/sh\necho tool\n";
        let mut h = tar::Header::new_gnu();
        h.set_mode(0o755);
        h.set_size(body.len() as u64);
        h.set_entry_type(tar::EntryType::Regular);
        b.append_data(&mut h, "top/bin/tool", &body[..]).unwrap();
        b.into_inner().unwrap().finish().unwrap();
        crate::fetch::sha256_file(path).unwrap()
    }

    /// 013 C-L11 and C-L5: a declared build lands in `dist/` before the hash is
    /// taken; a hash other than the expected one installs nothing.
    #[tokio::test]
    async fn builds_and_expected_hashes() {
        let (reg, root) = tmp_registry();
        let src = plugin_dir("withbuild", "1.0.0");
        let archive = src.with_extension("tgz");
        let sha = tool_archive(&archive);
        let manifest = format!(
            "name = \"withbuild\"\nversion = \"1.0.0\"\napi_version = \"1.1.0\"\n\
             [builds.{}]\nurl = \"file://{}\"\nsha256 = \"{sha}\"\nstrip = 1\n",
            client_platform(),
            archive.display()
        );
        std::fs::write(src.join("plugin.toml"), &manifest).unwrap();
        let spec = SourceSpec::Local { path: src.clone() };

        let first = reg.install_pinned(&spec, None, None).await.unwrap();
        let dir = reg.package_dir("withbuild").unwrap();
        assert!(
            dir.join("dist/bin/tool").is_file(),
            "build unpacked into dist/"
        );
        assert!(
            !src.join("dist").exists(),
            "the source directory is untouched"
        );
        let again = reg
            .install_pinned(&spec, None, Some(&first.hash))
            .await
            .unwrap();
        assert_eq!(again.hash, first.hash, "reproducible");

        let before = std::fs::read_to_string(root.join("index.toml")).unwrap();
        let err = reg
            .install_pinned(&spec, None, Some(&"0".repeat(64)))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not match the lock file"), "{err}");
        assert_eq!(
            std::fs::read_to_string(root.join("index.toml")).unwrap(),
            before,
            "nothing changed"
        );

        // A build with a wrong checksum is refused.
        std::fs::write(
            src.join("plugin.toml"),
            manifest.replace(&sha, &"1".repeat(64)),
        )
        .unwrap();
        let err = reg.install(&spec).await.unwrap_err().to_string();
        assert!(err.contains("integrity"), "{err}");
        let _ = std::fs::remove_file(&archive);
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&root);
    }
}
