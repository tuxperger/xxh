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
        let mut cmd = tokio::process::Command::new("git");
        cmd.args(["clone", "--depth", "1", "--quiet"]);
        if let Some(r) = reference {
            cmd.args(["--branch", r]);
        }
        cmd.arg(url).arg(&dest);
        let out = cmd
            .output()
            .await
            .map_err(|e| PluginError::Other(format!("spawning git: {e}")))?;
        if !out.status.success() {
            let _ = std::fs::remove_dir_all(&dest);
            return Err(PluginError::Other(format!(
                "git clone of `{url}` failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
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
        })
    }
}

fn tmp_clone_dir() -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("xxh-git-{}-{n:x}", std::process::id()))
}
