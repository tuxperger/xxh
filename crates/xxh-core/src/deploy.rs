//! Packaging, content-addressing and cache diffing for delivery (T015).
//!
//! Each component (shell, plugin, config bundle) is packed into a tar archive
//! (zstd when the host supports it, else gzip) and addressed by the blake3 hash of
//! its *packed* bytes. Only components whose hash is absent on the host are sent
//! (§FR-013, Принцип VI). See contracts/bootstrap-protocol.md.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

use crate::ShellError;

/// What a component contains (influences assembly order, not the hash).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentKind {
    Shell,
    Plugin,
    Config,
}

/// A packed, content-addressed unit of delivery.
#[derive(Debug, Clone)]
pub struct Component {
    pub kind: ComponentKind,
    /// blake3 hex of `payload` — the address in the host cache.
    pub hash: String,
    /// Archive format of `payload`: `"zst"` or `"gz"`.
    pub fmt: &'static str,
    pub payload: Vec<u8>,
}

impl Component {
    /// Pack a directory into a component using the given format.
    pub fn pack_dir(kind: ComponentKind, dir: &Path, fmt: &str) -> Result<Self, ShellError> {
        let tar_bytes = tar_dir(dir)?;
        let (payload, fmt) = match fmt {
            "zst" => (
                zstd::stream::encode_all(&tar_bytes[..], 3)
                    .map_err(|e| ShellError::Other(format!("zstd: {e}")))?,
                "zst",
            ),
            _ => (gzip(&tar_bytes)?, "gz"),
        };
        let hash = blake3::hash(&payload).to_hex().to_string();
        Ok(Self {
            kind,
            hash,
            fmt,
            payload,
        })
    }
}

/// Which of `components` are missing from the host, given the set of hashes the host
/// already has (§FR-013). Returns the components that must be transferred.
pub fn missing<'a>(components: &'a [Component], host_has: &BTreeSet<String>) -> Vec<&'a Component> {
    components
        .iter()
        .filter(|c| !host_has.contains(&c.hash))
        .collect()
}

fn tar_dir(dir: &Path) -> Result<Vec<u8>, ShellError> {
    let mut buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        // Deterministic archive: append the directory tree under ".".
        builder
            .append_dir_all(".", dir)
            .map_err(|e| ShellError::Other(format!("tar: {e}")))?;
        builder
            .finish()
            .map_err(|e| ShellError::Other(format!("tar finish: {e}")))?;
    }
    Ok(buf)
}

fn gzip(data: &[u8]) -> Result<Vec<u8>, ShellError> {
    // gzip is the portable fallback for hosts without zstd (part of the minimal
    // host contract). Wrapped so the backend can be swapped without touching callers.
    let mut out = Vec::new();
    let mut enc = GzWriter::new(&mut out);
    enc.write_all(data)
        .map_err(|e| ShellError::Other(format!("gzip: {e}")))?;
    enc.finish()
        .map_err(|e| ShellError::Other(format!("gzip finish: {e}")))?;
    Ok(out)
}

// Thin wrapper so the gzip backend can be swapped without touching callers.
struct GzWriter<'a> {
    inner: flate2::write::GzEncoder<&'a mut Vec<u8>>,
}
impl<'a> GzWriter<'a> {
    fn new(out: &'a mut Vec<u8>) -> Self {
        Self {
            inner: flate2::write::GzEncoder::new(out, flate2::Compression::default()),
        }
    }
    fn finish(self) -> std::io::Result<()> {
        self.inner.finish().map(|_| ())
    }
}
impl Write for GzWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn fixture_dir() -> tempdir_lite::TempDir {
        let d = tempdir_lite::TempDir::new();
        std::fs::write(d.path().join("greet.sh"), b"echo hi\n").unwrap();
        d
    }

    #[test]
    fn packing_is_content_addressed_and_deterministic() {
        let d = fixture_dir();
        let a = Component::pack_dir(ComponentKind::Shell, d.path(), "zst").unwrap();
        let b = Component::pack_dir(ComponentKind::Shell, d.path(), "zst").unwrap();
        assert_eq!(a.hash, b.hash, "same content ⇒ same hash");
        assert_eq!(a.hash.len(), 64);
        assert_eq!(a.fmt, "zst");
    }

    #[test]
    fn gzip_fallback_produces_a_component() {
        let d = fixture_dir();
        let c = Component::pack_dir(ComponentKind::Plugin, d.path(), "gz").unwrap();
        assert_eq!(c.fmt, "gz");
        assert!(!c.payload.is_empty());
    }

    #[test]
    fn missing_skips_hashes_already_on_host() {
        let d = fixture_dir();
        let c = Component::pack_dir(ComponentKind::Config, d.path(), "gz").unwrap();
        let comps = vec![c.clone()];

        let empty = BTreeSet::new();
        assert_eq!(missing(&comps, &empty).len(), 1, "nothing cached ⇒ send it");

        let mut has = BTreeSet::new();
        has.insert(c.hash.clone());
        assert_eq!(missing(&comps, &has).len(), 0, "already cached ⇒ skip");
    }
}

/// Tiny self-contained temp-dir helper for tests (avoids an extra dependency).
#[cfg(test)]
mod tempdir_lite {
    use std::path::{Path, PathBuf};

    pub struct TempDir(PathBuf);
    impl TempDir {
        pub fn new() -> Self {
            let base = std::env::temp_dir().join(format!(
                "xxh-test-{}-{}",
                std::process::id(),
                fastrand()
            ));
            std::fs::create_dir_all(&base).unwrap();
            Self(base)
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn fastrand() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .subsec_nanos() as u64
    }
}
