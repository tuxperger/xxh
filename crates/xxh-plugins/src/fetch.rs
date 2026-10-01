//! Downloading, checking and unpacking build archives (008 C-B2..C-B3; shared by
//! shell packages and, since 013, by plugins that declare `[builds]`).
//!
//! Archives are fetched with the client's `curl` (an argument array, only
//! `https`/`file`), checked against their SHA-256 **before** anything is
//! unpacked, and unpacked without letting any entry escape the destination.
//! Errors are plain messages; callers wrap them in their own error class.

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

fn err(msg: impl Into<String>) -> String {
    msg.into()
}

pub fn unique(prefix: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Download `url` to `dest` with the client's `curl` — an argument array, only
/// `https`/`file` (research R2).
pub async fn download(url: &str, dest: &Path) -> Result<(), String> {
    let out = tokio::process::Command::new("curl")
        .args(["-fsSL", "--proto", "=https,file", "-o"])
        .arg(dest)
        .arg(url)
        .output()
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                err("`curl` is needed to download shell builds; install curl and retry")
            }
            _ => err(format!("running curl: {e}")),
        })?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        return Err(err(format!("downloading {url} failed: {}", msg.trim())));
    }
    Ok(())
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| err(format!("{}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| err(format!("{}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    use std::fmt::Write as _;
    let mut hex = String::with_capacity(64);
    for b in hasher.finalize() {
        let _ = write!(hex, "{b:02x}");
    }
    Ok(hex)
}

/// The path of an archive entry after dropping `strip` leading components, or an
/// error if it is absolute or climbs out (C-B3). `None`: nothing left.
fn entry_path(path: &Path, strip: u32) -> Result<Option<PathBuf>, String> {
    let mut rel = PathBuf::new();
    let mut skipped = 0;
    for c in path.components() {
        match c {
            Component::Normal(_) if skipped < strip => skipped += 1,
            Component::Normal(part) => rel.push(part),
            Component::CurDir => {}
            _ => {
                return Err(err(format!(
                    "archive entry {} leaves the build directory",
                    path.display()
                )));
            }
        }
    }
    Ok((!rel.as_os_str().is_empty()).then_some(rel))
}

/// Whether a symlink at `rel` pointing to `target` stays inside the build.
fn link_stays_inside(rel: &Path, target: &Path) -> bool {
    let mut depth: Vec<&std::ffi::OsStr> = rel
        .parent()
        .map(|p| {
            p.components()
                .filter_map(|c| match c {
                    Component::Normal(s) => Some(s),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    for c in target.components() {
        match c {
            Component::Normal(s) => depth.push(s),
            Component::CurDir => {}
            Component::ParentDir => {
                if depth.pop().is_none() {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

/// No component between `root` and `path` may be a symlink — writing through one
/// could land outside the build (C-B3).
fn no_symlink_on_the_way(root: &Path, rel: &Path) -> bool {
    let mut cur = root.to_path_buf();
    if let Some(parent) = rel.parent() {
        for c in parent.components() {
            cur.push(c);
            if std::fs::symlink_metadata(&cur).is_ok_and(|m| m.file_type().is_symlink()) {
                return false;
            }
        }
    }
    true
}

/// Unpack a tar archive (plain, gzip or zstd by signature) into `dest`, dropping
/// `strip` leading components and refusing every entry that could escape (C-B3).
pub fn unpack(archive: &Path, dest: &Path, strip: u32) -> Result<(), String> {
    let io = |e: std::io::Error| err(format!("unpacking {}: {e}", archive.display()));
    let mut f = std::fs::File::open(archive).map_err(io)?;
    let mut magic = [0u8; 4];
    let n = f.read(&mut magic).map_err(io)?;
    let f = std::fs::File::open(archive).map_err(io)?;
    let reader: Box<dyn Read> = match &magic[..n] {
        [0x1f, 0x8b, ..] => Box::new(flate2::read::GzDecoder::new(f)),
        [0x28, 0xb5, 0x2f, 0xfd] => Box::new(zstd::stream::read::Decoder::new(f).map_err(io)?),
        _ => Box::new(f),
    };
    std::fs::create_dir_all(dest).map_err(io)?;
    let mut tar = tar::Archive::new(reader);
    for entry in tar.entries().map_err(io)? {
        let mut entry = entry.map_err(io)?;
        let raw = entry.path().map_err(io)?.into_owned();
        let Some(rel) = entry_path(&raw, strip)? else {
            continue;
        };
        if !no_symlink_on_the_way(dest, &rel) {
            return Err(err(format!(
                "archive entry {} would be written through a symlink",
                raw.display()
            )));
        }
        let out = dest.join(&rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        match entry.header().entry_type() {
            tar::EntryType::Symlink => {
                let target = entry
                    .link_name()
                    .map_err(io)?
                    .ok_or_else(|| err("symlink without a target"))?
                    .into_owned();
                if !link_stays_inside(&rel, &target) {
                    return Err(err(format!(
                        "archive symlink {} points outside the build",
                        raw.display()
                    )));
                }
                #[cfg(unix)]
                std::os::unix::fs::symlink(&target, &out).map_err(io)?;
            }
            tar::EntryType::Link => {
                let target = entry
                    .link_name()
                    .map_err(io)?
                    .ok_or_else(|| err("hard link without a target"))?
                    .into_owned();
                let Some(src) = entry_path(&target, strip)? else {
                    return Err(err("hard link to the archive root"));
                };
                std::fs::hard_link(dest.join(src), &out).map_err(io)?;
            }
            _ => {
                entry.unpack(&out).map_err(io)?;
            }
        }
    }
    Ok(())
}

/// Copy a tree, skipping top-level names in `skip`. Symlinks are copied as links.
pub fn copy_tree(src: &Path, dst: &Path, skip: &[&str]) -> Result<(), String> {
    let io = |e: std::io::Error| err(format!("copying {}: {e}", src.display()));
    std::fs::create_dir_all(dst).map_err(io)?;
    for e in std::fs::read_dir(src).map_err(io)? {
        let e = e.map_err(io)?;
        let name = e.file_name();
        if skip.iter().any(|s| name == *s) {
            continue;
        }
        let from = e.path();
        let to = dst.join(&name);
        let ft = e.file_type().map_err(io)?;
        if ft.is_dir() {
            copy_tree(&from, &to, &[])?;
        } else if ft.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(&from).map_err(io)?, &to).map_err(io)?;
        } else {
            std::fs::copy(&from, &to).map_err(io)?;
        }
    }
    Ok(())
}

pub fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(unique(&format!("xxh-fetch-{name}")));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn raw_tar(path: &Path, name: &str) {
        let mut h = tar::Header::new_old();
        let bytes = name.as_bytes();
        h.as_old_mut().name[..bytes.len()].copy_from_slice(bytes);
        h.set_size(1);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        let mut data = h.as_bytes().to_vec();
        data.extend_from_slice(b"x");
        data.resize(data.len() + 511, 0);
        data.extend_from_slice(&[0u8; 1024]);
        std::fs::write(path, data).unwrap();
    }

    #[test]
    fn entry_paths_cannot_escape() {
        assert_eq!(
            entry_path(Path::new("./bin/zsh"), 0).unwrap(),
            Some(PathBuf::from("bin/zsh"))
        );
        assert_eq!(
            entry_path(Path::new("zsh-5.8/bin/zsh"), 1).unwrap(),
            Some(PathBuf::from("bin/zsh"))
        );
        assert_eq!(entry_path(Path::new("zsh-5.8/"), 1).unwrap(), None);
        assert!(entry_path(Path::new("/etc/passwd"), 0).is_err());
        assert!(entry_path(Path::new("a/../../x"), 0).is_err());
        assert!(link_stays_inside(Path::new("bin/sh"), Path::new("zsh")));
        assert!(link_stays_inside(
            Path::new("bin/sh"),
            Path::new("../lib/x")
        ));
        assert!(!link_stays_inside(
            Path::new("bin/sh"),
            Path::new("../../x")
        ));
        assert!(!link_stays_inside(Path::new("sh"), Path::new("/bin/sh")));
    }

    #[test]
    fn hostile_archives_are_refused() {
        let d = scratch("hostile");
        for name in ["/abs", "../up"] {
            let a = d.join("a.tar");
            raw_tar(&a, name);
            assert!(unpack(&a, &d.join("out"), 0).is_err(), "{name}");
        }
        // A symlink pointing out of the build.
        let a = d.join("l.tar");
        let mut b = tar::Builder::new(std::fs::File::create(&a).unwrap());
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        b.append_link(&mut h, "esc", "../../etc").unwrap();
        b.finish().unwrap();
        assert!(unpack(&a, &d.join("out2"), 0).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
