//! GitProvider — fetch a plugin from a git repository (T033, §FR-016, C-S1).

use std::collections::BTreeMap;
use std::path::PathBuf;

use xxh_plugin_api::PluginError;

use crate::source::{Availability, FetchedPackage, PackageSource, SourceSpec, Support};

pub struct GitProvider;

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[async_trait::async_trait]
impl PackageSource for GitProvider {
    fn id(&self) -> &'static str {
        "git"
    }

    fn availability(&self) -> Availability {
        if git_available() {
            Availability::Available
        } else {
            Availability::Unavailable {
                reason: "`git` was not found on this client".into(),
            }
        }
    }

    fn supports_target(&self, _os: &str, _arch: &str, _libc: &str) -> Support {
        Support::Supported // plugin content is data; target filtering is per-manifest
    }

    async fn fetch(&self, spec: &SourceSpec) -> Result<FetchedPackage, PluginError> {
        let SourceSpec::Git { url, reference } = spec else {
            return Err(PluginError::Other("git provider got a non-git spec".into()));
        };
        if let Availability::Unavailable { reason } = self.availability() {
            return Err(PluginError::SourceUnavailable(reason));
        }

        let dest = tmp_clone_dir();
        // A full commit id is a pin (013 lock file): `--branch` cannot take one,
        // so clone and check it out. Anything else is a branch or tag.
        let pin = reference.as_deref().filter(|r| is_commit(r));
        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("clone").arg("--quiet");
        match (reference, pin) {
            (_, Some(_)) => {}
            (Some(r), None) => {
                cmd.args(["--depth", "1", "--branch", r]);
            }
            (None, _) => {
                cmd.args(["--depth", "1"]);
            }
        }
        cmd.arg(url).arg(&dest);
        run_git(cmd, url, &dest).await?;
        if let Some(sha) = pin {
            let mut co = tokio::process::Command::new("git");
            co.arg("-C").arg(&dest).args(["checkout", "--quiet", sha]);
            run_git(co, url, &dest).await?;
        }
        let revision = {
            let mut rp = tokio::process::Command::new("git");
            rp.arg("-C").arg(&dest).args(["rev-parse", "HEAD"]);
            let out = run_git(rp, url, &dest).await?;
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        // The clone metadata is not part of the package content.
        let _ = std::fs::remove_dir_all(dest.join(".git"));

        let manifest = crate::source::read_manifest(&dest).inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&dest);
        })?;
        Ok(FetchedPackage {
            manifest,
            dir: dest.clone(),
            env: BTreeMap::new(),
            cleanup: Some(dest),
            resolved: None,
            revision: Some(revision),
        })
    }
}

/// A full 40-hex commit id.
fn is_commit(r: &str) -> bool {
    r.len() == 40 && r.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Run a git step; on failure drop the half-made clone and say why.
async fn run_git(
    mut cmd: tokio::process::Command,
    url: &str,
    dest: &std::path::Path,
) -> Result<std::process::Output, PluginError> {
    let out = cmd
        .output()
        .await
        .map_err(|e| PluginError::Other(format!("spawning git: {e}")))?;
    if !out.status.success() {
        let _ = std::fs::remove_dir_all(dest);
        return Err(PluginError::Other(format!(
            "git of `{url}` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out)
}

fn tmp_clone_dir() -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("xxh-git-{}-{n:x}", std::process::id()))
}
#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &std::path::Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit(repo: &std::path::Path, version: &str) -> String {
        std::fs::write(
            repo.join("plugin.toml"),
            format!("name = \"g\"\nversion = \"{version}\"\napi_version = \"1.0.0\"\n"),
        )
        .unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-q", "-m", version]);
        git(repo, &["rev-parse", "HEAD"])
    }

    /// The fetch reports its commit, and a full commit id pins it even after
    /// the branch moved on (013 T004, research R3).
    #[tokio::test]
    async fn revision_is_reported_and_commits_pin() {
        if !git_available() {
            eprintln!("skipping: no git");
            return;
        }
        let repo = std::env::temp_dir().join(format!("xxh-git-pin-{}", std::process::id()));
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        let first = commit(&repo, "1.0.0");
        let second = commit(&repo, "2.0.0");
        let url = format!("file://{}", repo.display());

        let head = GitProvider
            .fetch(&SourceSpec::Git {
                url: url.clone(),
                reference: None,
            })
            .await
            .unwrap();
        assert_eq!(head.revision.as_deref(), Some(second.as_str()));
        assert_eq!(head.manifest.version.to_string(), "2.0.0");
        assert!(!head.dir.join(".git").exists());

        let pinned = GitProvider
            .fetch(&SourceSpec::Git {
                url,
                reference: Some(first.clone()),
            })
            .await
            .unwrap();
        assert_eq!(pinned.revision.as_deref(), Some(first.as_str()));
        assert_eq!(pinned.manifest.version.to_string(), "1.0.0");
        for d in [&head.dir, &pinned.dir, &repo] {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}
