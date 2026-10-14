//! `xxh sync`: make the installed plugins and shells match what the config
//! declares and what the lock file pins (013; contracts/lock-and-sync.md).
//!
//! For every declaration: already installed with the locked hash — nothing to do
//! (C-L4); locked — install at the locked revision and insist on the locked hash
//! (C-L5); not locked yet or its source changed — already installed from the
//! declared source: record what is installed (024 C-A1), otherwise install from
//! the source and record it (C-L6). Lock entries nothing declares any more are dropped (C-L7). A
//! lock file that cannot be written (Nix provides it read-only) is printed for
//! the user to carry over instead (C-L8). Plugins installed the old way, without
//! a declaration, are left alone (§FR-009).

use std::collections::BTreeMap;
use std::path::Path;

use xxh_config::{Config, Declared};
use xxh_plugins::lock::{Lock, LockEntry};
use xxh_plugins::registry::Registry;
use xxh_plugins::source::SourceSpec;

use crate::shellmgr::{self, Builds};

/// What happened to one declaration (C-L9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Unchanged,
    Installed(String),
    Updated { from: String, to: String },
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// `plugin` or `shell`.
    pub kind: &'static str,
    pub name: String,
    pub outcome: Outcome,
}

/// The whole run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub lines: Vec<Line>,
    /// Lock entries dropped because nothing declares them.
    pub dropped: Vec<String>,
    /// The lock changed and was written.
    pub lock_written: bool,
    /// The lock changed but could not be written: its new text (C-L8).
    pub lock_unwritable: Option<String>,
}

impl Report {
    pub fn failed(&self) -> bool {
        self.lines
            .iter()
            .any(|l| matches!(l.outcome, Outcome::Failed(_)))
    }
}

/// A short name for what was installed: the git commit or the content hash.
fn short(entry: &LockEntry) -> String {
    entry
        .revision
        .as_deref()
        .unwrap_or(&entry.hash)
        .chars()
        .take(12)
        .collect()
}

fn locked<'a>(
    lock: &'a BTreeMap<String, LockEntry>,
    name: &str,
    decl: &Declared,
) -> Option<&'a LockEntry> {
    lock.get(name).filter(|e| e.source == decl.source)
}

/// The lock entry for something already installed from the declared source,
/// recorded as an install would record it (024 C-A1, C-A2). A git install
/// without a known revision cannot be reproduced elsewhere, so it is not taken.
fn adopt(
    decl: &Declared,
    spec: &SourceSpec,
    installed_from: &SourceSpec,
    revision: Option<String>,
    hash: String,
) -> Option<LockEntry> {
    if !installed_from.same_origin(spec) {
        return None;
    }
    if matches!(spec, SourceSpec::Git { .. }) && revision.is_none() {
        return None;
    }
    Some(LockEntry {
        source: decl.source.clone(),
        revision,
        hash,
    })
}

async fn sync_plugin(
    registry: &Registry,
    name: &str,
    decl: &Declared,
    lock: &mut BTreeMap<String, LockEntry>,
) -> Outcome {
    let spec = match SourceSpec::parse(&decl.source) {
        Ok(s) => s,
        Err(e) => return Outcome::Failed(e.to_string()),
    };
    let entry = registry.entry(name).ok();
    let pinned = locked(lock, name, decl).cloned();
    if let Some(e) = &pinned {
        if entry.as_ref().map(|i| i.hash.as_str()) == Some(e.hash.as_str()) {
            return Outcome::Unchanged;
        }
    }
    // Not locked yet but already installed from this source: lock what is
    // there instead of fetching (and maybe building) it again (024 C-A1).
    if pinned.is_none() {
        if let Some(i) = &entry {
            if let Some(adopted) = adopt(decl, &spec, &i.source, i.revision.clone(), i.hash.clone())
            {
                lock.insert(name.to_string(), adopted);
                return Outcome::Unchanged;
            }
        }
    }
    let installed = entry.map(|e| e.hash);
    let (pin, expect) = match &pinned {
        Some(e) => (e.revision.as_deref(), Some(e.hash.as_str())),
        None => (None, None),
    };
    match registry.install_pinned(&spec, pin, expect).await {
        Ok(got) if got.manifest.name != name => Outcome::Failed(format!(
            "{} provides the plugin `{}`, not `{name}`",
            decl.source, got.manifest.name
        )),
        Ok(got) => {
            let entry = LockEntry {
                source: decl.source.clone(),
                revision: got.revision,
                hash: got.hash,
            };
            let outcome = match (&installed, pinned.is_some()) {
                (Some(old), _) if *old == entry.hash => Outcome::Unchanged,
                (Some(old), _) => Outcome::Updated {
                    from: old.chars().take(12).collect(),
                    to: short(&entry),
                },
                (None, _) => Outcome::Installed(short(&entry)),
            };
            lock.insert(name.to_string(), entry);
            outcome
        }
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

async fn sync_shell(
    name: &str,
    decl: &Declared,
    lock: &mut BTreeMap<String, LockEntry>,
) -> Outcome {
    let spec = match SourceSpec::parse(&decl.source) {
        Ok(s) => s,
        Err(e) => return Outcome::Failed(e.to_string()),
    };
    let installed = shellmgr::list()
        .ok()
        .and_then(|l| l.into_iter().find(|i| i.shell == name));
    let pinned = locked(lock, name, decl).cloned();
    let complete = |i: &shellmgr::Installed| i.missing.iter().all(|p| !p.starts_with("linux-"));
    if let (Some(e), Some(i)) = (&pinned, &installed) {
        if i.hash.as_deref() == Some(e.hash.as_str()) && complete(i) {
            return Outcome::Unchanged;
        }
    }
    // Same as for plugins, provided every Linux build is there (024 C-A3).
    if let (None, Some(i)) = (&pinned, &installed) {
        if let (Some(from), Some(hash), true) = (&i.source, &i.hash, complete(i)) {
            if let Some(adopted) = adopt(decl, &spec, from, i.revision.clone(), hash.clone()) {
                lock.insert(name.to_string(), adopted);
                return Outcome::Unchanged;
            }
        }
    }
    let (pin, expect) = match &pinned {
        Some(e) => (e.revision.as_deref(), Some(e.hash.as_str())),
        None => (None, None),
    };
    match shellmgr::add_pinned(&spec, pin, expect, &Builds::Default).await {
        Ok(got) if got.shell != name => Outcome::Failed(format!(
            "{} provides the shell `{}`, not `{name}`",
            decl.source, got.shell
        )),
        Ok(got) => {
            let entry = LockEntry {
                source: decl.source.clone(),
                revision: got.revision,
                hash: got.hash.unwrap_or_default(),
            };
            let before = installed.and_then(|i| i.hash);
            let outcome = match before {
                Some(old) if old == entry.hash => Outcome::Unchanged,
                Some(old) => Outcome::Updated {
                    from: old.chars().take(12).collect(),
                    to: short(&entry),
                },
                None => Outcome::Installed(short(&entry)),
            };
            lock.insert(name.to_string(), entry);
            outcome
        }
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

/// A lock file managed elsewhere (a Home Manager symlink into the store) must
/// not be replaced by a regular file.
fn writable(path: &Path) -> bool {
    !std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Bring the registry and the shells in line with `config` and the lock file
/// at `lock_path`.
pub async fn sync(
    config: &Config,
    registry: &Registry,
    lock_path: &Path,
) -> Result<Report, String> {
    let before = Lock::load(lock_path).map_err(|e| e.to_string())?;
    let mut lock = before.clone();
    let mut report = Report::default();

    for (name, decl) in &config.plugins {
        let outcome = sync_plugin(registry, name, decl, &mut lock.plugins).await;
        report.lines.push(Line {
            kind: "plugin",
            name: name.clone(),
            outcome,
        });
    }
    for (name, decl) in &config.shells {
        let outcome = sync_shell(name, decl, &mut lock.shells).await;
        report.lines.push(Line {
            kind: "shell",
            name: name.clone(),
            outcome,
        });
    }

    // Entries nothing declares any more (C-L7).
    let stale_plugins: Vec<String> = lock
        .plugins
        .keys()
        .filter(|n| !config.plugins.contains_key(*n))
        .cloned()
        .collect();
    for n in stale_plugins {
        lock.plugins.remove(&n);
        report.dropped.push(format!("plugin {n}"));
    }
    let stale_shells: Vec<String> = lock
        .shells
        .keys()
        .filter(|n| !config.shells.contains_key(*n))
        .cloned()
        .collect();
    for n in stale_shells {
        lock.shells.remove(&n);
        report.dropped.push(format!("shell {n}"));
    }

    if lock != before {
        if writable(lock_path) && lock.save(lock_path).is_ok() {
            report.lock_written = true;
        } else {
            report.lock_unwritable = Some(lock.render());
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shellpkg::testenv;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(xxh_plugins::fetch::unique(&format!("xxh-sync-{name}")));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn plugin(dir: &Path, name: &str, version: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            format!("name = \"{name}\"\nversion = \"{version}\"\napi_version = \"1.0.0\"\n"),
        )
        .unwrap();
    }

    fn config(plugins: &[(&str, &Path)]) -> Config {
        let mut c = Config::default();
        for (n, p) in plugins {
            c.plugins.insert(
                n.to_string(),
                Declared {
                    source: p.display().to_string(),
                },
            );
        }
        c
    }

    /// US1: a clean client gets the declared plugin and a lock entry; a second
    /// run changes nothing; a lock that disagrees with the content refuses it;
    /// an entry nothing declares is dropped (C-L4..C-L7).
    #[tokio::test]
    async fn sync_installs_locks_and_verifies() {
        let _g = testenv::shells_dir(&scratch("shells"));
        let d = scratch("plugins");
        let reg = Registry::open(d.join("registry"));
        let lock_path = d.join("xxh.lock");
        let src = d.join("p");
        plugin(&src, "p", "1.0.0");
        let cfg = config(&[("p", &src)]);

        let r = sync(&cfg, &reg, &lock_path).await.unwrap();
        assert!(matches!(r.lines[0].outcome, Outcome::Installed(_)), "{r:?}");
        assert!(r.lock_written);
        let lock = Lock::load(&lock_path).unwrap();
        let hash = lock.plugins["p"].hash.clone();
        assert_eq!(reg.entry("p").unwrap().hash, hash);

        let r = sync(&cfg, &reg, &lock_path).await.unwrap();
        assert_eq!(r.lines[0].outcome, Outcome::Unchanged);
        assert!(!r.lock_written, "nothing changed, nothing written");

        // The source changed but the lock says otherwise: refused, kept as is.
        reg.remove("p").unwrap();
        plugin(&src, "p", "2.0.0");
        let r = sync(&cfg, &reg, &lock_path).await.unwrap();
        assert!(r.failed());
        assert!(
            matches!(&r.lines[0].outcome, Outcome::Failed(m) if m.contains("lock file")),
            "{r:?}"
        );
        assert!(reg.entry("p").is_err(), "nothing installed");
        assert_eq!(Lock::load(&lock_path).unwrap().plugins["p"].hash, hash);

        // Nothing declares it: the entry goes.
        let r = sync(&Config::default(), &reg, &lock_path).await.unwrap();
        assert_eq!(r.dropped, ["plugin p"]);
        assert!(Lock::load(&lock_path).unwrap().plugins.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// C-L8: a lock provided read-only (a symlink, as Home Manager makes it) is
    /// never replaced; its new text is reported instead.
    #[tokio::test]
    async fn read_only_lock_is_reported_not_replaced() {
        let _g = testenv::shells_dir(&scratch("shells-ro"));
        let d = scratch("ro");
        let reg = Registry::open(d.join("registry"));
        let src = d.join("q");
        plugin(&src, "q", "1.0.0");
        let managed = d.join("managed.lock");
        std::fs::write(&managed, "").unwrap();
        let lock_path = d.join("xxh.lock");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&managed, &lock_path).unwrap();

        let r = sync(&config(&[("q", &src)]), &reg, &lock_path)
            .await
            .unwrap();
        assert!(!r.lock_written);
        assert!(
            r.lock_unwritable
                .as_deref()
                .unwrap()
                .contains("[plugins.q]")
        );
        assert!(
            std::fs::symlink_metadata(&lock_path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&managed).unwrap(), "");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 024 US1: without a lock, what is installed from the declared source is
    /// locked as is — the source is not even looked at (C-A1, C-A6).
    #[tokio::test]
    async fn installed_is_adopted_without_a_lock() {
        let _g = testenv::shells_dir(&scratch("shells-a"));
        let d = scratch("adopt");
        let reg = Registry::open(d.join("registry"));
        let lock_path = d.join("xxh.lock");
        let src = d.join("a");
        plugin(&src, "a", "1.0.0");
        let cfg = config(&[("a", &src)]);
        sync(&cfg, &reg, &lock_path).await.unwrap();
        let locked = Lock::load(&lock_path).unwrap().plugins["a"].clone();

        std::fs::remove_file(&lock_path).unwrap();
        std::fs::remove_dir_all(&src).unwrap();

        // A lock Nix provides read-only: the adopted entry is printed.
        let managed = d.join("managed.lock");
        std::fs::write(&managed, "").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&managed, &lock_path).unwrap();
        let r = sync(&cfg, &reg, &lock_path).await.unwrap();
        assert_eq!(r.lines[0].outcome, Outcome::Unchanged, "{r:?}");
        assert!(r.lock_unwritable.as_deref().unwrap().contains(&locked.hash));
        std::fs::remove_file(&lock_path).unwrap();

        let r = sync(&cfg, &reg, &lock_path).await.unwrap();
        assert_eq!(r.lines[0].outcome, Outcome::Unchanged, "{r:?}");
        assert!(r.lock_written);
        assert_eq!(Lock::load(&lock_path).unwrap().plugins["a"], locked);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 024 US2: installed from another source — installed from the declared one
    /// (C-A4).
    #[tokio::test]
    async fn another_source_is_installed_not_adopted() {
        let _g = testenv::shells_dir(&scratch("shells-s"));
        let d = scratch("switch");
        let reg = Registry::open(d.join("registry"));
        let lock_path = d.join("xxh.lock");
        let (s1, s2) = (d.join("s1"), d.join("s2"));
        plugin(&s1, "s", "1.0.0");
        plugin(&s2, "s", "2.0.0");
        sync(&config(&[("s", &s1)]), &reg, &lock_path)
            .await
            .unwrap();
        std::fs::remove_file(&lock_path).unwrap();

        let r = sync(&config(&[("s", &s2)]), &reg, &lock_path)
            .await
            .unwrap();
        assert!(
            matches!(r.lines[0].outcome, Outcome::Updated { .. }),
            "{r:?}"
        );
        assert_eq!(reg.entry("s").unwrap().version, "2.0.0");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 024 C-A2: a git install is adopted only with its revision known.
    #[test]
    fn git_needs_a_revision_to_be_adopted() {
        let decl = Declared {
            source: "file:///r.git".into(),
        };
        let spec = SourceSpec::parse(&decl.source).unwrap();
        assert_eq!(adopt(&decl, &spec, &spec, None, "h".into()), None);
        let e = adopt(&decl, &spec, &spec, Some("c1".into()), "h".into()).unwrap();
        assert_eq!(
            (e.source.as_str(), e.revision.as_deref(), e.hash.as_str()),
            ("file:///r.git", Some("c1"), "h")
        );
        let nix = SourceSpec::parse("nixpkgs:htop").unwrap();
        let nix_decl = Declared {
            source: "nixpkgs:htop".into(),
        };
        assert!(adopt(&nix_decl, &nix, &nix, None, "h".into()).is_some());
        assert_eq!(adopt(&nix_decl, &nix, &spec, None, "h".into()), None);
    }

    /// A declaration whose source provides another name is reported.
    #[tokio::test]
    async fn names_must_match() {
        let _g = testenv::shells_dir(&scratch("shells-n"));
        let d = scratch("names");
        let reg = Registry::open(d.join("registry"));
        let src = d.join("r");
        plugin(&src, "real", "1.0.0");
        let r = sync(&config(&[("other", &src)]), &reg, &d.join("xxh.lock"))
            .await
            .unwrap();
        assert!(matches!(&r.lines[0].outcome, Outcome::Failed(m) if m.contains("`real`")));
        let _ = std::fs::remove_dir_all(&d);
    }
}
