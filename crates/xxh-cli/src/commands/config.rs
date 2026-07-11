//! `xxh config path|show [--host]` (T047, contracts/cli-commands.md C-C1).
//!
//! `show` prints the *effective* configuration after precedence
//! (flag > per-host > global > default). The config carries no secrets by
//! design, so nothing needs masking here (Принцип V).

use clap::Subcommand;
use xxh_config::{CliOverrides, Config, ConfigError};

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Print the canonical config file path.
    Path,
    /// Show the effective configuration (with precedence applied).
    Show {
        /// Resolve as for this host alias.
        #[arg(long)]
        host: Option<String>,
    },
}

/// Load the canonical config; a missing file is the built-in default (§FR-022).
/// The per-user file wins over a NixOS-managed system-wide one (§FR-044).
pub fn load() -> Result<Config, ConfigError> {
    Config::load_default()
}

pub fn run(action: &ConfigAction, cli: &CliOverrides) -> Result<(), ConfigError> {
    match action {
        ConfigAction::Path => {
            match Config::default_path() {
                Some(p) => println!("{}", p.display()),
                None => println!("<cannot determine config directory>"),
            }
            Ok(())
        }
        ConfigAction::Show { host } => {
            let cfg = load()?;
            let alias = host.as_deref().unwrap_or("<global>");
            let eff = cfg.resolve(alias, cli);
            println!("host              = {alias}");
            println!("shell             = {}", eff.shell);
            println!("transport         = {:?}", eff.transport);
            println!("container_runtime = {:?}", eff.container_runtime);
            println!("cleanup           = {:?}", eff.cleanup);
            println!("connect_timeout_s = {}", eff.connect_timeout_s);
            println!(
                "user              = {}",
                eff.user.as_deref().unwrap_or("<ssh-config>")
            );
            println!(
                "identity          = {}",
                eff.identity
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<ssh-config>".into())
            );
            println!("enabled_plugins   = {:?}", eff.enabled_plugins);
            Ok(())
        }
    }
}
