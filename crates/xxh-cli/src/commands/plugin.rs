//! `xxh plugin …` — add/remove/enable/disable/update/list (T038, §FR-015).
//!
//! Installed state lives in the registry (`~/.local/share/xxh/plugins`);
//! *enabled* state lives in the canonical config file (Принцип XI). This module
//! also assembles the enabled plugins for a session in resolved order (T039).

use clap::Subcommand;
use clap_complete::engine::ArgValueCompleter;
use xxh_config::{Config, ConfigError, Effective, edit};

use crate::complete;
use xxh_core::session::SessionPlugin;
use xxh_plugins::registry::Registry;
use xxh_plugins::source::SourceSpec;
use xxh_plugins::{PluginError, resolver};

#[derive(Subcommand)]
pub enum PluginAction {
    /// Install a plugin from a git URL, a local path, `nixpkgs:<attr>`, or a flake
    /// output `flake:<ref>[#<attr>]` (the latter two need Nix on this client).
    Add {
        source: String,
        /// Plugin name for a program taken from a flake (default: derived from
        /// the flake reference).
        #[arg(long)]
        name: Option<String>,
    },
    /// Remove an installed plugin (and disable it).
    Remove {
        #[arg(add = ArgValueCompleter::new(complete::installed_plugins))]
        name: String,
    },
    /// Enable an installed plugin in the config.
    Enable {
        #[arg(add = ArgValueCompleter::new(complete::disabled_plugins))]
        name: String,
    },
    /// Disable a plugin in the config (keeps it installed).
    Disable {
        #[arg(add = ArgValueCompleter::new(complete::enabled_plugins))]
        name: String,
    },
    /// Re-fetch a plugin from its recorded source.
    Update {
        #[arg(add = ArgValueCompleter::new(complete::installed_plugins))]
        name: String,
    },
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

/// Change the lock file, if there is one and it can be written; a lock that
/// is a symlink is managed elsewhere (Nix) and only gets a note (013 C-L8).
fn edit_lock(f: impl FnOnce(&mut xxh_plugins::lock::Lock)) {
    let Some(path) = Config::default_path().map(|c| xxh_plugins::lock::default_path(&c)) else {
        return;
    };
    let Ok(mut lock) = xxh_plugins::lock::Lock::load(&path) else {
        return;
    };
    let before = lock.clone();
    f(&mut lock);
    if lock == before {
        return;
    }
    let managed = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink());
    if managed || lock.save(&path).is_err() {
        eprintln!(
            "xxh: note: the lock file {} cannot be written here; update it to:\n{}",
            path.display(),
            lock.render()
        );
    }
}

/// `name: 1.0.0 (abc123def456) → 1.1.0 (0123456789ab)` (013 C-L10).
fn update_line(name: &str, from: (&str, Option<&str>), to: (&str, Option<&str>)) -> String {
    let side = |(v, r): (&str, Option<&str>)| match r {
        Some(r) => format!("{v} ({})", r.chars().take(12).collect::<String>()),
        None => v.to_string(),
    };
    format!("{name}: {} → {}", side(from), side(to))
}

/// Change `enabled_plugins` in the config file. Only that list is rewritten —
/// comments and every other line stay (009 C-G19) — and a config managed
/// elsewhere is refused with the reason (C-G15) when there is a change to make.
fn edit_config(f: impl FnOnce(&mut Config)) -> Result<(), PluginCmdError> {
    let path = config_path()?;
    let mut cfg = Config::load(&path)?;
    let before = cfg.enabled_plugins.clone();
    f(&mut cfg);
    if cfg.enabled_plugins == before {
        return Ok(());
    }
    edit::ensure_writable(&path)?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => return Err(ConfigError::Io { path, source }.into()),
    };
    let text = edit::set_list(&text, "enabled_plugins", &cfg.enabled_plugins)?;
    edit::write_atomic(&path, &text)?;
    Ok(())
}

pub async fn run(action: &PluginAction) -> Result<(), PluginCmdError> {
    let registry = Registry::open_default()?;
    match action {
        PluginAction::Add { source, name } => {
            let mut spec = SourceSpec::parse(source)?;
            if let Some(chosen) = name {
                match &mut spec {
                    SourceSpec::Flake { name, .. } => *name = Some(chosen.clone()),
                    // Every other source carries its name in its own manifest (C-FC3).
                    _ => {
                        return Err(PluginError::Other(
                            "--name applies only to `flake:` sources; other plugins are \
                             named by their plugin.toml"
                                .into(),
                        )
                        .into());
                    }
                }
            }
            let m = registry.install(&spec).await?;
            // The registry records what was actually installed (incl. the pin).
            let installed = registry.entry(&m.name)?.source;
            println!(
                "installed {} {} (from {})",
                m.name,
                m.version,
                installed.label()
            );
            if matches!(installed, SourceSpec::Flake { revision: None, .. }) {
                eprintln!(
                    "xxh: warning: flake source has no fixed revision (local or dirty \
                     tree); this install is not reproducible"
                );
            }
            println!("enable it with: xxh plugin enable {}", m.name);
        }
        PluginAction::Remove { name } => {
            registry.remove(name)?;
            // A removed plugin is no longer pinned (013 C-L10).
            edit_lock(|l| {
                l.plugins.remove(name);
            });
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
            let before = registry.entry(name)?;
            let got = registry
                .install_pinned(&before.source.unpinned(), None, None)
                .await?;
            let m = got.manifest.clone();
            let after = registry.entry(&m.name)?;
            // A locked plugin moves its lock entry along and says from what to
            // what (013 C-L10, §FR-007).
            let mut moved = None;
            edit_lock(|l| {
                if let Some(e) = l.plugins.get_mut(name) {
                    moved = Some(update_line(
                        name,
                        (&before.version, e.revision.as_deref()),
                        (&after.version, got.revision.as_deref()),
                    ));
                    e.revision = got.revision.clone();
                    e.hash = got.hash.clone();
                }
            });
            if let Some(line) = moved {
                println!("{line}");
            } else if after.source.is_flake() {
                // Pinned sources say which revision they moved from and to (C-FC5).
                let (old, new) = (
                    before.source.short_revision(),
                    after.source.short_revision(),
                );
                if old == new && before.hash == after.hash {
                    println!("{} is up to date ({new})", m.name);
                } else {
                    println!("updated {} to {} ({old} -> {new})", m.name, m.version);
                }
            } else {
                println!("updated {} to {}", m.name, m.version);
            }
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
                    entry.source.label()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_lines_show_versions_and_revisions() {
        assert_eq!(
            update_line(
                "p",
                ("1.0.0", Some("abcdef0123456789")),
                ("1.1.0", Some("0123"))
            ),
            "p: 1.0.0 (abcdef012345) → 1.1.0 (0123)"
        );
        assert_eq!(
            update_line("q", ("1.0.0", None), ("1.0.0", None)),
            "q: 1.0.0 → 1.0.0"
        );
    }
}
