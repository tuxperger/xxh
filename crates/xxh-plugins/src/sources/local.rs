//! LocalProvider — a plugin from a local directory (T034, §FR-016, C-S1).

use std::collections::BTreeMap;

use xxh_plugin_api::PluginError;

use crate::source::{Availability, FetchedPackage, PackageSource, SourceSpec, Support};

pub struct LocalProvider;

#[async_trait::async_trait]
impl PackageSource for LocalProvider {
    fn id(&self) -> &'static str {
        "local"
    }

    fn availability(&self) -> Availability {
        Availability::Available
    }

    fn supports_target(&self, _os: &str, _arch: &str, _libc: &str) -> Support {
        Support::Supported
    }

    async fn fetch(&self, spec: &SourceSpec) -> Result<FetchedPackage, PluginError> {
        let SourceSpec::Local { path } = spec else {
            return Err(PluginError::Other(
                "local provider got a non-local spec".into(),
            ));
        };
        let dir = path
            .canonicalize()
            .map_err(|e| PluginError::Other(format!("{}: {e}", path.display())))?;
        let manifest = crate::source::read_manifest(&dir)?;
        Ok(FetchedPackage {
            manifest,
            dir,
            env: BTreeMap::new(),
            cleanup: None, // the user's directory is not ours to delete
        })
    }
}
