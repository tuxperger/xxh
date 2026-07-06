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

        let mut index = self.load_index()?;
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

    /// Re-fetch a plugin from its recorded source (T035 update).
    pub async fn update(&self, name: &str) -> Result<Manifest, PluginError> {
        let entry = self.entry(name)?;
        self.install(&entry.source).await
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

    fn entry(&self, name: &str) -> Result<IndexEntry, PluginError> {
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
}
