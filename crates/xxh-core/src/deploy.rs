//! Packaging, content-addressing and cache diffing for delivery (T015, 023).
//!
//! A component (shell, plugin, config bundle) is addressed by the blake3 hash of
//! its **tree** — sorted paths, permission bits and file contents — so the address
//! is known without packing anything and never depends on timestamps or on the
//! archive format (023, contracts/component-address.md). Only components whose
//! address is absent on the host are packed (deterministic tar; zstd when the host
//! supports it, else gzip) and sent (§FR-013, Принцип VI). See
//! contracts/bootstrap-protocol.md.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::ShellError;

/// What a component contains (influences assembly order, not the hash).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentKind {
    Shell,
    Plugin,
    Config,
}

/// Where a component's archive comes from when it has to be sent.
#[derive(Debug, Clone)]
enum Source {
    /// A directory on the client, packed on demand.
    Dir(PathBuf),
    /// Already packed — for small generated components whose directory is gone.
    Packed(Vec<u8>),
}

/// A content-addressed unit of delivery.
#[derive(Debug, Clone)]
pub struct Component {
    pub kind: ComponentKind,
    /// blake3 hex of the component's tree — the address in the host cache (C-A1).
    pub hash: String,
    /// Archive format used if the component is sent: `"zst"` or `"gz"`. Not part
    /// of the address.
    pub fmt: &'static str,
    source: Source,
}

fn fmt_of(fmt: &str) -> &'static str {
    if fmt == "zst" { "zst" } else { "gz" }
}

impl Component {
    /// Address a directory as a component. Nothing is packed here (C-A3): the
    /// archive is produced by [`payload`](Self::payload), and only when the host
    /// lacks the component. The directory must outlive the component.
    pub fn pack_dir(kind: ComponentKind, dir: &Path, fmt: &str) -> Result<Self, ShellError> {
        Ok(Self {
            kind,
            hash: tree_hash(dir)?,
            fmt: fmt_of(fmt),
            source: Source::Dir(dir.to_path_buf()),
        })
    }

    /// Like [`pack_dir`](Self::pack_dir) but packs immediately, for a temporary
    /// directory the caller is about to delete.
    pub fn pack_dir_eager(kind: ComponentKind, dir: &Path, fmt: &str) -> Result<Self, ShellError> {
        let fmt = fmt_of(fmt);
        Ok(Self {
            kind,
            hash: tree_hash(dir)?,
            fmt,
            source: Source::Packed(compress(&tar_dir(dir)?, fmt)?),
        })
    }

    /// The archive to send. For a directory source this is where the packing
    /// happens — reusing the client's archive cache when it holds a valid copy.
    pub fn payload(&self) -> Result<Vec<u8>, ShellError> {
        self.payload_with(pack_cache_dir().as_deref())
    }

    fn payload_with(&self, cache: Option<&Path>) -> Result<Vec<u8>, ShellError> {
        match &self.source {
            Source::Packed(bytes) => Ok(bytes.clone()),
            Source::Dir(dir) => {
                if let Some(hit) = cache.and_then(|c| cache_read(c, &self.hash, self.fmt)) {
                    return Ok(hit);
                }
                let packed = compress(&tar_dir(dir)?, self.fmt)?;
                if let Some(c) = cache {
                    cache_write(c, &self.hash, self.fmt, &packed);
                }
                Ok(packed)
            }
        }
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

/// One entry of a component tree, relative to its root.
struct Entry {
    rel: String,
    path: PathBuf,
    is_dir: bool,
    mode: u32,
}

/// Every directory and file under `dir`, sorted by relative path. Symlinks are
/// followed, so the walk describes what the archive will actually contain.
fn entries(dir: &Path) -> Result<Vec<Entry>, ShellError> {
    fn mode_of(meta: &std::fs::Metadata) -> u32 {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            meta.permissions().mode() & 0o777
        }
        #[cfg(not(unix))]
        {
            if meta.is_dir() { 0o755 } else { 0o644 }
        }
    }
    fn walk(base: &Path, dir: &Path, out: &mut Vec<Entry>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            let meta = std::fs::metadata(&path)?;
            let rel = path
                .strip_prefix(base)
                .expect("child of base")
                .to_string_lossy()
                .into_owned();
            let is_dir = meta.is_dir();
            out.push(Entry {
                rel,
                path: path.clone(),
                is_dir,
                mode: mode_of(&meta),
            });
            if is_dir {
                walk(base, &path, out)?;
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)
        .map_err(|e| ShellError::Other(format!("reading {}: {e}", dir.display())))?;
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

/// The component address: blake3 over the canonical description of the tree —
/// for each entry in path order its kind, path, permission bits and, for files,
/// length and contents (data-model.md). File contents are always read, so an edit
/// that keeps size and mtime is still seen (023 §FR-004).
pub fn tree_hash(dir: &Path) -> Result<String, ShellError> {
    let mut hasher = blake3::Hasher::new();
    for e in entries(dir)? {
        hasher.update(if e.is_dir { b"d" } else { b"f" });
        hasher.update(e.rel.as_bytes());
        hasher.update(&[0]);
        hasher.update(&e.mode.to_le_bytes());
        if !e.is_dir {
            let data = std::fs::read(&e.path)
                .map_err(|err| ShellError::Other(format!("reading {}: {err}", e.path.display())))?;
            hasher.update(&(data.len() as u64).to_le_bytes());
            hasher.update(&data);
        }
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Timestamp stamped on every archive entry (2000-01-01): fixed so the archive is
/// reproducible, and not the epoch, which GNU tar reports as implausibly old.
const ARCHIVE_MTIME: u64 = 946_684_800;

/// Deterministic tar of a tree (C-A5): entries in path order, fixed mtime and
/// owner, permission bits from the files — the same bytes on every run.
fn tar_dir(dir: &Path) -> Result<Vec<u8>, ShellError> {
    let tar_err = |e: std::io::Error| ShellError::Other(format!("tar: {e}"));
    let mut buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        for e in entries(dir)? {
            let mut header = tar::Header::new_gnu();
            header.set_mode(e.mode);
            header.set_mtime(ARCHIVE_MTIME);
            header.set_uid(0);
            header.set_gid(0);
            if e.is_dir {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                builder
                    .append_data(&mut header, format!("{}/", e.rel), std::io::empty())
                    .map_err(tar_err)?;
            } else {
                let data = std::fs::read(&e.path).map_err(tar_err)?;
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(data.len() as u64);
                builder
                    .append_data(&mut header, &e.rel, &data[..])
                    .map_err(tar_err)?;
            }
        }
        builder.finish().map_err(tar_err)?;
    }
    Ok(buf)
}

fn compress(tar_bytes: &[u8], fmt: &str) -> Result<Vec<u8>, ShellError> {
    match fmt {
        "zst" => zstd::stream::encode_all(tar_bytes, 3)
            .map_err(|e| ShellError::Other(format!("zstd: {e}"))),
        _ => gzip(tar_bytes),
    }
}

/// How many packed archives the client keeps (oldest evicted first).
const PACK_CACHE_ENTRIES: usize = 64;

/// The client's cache of packed archives (research R4): `$XXH_PACK_CACHE_DIR`, else
/// `<user cache dir>/xxh/packed`. Unit tests get no implicit cache, so they never
/// touch the developer's.
fn pack_cache_dir() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("XXH_PACK_CACHE_DIR") {
        return Some(PathBuf::from(v));
    }
    if cfg!(test) {
        return None;
    }
    directories::BaseDirs::new().map(|b| b.cache_dir().join("xxh").join("packed"))
}

/// A cached archive, if present and intact: the entry is `blake3(archive)`
/// followed by the archive, and a mismatch is treated as a miss (C-A6).
fn cache_read(cache: &Path, hash: &str, fmt: &str) -> Option<Vec<u8>> {
    let data = std::fs::read(cache.join(format!("{hash}.{fmt}"))).ok()?;
    let (sum, archive) = data.split_at_checked(blake3::OUT_LEN)?;
    (blake3::hash(archive).as_bytes() == sum).then(|| archive.to_vec())
}

/// Store an archive, atomically; every failure is ignored — the cache is an
/// optimisation, never a reason for a connect to fail (C-A6).
fn cache_write(cache: &Path, hash: &str, fmt: &str, archive: &[u8]) {
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(cache)?;
        let tmp = cache.join(format!(".tmp-{hash}.{fmt}.{}", std::process::id()));
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(blake3::hash(archive).as_bytes())?;
        f.write_all(archive)?;
        drop(f);
        std::fs::rename(&tmp, cache.join(format!("{hash}.{fmt}")))
    };
    if write().is_ok() {
        cache_evict(cache);
    }
}

fn cache_evict(cache: &Path) {
    let Ok(dir) = std::fs::read_dir(cache) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = dir
        .filter_map(|e| e.ok())
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= PACK_CACHE_ENTRIES {
        return;
    }
    files.sort();
    for (_, path) in &files[..files.len() - PACK_CACHE_ENTRIES] {
        let _ = std::fs::remove_file(path);
    }
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
        std::fs::create_dir_all(d.path().join("bin")).unwrap();
        std::fs::write(d.path().join("bin/tool"), b"#!/bin/sh\necho tool\n").unwrap();
        set_mode(&d.path().join("bin/tool"), 0o755);
        d
    }

    fn set_mode(path: &Path, mode: u32) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
    }

    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in entries(from).unwrap() {
            if e.is_dir {
                std::fs::create_dir_all(to.join(&e.rel)).unwrap();
            } else {
                std::fs::copy(&e.path, to.join(&e.rel)).unwrap();
            }
        }
    }

    /// Push every mtime in the tree into the past without changing any content.
    fn age(dir: &Path) {
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        for e in entries(dir).unwrap() {
            if !e.is_dir {
                std::fs::File::options()
                    .write(true)
                    .open(&e.path)
                    .unwrap()
                    .set_modified(old)
                    .unwrap();
            }
        }
    }

    #[test]
    fn address_depends_only_on_the_tree() {
        let d = fixture_dir();
        let a = Component::pack_dir(ComponentKind::Shell, d.path(), "zst").unwrap();
        assert_eq!(a.hash.len(), 64);
        assert_eq!(a.fmt, "zst");
        // Not on the archive format…
        let gz = Component::pack_dir(ComponentKind::Shell, d.path(), "gz").unwrap();
        assert_eq!(a.hash, gz.hash);
        // …nor on where the tree lives, nor on its timestamps (C-A1).
        let other = tempdir_lite::TempDir::new();
        copy(d.path(), other.path());
        age(other.path());
        assert_eq!(tree_hash(other.path()).unwrap(), a.hash);
        // The eager constructor addresses identically.
        let eager = Component::pack_dir_eager(ComponentKind::Shell, d.path(), "gz").unwrap();
        assert_eq!(eager.hash, a.hash);
    }

    #[test]
    fn any_change_to_the_tree_changes_the_address() {
        let d = fixture_dir();
        let base = tree_hash(d.path()).unwrap();

        // Same length, same mtime, different bytes (023 §FR-004).
        let file = d.path().join("greet.sh");
        let mtime = std::fs::metadata(&file).unwrap().modified().unwrap();
        std::fs::write(&file, b"echo ho\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        let edited = tree_hash(d.path()).unwrap();
        assert_ne!(edited, base, "content edit");

        std::fs::rename(&file, d.path().join("hello.sh")).unwrap();
        let renamed = tree_hash(d.path()).unwrap();
        assert_ne!(renamed, edited, "rename");

        set_mode(&d.path().join("bin/tool"), 0o644);
        assert_ne!(tree_hash(d.path()).unwrap(), renamed, "permission change");
    }

    #[test]
    fn archive_is_reproducible_byte_for_byte() {
        let d = fixture_dir();
        for fmt in ["gz", "zst"] {
            let c = Component::pack_dir(ComponentKind::Plugin, d.path(), fmt).unwrap();
            let first = c.payload().unwrap();
            assert!(!first.is_empty());
            age(d.path());
            assert_eq!(
                c.payload().unwrap(),
                first,
                "{fmt}: mtimes must not leak in"
            );
        }
    }

    /// The archive must unpack with the host's plain `tar` and keep the
    /// executable bit (C-A7) — that is what the bootstrap script does with it.
    #[test]
    fn archive_unpacks_with_system_tar_and_keeps_modes() {
        let d = fixture_dir();
        let c = Component::pack_dir(ComponentKind::Plugin, d.path(), "gz").unwrap();
        let out = tempdir_lite::TempDir::new();
        let archive = out.path().join("a.tgz");
        std::fs::write(&archive, c.payload().unwrap()).unwrap();
        let dest = out.path().join("x");
        std::fs::create_dir_all(&dest).unwrap();
        let tar = std::process::Command::new("sh")
            .arg("-c")
            .arg("gzip -dc \"$1\" | tar -xf - -C \"$2\" 2>&1")
            .args(["sh", archive.to_str().unwrap(), dest.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(tar.status.success());
        assert_eq!(
            String::from_utf8_lossy(&tar.stdout),
            "",
            "tar must not warn"
        );
        assert_eq!(
            tree_hash(&dest).unwrap(),
            c.hash,
            "round trip preserves the tree"
        );
        let run = std::process::Command::new(dest.join("bin/tool"))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&run.stdout), "tool\n");
    }

    #[test]
    fn eager_component_survives_its_directory() {
        let d = fixture_dir();
        let c = Component::pack_dir_eager(ComponentKind::Config, d.path(), "gz").unwrap();
        drop(d);
        assert!(!c.payload().unwrap().is_empty());
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

    #[test]
    fn archive_cache_serves_valid_entries_only() {
        let cache = tempdir_lite::TempDir::new();
        let archive = b"packed bytes".to_vec();
        assert_eq!(cache_read(cache.path(), "h", "gz"), None, "empty cache");

        cache_write(cache.path(), "h", "gz", &archive);
        assert_eq!(cache_read(cache.path(), "h", "gz"), Some(archive.clone()));
        assert_eq!(
            cache_read(cache.path(), "h", "zst"),
            None,
            "format is part of the key"
        );

        // A damaged entry is a miss, never an error, and gets overwritten (C-A6).
        let entry = cache.path().join("h.gz");
        let mut bytes = std::fs::read(&entry).unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        std::fs::write(&entry, &bytes).unwrap();
        assert_eq!(cache_read(cache.path(), "h", "gz"), None, "corrupted");
        std::fs::write(&entry, b"short").unwrap();
        assert_eq!(cache_read(cache.path(), "h", "gz"), None, "truncated");
        cache_write(cache.path(), "h", "gz", &archive);
        assert_eq!(cache_read(cache.path(), "h", "gz"), Some(archive.clone()));

        // An unusable cache location is silently skipped.
        let blocked = cache.path().join("h.gz").join("not-a-dir");
        cache_write(&blocked, "x", "gz", &archive);
        assert_eq!(cache_read(&blocked, "x", "gz"), None);
    }

    /// Once packed, an unchanged component is never packed again — the archive
    /// comes from the cache even if its source directory is gone (023 §FR-003).
    #[test]
    fn unchanged_component_is_packed_once() {
        let cache = tempdir_lite::TempDir::new();
        let d = fixture_dir();
        let c = Component::pack_dir(ComponentKind::Plugin, d.path(), "gz").unwrap();
        let first = c.payload_with(Some(cache.path())).unwrap();
        drop(d);
        assert_eq!(c.payload_with(Some(cache.path())).unwrap(), first);
        // Without the cache the missing directory is an error, proving the hit.
        assert!(c.payload_with(None).is_err());
    }

    #[test]
    fn archive_cache_is_bounded() {
        let cache = tempdir_lite::TempDir::new();
        for i in 0..PACK_CACHE_ENTRIES + 5 {
            cache_write(cache.path(), &format!("h{i}"), "gz", b"x");
        }
        let left = std::fs::read_dir(cache.path()).unwrap().count();
        assert_eq!(left, PACK_CACHE_ENTRIES);
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
