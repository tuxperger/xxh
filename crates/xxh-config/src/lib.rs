//! xxh-config — the single canonical configuration (Принцип XI).
//!
//! The runtime reads only this format (plus CLI-flag overrides). Declarative Nix
//! modules merely *generate* this file (T054–T059); they are not an alternative
//! runtime source. See data-model.md and contracts/nix-config-module.md.

pub mod schema;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Error class for configuration problems. Maps to CLI exit code 40 (§FR-026).
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config `{path}`: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config `{path}`: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

/// Cleanup behaviour on session exit (§FR-005/012, Принцип I).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CleanupMode {
    /// Default: remove everything on exit — the host is left as before.
    Ephemeral,
    /// Keep the content-addressed cache between sessions (requires the `--keep` flag).
    Keep,
}

impl Default for CleanupMode {
    fn default() -> Self {
        Self::Ephemeral
    }
}

/// Which transport backend to use (Принцип III).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TransportBackend {
    /// Pure-Rust russh backend (default).
    Russh,
    /// Wrapper over the system `ssh` binary (fallback/compat).
    Ssh,
}

impl Default for TransportBackend {
    fn default() -> Self {
        Self::Russh
    }
}

/// Which container runtime drives `container:`-family targets, and the order for
/// `auto` (002-container-targets, data-model). Distinct from [`TransportBackend`]:
/// that selects the SSH client, this selects the container runtime (C-A5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeSetting {
    /// Probe docker, then podman; take the first available (C-A3).
    Auto,
    Docker,
    Podman,
}

impl Default for RuntimeSetting {
    fn default() -> Self {
        Self::Auto
    }
}

/// The `[container]` config section (002-container-targets, Принцип XI).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ContainerConfig {
    /// Runtime for `container:` targets and the `auto` probe order (default `auto`).
    #[serde(default)]
    pub runtime: RuntimeSetting,
}

fn default_shell() -> String {
    "zsh".to_string()
}

fn default_timeout() -> u64 {
    10
}

/// Per-host overrides applied on top of the global config (§FR-023).
///
/// Every field is optional; `None` means "inherit the global value". List-valued
/// fields (`enabled_plugins`) **replace** the global list rather than merging
/// (resolves analysis finding C4 — simple, predictable precedence).
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HostOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_shell: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_plugins: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<CleanupMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportBackend>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout_s: Option<u64>,
    /// Login user for this host (like ssh `-l`; отсутствие → ssh-config/текущий).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Private key (identity file) for this host (like ssh `-i`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<PathBuf>,
    /// Container runtime for this target (per-target layer of C-A3 precedence);
    /// only meaningful for `container:` targets, ignored for SSH.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_runtime: Option<RuntimeSetting>,
}

/// The canonical user configuration file (`~/.config/xxh/config.toml`).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Config {
    #[serde(default = "default_shell")]
    pub default_shell: String,
    #[serde(default)]
    pub enabled_plugins: Vec<String>,
    #[serde(default)]
    pub cleanup: CleanupMode,
    #[serde(default)]
    pub transport: TransportBackend,
    #[serde(default = "default_timeout")]
    pub connect_timeout_s: u64,
    /// Login user for all hosts unless overridden (§FR-029: ssh-config remains
    /// the fallback when unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Private key (identity file) used for all hosts unless overridden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<PathBuf>,
    /// Container-family settings (runtime selection) — Принцип XI, one config.
    #[serde(default)]
    pub container: ContainerConfig,
    #[serde(default)]
    pub hosts: BTreeMap<String, HostOverride>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_shell: default_shell(),
            enabled_plugins: Vec::new(),
            cleanup: CleanupMode::default(),
            transport: TransportBackend::default(),
            connect_timeout_s: default_timeout(),
            user: None,
            identity: None,
            container: ContainerConfig::default(),
            hosts: BTreeMap::new(),
        }
    }
}

/// CLI-flag overrides for a single run — the highest-precedence layer (§FR-024).
#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    pub shell: Option<String>,
    pub cleanup: Option<CleanupMode>,
    pub transport: Option<TransportBackend>,
    pub connect_timeout_s: Option<u64>,
    /// `-l/--user` or the `user@host` prefix.
    pub user: Option<String>,
    /// `-i/--identity`.
    pub identity: Option<PathBuf>,
    /// `--runtime` (container family only); highest layer of C-A3 precedence.
    pub container_runtime: Option<RuntimeSetting>,
}

/// The effective settings for one connection, after applying precedence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effective {
    pub shell: String,
    pub enabled_plugins: Vec<String>,
    pub cleanup: CleanupMode,
    pub transport: TransportBackend,
    pub connect_timeout_s: u64,
    /// Login user; `None` → ssh-config / current user decide (§FR-029).
    pub user: Option<String>,
    /// Explicit private key; `None` → ssh-config / default identities.
    pub identity: Option<PathBuf>,
    /// Resolved container runtime after precedence (C-A3). Only consulted for
    /// `container:` targets; explicit `docker:`/`podman:` schemes override it.
    pub container_runtime: RuntimeSetting,
}

impl Config {
    /// Default canonical config path (`$XDG_CONFIG_HOME/xxh/config.toml`).
    pub fn default_path() -> Option<PathBuf> {
        directories::BaseDirs::new().map(|d| d.config_dir().join("xxh").join("config.toml"))
    }

    /// System-wide config location, written by the NixOS module (§FR-044).
    pub fn system_path() -> PathBuf {
        PathBuf::from("/etc/xxh/config.toml")
    }

    /// Load the effective config file: the per-user file wins; a NixOS-managed
    /// system-wide file is the fallback; with neither, built-in defaults apply.
    pub fn load_default() -> Result<Self, ConfigError> {
        if let Some(user) = Self::default_path() {
            if user.is_file() {
                return Self::load(&user);
            }
        }
        let system = Self::system_path();
        if system.is_file() {
            return Self::load(&system);
        }
        Ok(Self::default())
    }

    /// Load from `path`. A missing file yields the default config (§FR-022 —
    /// the tool works with no config). Invalid TOML is a `ConfigError::Parse`
    /// (exit 40), never a runtime panic.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|source| ConfigError::Parse {
                path: path.to_path_buf(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Persist the config back to `path` (used by `xxh plugin enable/disable`,
    /// §FR-015: enabled-state lives in the canonical config, Принцип XI).
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        }
        let text = toml::to_string_pretty(self).expect("Config always serializes");
        std::fs::write(path, text).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Resolve effective settings for `alias`, applying precedence:
    /// CLI flag > per-host override > global config > built-in default (§FR-024).
    pub fn resolve(&self, alias: &str, cli: &CliOverrides) -> Effective {
        let ho = self.hosts.get(alias);

        let shell = cli
            .shell
            .clone()
            .or_else(|| ho.and_then(|h| h.default_shell.clone()))
            .unwrap_or_else(|| self.default_shell.clone());

        let enabled_plugins = ho
            .and_then(|h| h.enabled_plugins.clone())
            .unwrap_or_else(|| self.enabled_plugins.clone());

        let cleanup = cli
            .cleanup
            .or_else(|| ho.and_then(|h| h.cleanup))
            .unwrap_or(self.cleanup);

        let transport = cli
            .transport
            .or_else(|| ho.and_then(|h| h.transport))
            .unwrap_or(self.transport);

        let connect_timeout_s = cli
            .connect_timeout_s
            .or_else(|| ho.and_then(|h| h.connect_timeout_s))
            .unwrap_or(self.connect_timeout_s);

        let user = cli
            .user
            .clone()
            .or_else(|| ho.and_then(|h| h.user.clone()))
            .or_else(|| self.user.clone());

        let identity = cli
            .identity
            .clone()
            .or_else(|| ho.and_then(|h| h.identity.clone()))
            .or_else(|| self.identity.clone())
            .map(expand_tilde);

        // Runtime precedence (C-A3): `--runtime` > per-target > `container.runtime`
        // > default (`auto`). The explicit `docker:`/`podman:` scheme is applied
        // later, at the call site, and is never silently overridden (C-A4).
        let container_runtime = cli
            .container_runtime
            .or_else(|| ho.and_then(|h| h.container_runtime))
            .unwrap_or(self.container.runtime);

        Effective {
            shell,
            enabled_plugins,
            cleanup,
            transport,
            connect_timeout_s,
            user,
            identity,
            container_runtime,
        }
    }
}

/// Expand a leading `~/` to the user's home directory: config-file values are
/// not shell-expanded, and both transports need a real path (ssh `-i` parity).
fn expand_tilde(p: PathBuf) -> PathBuf {
    if let Ok(rest) = p.strip_prefix("~") {
        if let Some(bd) = directories::BaseDirs::new() {
            return bd.home_dir().join(rest);
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_applied_for_empty_config() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.default_shell, "zsh");
        assert_eq!(c.transport, TransportBackend::Russh);
        assert_eq!(c.cleanup, CleanupMode::Ephemeral);
        assert_eq!(c.connect_timeout_s, 10);
    }

    #[test]
    fn precedence_flag_beats_host_beats_global() {
        let cfg: Config = toml::from_str(
            r#"
            default_shell = "bash"
            connect_timeout_s = 20
            [hosts.web]
            default_shell = "fish"
            "#,
        )
        .unwrap();

        // Global only.
        let e = cfg.resolve("other", &CliOverrides::default());
        assert_eq!(e.shell, "bash");
        assert_eq!(e.connect_timeout_s, 20);

        // Per-host override wins over global.
        let e = cfg.resolve("web", &CliOverrides::default());
        assert_eq!(e.shell, "fish");

        // CLI flag wins over everything.
        let cli = CliOverrides {
            shell: Some("zsh".into()),
            ..Default::default()
        };
        let e = cfg.resolve("web", &cli);
        assert_eq!(e.shell, "zsh");
    }

    #[test]
    fn user_and_identity_follow_precedence() {
        let cfg: Config = toml::from_str(
            r#"
            user = "deploy"
            identity = "/keys/global"
            [hosts.web]
            user = "www"
            identity = "/keys/web"
            "#,
        )
        .unwrap();

        // Global values apply to hosts without overrides.
        let e = cfg.resolve("other", &CliOverrides::default());
        assert_eq!(e.user.as_deref(), Some("deploy"));
        assert_eq!(e.identity.as_deref(), Some(Path::new("/keys/global")));

        // Per-host override wins over global.
        let e = cfg.resolve("web", &CliOverrides::default());
        assert_eq!(e.user.as_deref(), Some("www"));
        assert_eq!(e.identity.as_deref(), Some(Path::new("/keys/web")));

        // CLI flag (-l / -i / user@host) wins over everything.
        let cli = CliOverrides {
            user: Some("root".into()),
            identity: Some(PathBuf::from("/keys/cli")),
            ..Default::default()
        };
        let e = cfg.resolve("web", &cli);
        assert_eq!(e.user.as_deref(), Some("root"));
        assert_eq!(e.identity.as_deref(), Some(Path::new("/keys/cli")));

        // Unset everywhere → None (ssh-config decides, §FR-029).
        let bare = Config::default().resolve("web", &CliOverrides::default());
        assert_eq!(bare.user, None);
        assert_eq!(bare.identity, None);
    }

    #[test]
    fn identity_tilde_expands_to_home() {
        let cfg: Config = toml::from_str(r#"identity = "~/.ssh/key""#).unwrap();
        let e = cfg.resolve("any", &CliOverrides::default());
        let id = e.identity.expect("identity set");
        assert!(id.is_absolute(), "expanded path must be absolute: {id:?}");
        assert!(id.ends_with(".ssh/key"));
    }

    #[test]
    fn container_runtime_follows_precedence() {
        // Default with no config → auto.
        assert_eq!(
            Config::default()
                .resolve("any", &CliOverrides::default())
                .container_runtime,
            RuntimeSetting::Auto
        );

        let cfg: Config = toml::from_str(
            r#"
            [container]
            runtime = "docker"
            [hosts.app1]
            container_runtime = "podman"
            "#,
        )
        .unwrap();

        // Global `container.runtime` applies to targets without an override.
        assert_eq!(
            cfg.resolve("other", &CliOverrides::default())
                .container_runtime,
            RuntimeSetting::Docker
        );
        // Per-target override wins over global.
        assert_eq!(
            cfg.resolve("app1", &CliOverrides::default())
                .container_runtime,
            RuntimeSetting::Podman
        );
        // `--runtime` flag wins over everything.
        let cli = CliOverrides {
            container_runtime: Some(RuntimeSetting::Docker),
            ..Default::default()
        };
        assert_eq!(
            cfg.resolve("app1", &cli).container_runtime,
            RuntimeSetting::Docker
        );
    }

    #[test]
    fn per_host_plugins_replace_global_list() {
        let cfg: Config = toml::from_str(
            r#"
            enabled_plugins = ["a", "b"]
            [hosts.web]
            enabled_plugins = ["c"]
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg.resolve("other", &CliOverrides::default())
                .enabled_plugins,
            vec!["a", "b"]
        );
        assert_eq!(
            cfg.resolve("web", &CliOverrides::default()).enabled_plugins,
            vec!["c"]
        );
    }
}
