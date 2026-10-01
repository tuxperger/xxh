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
        let provider = provider_for(spec)?;
        let fetched = provider.fetch(spec).await?;
        let result = self.store(spec, &fetched);
        if let Some(tmp) = &fetched.cleanup {
            let _ = std::fs::remove_dir_all(tmp);
        }
        result.map(|_| fetched.manifest)
    }

    fn store(&self, spec: &SourceSpec, fetched: &FetchedPackage) -> Result<(), PluginError> {
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

        let hash = hash_dir(&fetched.dir)?;
        let dest = self.root.join("packages").join(&hash);
        if !dest.is_dir() {
            let staging = self.root.join("packages").join(format!(".tmp-{hash}"));
            let _ = std::fs::remove_dir_all(&staging);
            copy_tree(&fetched.dir, &staging)
                .map_err(|e| PluginError::Other(format!("storing package: {e}")))?;
            std::fs::rename(&staging, &dest)
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

    /// What a provider hands back, built by hand so the registry rules can be
    /// tested without Nix.
    fn fetched(dir: &Path, resolved: Option<SourceSpec>) -> FetchedPackage {
        FetchedPackage {
            manifest: crate::source::read_manifest(dir).unwrap(),
            dir: dir.to_path_buf(),
            env: BTreeMap::new(),
            cleanup: None,
            resolved,
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
        reg.store(
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
        let err = reg
            .store(&flake_spec("github:o/r"), &fetched(&src, None))
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
        reg.store(&flake_spec("github:o/r"), &fetched(&src, None))
            .unwrap();
        reg.store(&flake_spec("github:o/r"), &fetched(&src, None))
            .unwrap();
        // …while another flake of the same name, or a local plugin, is a conflict.
        assert!(
            reg.store(&flake_spec("github:x/y"), &fetched(&src, None))
                .is_err()
        );
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
}
