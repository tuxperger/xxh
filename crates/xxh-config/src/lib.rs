//! xxh-config — the single canonical configuration (Принцип XI).
//!
//! The runtime reads only this format (plus CLI-flag overrides). Declarative Nix
//! modules merely *generate* this file (T054–T059); they are not an alternative
//! runtime source. See data-model.md and contracts/nix-config-module.md.

pub mod edit;
pub mod keys;
pub mod schema;
pub mod template;
pub mod validate;

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
    /// The file is not TOML at all, so it cannot be edited in place.
    #[error("the config is not valid TOML:\n{0}")]
    Unparsable(String),
    /// `xxh config validate` found errors (or, strictly, warnings).
    #[error("invalid config `{path}`{details}")]
    Invalid { path: PathBuf, details: String },
    #[error("`{key}` is not a config key")]
    InvalidKey { key: String },
    #[error("unknown config key `{key}`{hint}")]
    UnknownKey { key: String, hint: String },
    #[error("invalid value `{value}` for `{key}`: {expected}")]
    InvalidValue {
        key: String,
        value: String,
        expected: String,
    },
    #[error("`{key}` is not set{default}")]
    NotSet { key: String, default: String },
    /// Something else owns the file (a Nix module, a read-only mount).
    #[error("`{path}` is managed elsewhere ({reason}) — change it at its source")]
    Managed { path: PathBuf, reason: String },
    #[error("`{path}` already exists (use --force to overwrite it)")]
    Exists { path: PathBuf },
    /// A failure with no narrower kind (no config directory, no editor).
    #[error("{0}")]
    Other(String),
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
    /// Personal files for this host, merged over the global `[files]` by name;
    /// `false` drops a global entry here (010 C-F3).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, HostFileEntry>,
    /// Session variables for this host, merged over the global `[env]` by name
    /// (011 C-E3).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

/// The canonical user configuration file (`~/.config/xxh/config.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
    /// Plugins declared with their sources, keyed by plugin name (013 C-L1);
    /// `xxh sync` installs them. Enabling stays with `enabled_plugins`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plugins: BTreeMap<String, Declared>,
    /// Shell packages declared with their sources, keyed by shell name (013).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub shells: BTreeMap<String, Declared>,
    /// Personal files made visible in the session, by the name a program looks
    /// for (`.gitconfig`, `.config/nvim`) (010 C-F1).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, FileEntry>,
    /// Variables set in every session (011 C-E1). Values may be secrets: xxh
    /// never prints them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

/// A plugin or shell package declared in the config (013 C-L1): its source is
/// the same string `xxh plugin add` accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Declared {
    pub source: String,
}

/// One declared personal file or directory (010 C-F1): the path on the client,
/// or a table when it needs more than a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum FileEntry {
    Path(String),
    Detailed(FileSpec),
}

/// A declared file in full.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FileSpec {
    /// Path on the client; `~/` is the client's home directory.
    pub source: String,
    /// Variable that gets the delivered path, for a program without a known one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// Deliver it even though it looks like a secret (Принцип V).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secret: bool,
}

impl FileEntry {
    /// The entry in full, whichever way it was written.
    pub fn spec(&self) -> FileSpec {
        match self {
            FileEntry::Path(source) => FileSpec {
                source: source.clone(),
                ..FileSpec::default()
            },
            FileEntry::Detailed(spec) => spec.clone(),
        }
    }
}

/// A host's say about one file: its own entry, or `false` to go without the
/// global one (010 C-F3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum HostFileEntry {
    /// `false`: not on this host. (`true` changes nothing.)
    Wanted(bool),
    Entry(FileEntry),
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
            plugins: BTreeMap::new(),
            shells: BTreeMap::new(),
            files: BTreeMap::new(),
            env: BTreeMap::new(),
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
    /// `-e/--env`, in the order given: a later one of the same name wins
    /// (011 C-E3). Values only — a bare `-e NAME` is resolved by the CLI.
    pub env: Vec<(String, String)>,
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
    /// Personal files for this target: the global set with the host's entries
    /// merged over it by name (010 C-F3).
    pub files: BTreeMap<String, FileSpec>,
    /// Session variables for this target after precedence (011 C-E3). Values
    /// may be secrets: never print them (§FR-008).
    pub env: BTreeMap<String, String>,
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

        let mut files: BTreeMap<String, FileSpec> = self
            .files
            .iter()
            .map(|(name, entry)| (name.clone(), entry.spec()))
            .collect();
        for (name, entry) in ho.into_iter().flat_map(|h| &h.files) {
            match entry {
                HostFileEntry::Wanted(false) => {
                    files.remove(name);
                }
                HostFileEntry::Wanted(true) => {}
                HostFileEntry::Entry(entry) => {
                    files.insert(name.clone(), entry.spec());
                }
            }
        }

        // Global, then the host by name, then the flags by name (C-E3).
        let mut env = self.env.clone();
        env.extend(ho.into_iter().flat_map(|h| h.env.clone()));
        env.extend(cli.env.iter().cloned());

        Effective {
            shell,
            enabled_plugins,
            cleanup,
            transport,
            connect_timeout_s,
            user,
            identity,
            container_runtime,
            files,
            env,
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

    /// 013 C-L1: declarations parse, round-trip, and are omitted when empty.
    /// 010 C-F1/C-F3: both forms parse; a host's entries merge over the global
    /// set by name and `false` drops one.
    #[test]
    fn files_merge_by_name() {
        let cfg: Config = toml::from_str(
            r#"
[files]
".gitconfig" = "~/.gitconfig"
".config/nvim" = "~/.config/nvim"
".myrc" = { source = "~/.myrc", env = "MY_RC" }

[hosts.web.files]
".gitconfig" = "~/work/gitconfig"
".config/nvim" = false
".netrc" = { source = "~/.netrc", secret = true }
"#,
        )
        .unwrap();
        let spec = |source: &str| FileSpec {
            source: source.into(),
            ..FileSpec::default()
        };
        let global = cfg.resolve("other", &CliOverrides::default()).files;
        assert_eq!(global.len(), 3);
        assert_eq!(global[".gitconfig"], spec("~/.gitconfig"));
        assert_eq!(global[".myrc"].env.as_deref(), Some("MY_RC"));

        let web = cfg.resolve("web", &CliOverrides::default()).files;
        assert_eq!(
            web.keys().collect::<Vec<_>>(),
            [".gitconfig", ".myrc", ".netrc"]
        );
        assert_eq!(web[".gitconfig"], spec("~/work/gitconfig"));
        assert!(web[".netrc"].secret);

        // The file round-trips, short forms staying short.
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert!(text.contains("\".gitconfig\" = \"~/.gitconfig\""), "{text}");
        assert!(text.contains("\".config/nvim\" = false"), "{text}");
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), cfg);
        // No files: nothing about them is written.
        let text = toml::to_string_pretty(&Config::default()).unwrap();
        assert!(!text.contains("files"), "{text}");
    }

    /// 011 C-E1/C-E3: global, then the host by name, then the flags by name.
    #[test]
    fn env_layers_merge_by_name() {
        let cfg: Config = toml::from_str(
            r#"
[env]
EDITOR = "nvim"
LANG = "C.UTF-8"

[hosts.web.env]
EDITOR = "vi"
EXTRA = "multi\nline"
"#,
        )
        .unwrap();
        let other = cfg.resolve("other", &CliOverrides::default()).env;
        assert_eq!(other.len(), 2);
        assert_eq!(other["EDITOR"], "nvim");

        let web = cfg.resolve("web", &CliOverrides::default()).env;
        assert_eq!(
            (web["EDITOR"].as_str(), web["LANG"].as_str()),
            ("vi", "C.UTF-8")
        );
        assert_eq!(web["EXTRA"], "multi\nline");

        let cli = CliOverrides {
            env: vec![("EDITOR".into(), "nano".into()), ("NEW".into(), "".into())],
            ..CliOverrides::default()
        };
        let flagged = cfg.resolve("web", &cli).env;
        assert_eq!(flagged["EDITOR"], "nano", "the flag beats the host");
        assert_eq!(flagged["NEW"], "");
        assert_eq!(flagged.len(), 4);

        let text = toml::to_string_pretty(&cfg).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), cfg);
        let text = toml::to_string_pretty(&Config::default()).unwrap();
        assert!(!text.contains("env"), "{text}");
    }

    #[test]
    fn plugins_and_shells_are_declared_with_sources() {
        let c: Config = toml::from_str(
            r#"
            enabled_plugins = ["neovim"]
            [plugins.neovim]
            source = "git@github.com:me/xxh-plugin-neovim.git"
            [plugins.nix-htop]
            source = "nixpkgs:htop"
            [shells.zsh]
            source = "https://example.org/xxh-shell-zsh.git#v1"
            "#,
        )
        .unwrap();
        assert_eq!(c.plugins.len(), 2);
        assert_eq!(c.plugins["nix-htop"].source, "nixpkgs:htop");
        assert_eq!(
            c.shells["zsh"].source,
            "https://example.org/xxh-shell-zsh.git#v1"
        );
        let back: Config = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.plugins, c.plugins);
        assert_eq!(back.shells, c.shells);
        let plain = toml::to_string(&Config::default()).unwrap();
        assert!(
            !plain.contains("[plugins") && !plain.contains("[shells"),
            "{plain}"
        );
    }

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
