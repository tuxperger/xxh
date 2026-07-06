//! `xxh plugin …` — add/remove/enable/disable/update/list (T038, §FR-015).
//!
//! Installed state lives in the registry (`~/.local/share/xxh/plugins`);
//! *enabled* state lives in the canonical config file (Принцип XI). This module
//! also assembles the enabled plugins for a session in resolved order (T039).

use clap::Subcommand;
use xxh_config::{Config, ConfigError, Effective};
use xxh_core::session::SessionPlugin;
use xxh_plugins::registry::Registry;
use xxh_plugins::source::SourceSpec;
use xxh_plugins::{PluginError, resolver};

#[derive(Subcommand)]
pub enum PluginAction {
    /// Install a plugin from a git URL, a local path, or `nixpkgs:<attr>`.
    Add { source: String },
    /// Remove an installed plugin (and disable it).
    Remove { name: String },
    /// Enable an installed plugin in the config.
    Enable { name: String },
    /// Disable a plugin in the config (keeps it installed).
    Disable { name: String },
    /// Re-fetch a plugin from its recorded source.
    Update { name: String },
    /// List installed plugins.
    List {
        /// Show only enabled plugins.
        #[arg(long)]
        enabled: bool,
    },
}

/// Plugin-command failures keep their error class: registry/source problems are
/// plugin-class (exit 30), config read/write problems are config-class (exit 40).
#[derive(Debug, thiserror::Error)]
pub enum PluginCmdError {
    #[error(transparent)]
    Plugin(#[from] PluginError),
    #[error(transparent)]
    Config(#[from] ConfigError),
}

fn config_path() -> Result<std::path::PathBuf, PluginError> {
    Config::default_path()
        .ok_or_else(|| PluginError::Other("cannot determine config directory".into()))
}

fn edit_config(f: impl FnOnce(&mut Config)) -> Result<(), PluginCmdError> {
    let path = config_path()?;
    let mut cfg = Config::load(&path)?;
    f(&mut cfg);
    cfg.save(&path)?;
    Ok(())
}

pub async fn run(action: &PluginAction) -> Result<(), PluginCmdError> {
    let registry = Registry::open_default()?;
    match action {
        PluginAction::Add { source } => {
            let spec = SourceSpec::parse(source)?;
            let m = registry.install(&spec).await?;
            println!(
                "installed {} {} (from {})",
                m.name,
                m.version,
                spec.describe()
            );
            println!("enable it with: xxh plugin enable {}", m.name);
        }
        PluginAction::Remove { name } => {
            registry.remove(name)?;
            edit_config(|c| c.enabled_plugins.retain(|p| p != name))?;
            println!("removed {name}");
        }
        PluginAction::Enable { name } => {
            // Must be installed and manifest-valid before it can be enabled.
            registry.manifest(name)?;
            edit_config(|c| {
                if !c.enabled_plugins.iter().any(|p| p == name) {
                    c.enabled_plugins.push(name.clone());
                }
            })?;
            println!("enabled {name}");
        }
        PluginAction::Disable { name } => {
            edit_config(|c| c.enabled_plugins.retain(|p| p != name))?;
            println!("disabled {name}");
        }
        PluginAction::Update { name } => {
            let m = registry.update(name).await?;
            println!("updated {} to {}", m.name, m.version);
        }
        PluginAction::List { enabled } => {
            let enabled_set = crate::commands::config::load()
                .map(|c| c.enabled_plugins)
                .unwrap_or_default();
            for (name, entry) in registry.list()? {
                let is_enabled = enabled_set.iter().any(|p| p == &name);
                if *enabled && !is_enabled {
                    continue;
                }
                let mark = if is_enabled { "enabled " } else { "disabled" };
                println!(
                    "{mark}  {name} {} ({})",
                    entry.version,
                    entry.source.describe()
                );
            }
        }
    }
    Ok(())
}

/// Assemble the enabled plugins for a session: load their manifests from the
/// registry and resolve a deterministic load order — conflicts, missing
/// dependencies and cycles fail *before* any deployment (§FR-018/021).
pub fn session_plugins(eff: &Effective) -> Result<Vec<SessionPlugin>, PluginError> {
    if eff.enabled_plugins.is_empty() {
        return Ok(Vec::new());
    }
    let registry = Registry::open_default()?;
    let mut plugins = Vec::with_capacity(eff.enabled_plugins.len());
    for name in &eff.enabled_plugins {
        plugins.push(SessionPlugin {
            manifest: registry.manifest(name)?,
            dir: registry.package_dir(name)?,
        });
    }
    let manifests: Vec<_> = plugins.iter().map(|p| p.manifest.clone()).collect();
    let order = resolver::resolve(&manifests)?;
    plugins.sort_by_key(|p| {
        order
            .iter()
            .position(|n| *n == p.manifest.name)
            .unwrap_or(usize::MAX)
    });
    Ok(plugins)
}
