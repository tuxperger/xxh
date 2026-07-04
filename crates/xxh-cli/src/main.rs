//! xxh — CLI entry point.
//!
//! Implements the error-class → exit-code taxonomy (T005) and command dispatch.
//! Full session execution lands in T019 (connect); plugin management in T038.
//! See contracts/cli-commands.md.

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use xxh_config::{CliOverrides, CleanupMode, Config, ConfigError, Effective, TransportBackend};
use xxh_core::session::{Session, SessionError};
use xxh_core::{ShellError, Verbosity};
use xxh_plugins::PluginError;
use xxh_transport::{ResolvedSshTarget, RusshTransport, SshCliTransport, Transport, TransportError};

/// Exit-code taxonomy so error classes are distinguishable (§FR-026).
mod exit {
    pub const OK: u8 = 0;
    pub const TRANSPORT: u8 = 10;
    pub const SHELL: u8 = 20;
    pub const PLUGIN: u8 = 30;
    pub const CONFIG: u8 = 40;
    pub const USAGE: u8 = 2;
}

/// Map each error class to its exit code (T005, §FR-026). Kept in one place so the
/// four classes stay distinguishable as error paths are wired into `connect`/`plugin`.
trait ClassifiedError {
    fn exit_code(&self) -> u8;
}
impl ClassifiedError for TransportError {
    fn exit_code(&self) -> u8 {
        exit::TRANSPORT
    }
}
impl ClassifiedError for ShellError {
    fn exit_code(&self) -> u8 {
        exit::SHELL
    }
}
impl ClassifiedError for PluginError {
    fn exit_code(&self) -> u8 {
        exit::PLUGIN
    }
}
impl ClassifiedError for ConfigError {
    fn exit_code(&self) -> u8 {
        exit::CONFIG
    }
}
impl ClassifiedError for SessionError {
    fn exit_code(&self) -> u8 {
        match self {
            SessionError::Transport(_) => exit::TRANSPORT,
            SessionError::Shell(_) => exit::SHELL,
        }
    }
}

/// Report a classified error to the user and return its exit code.
fn report<E: std::fmt::Display + ClassifiedError>(err: &E) -> u8 {
    eprintln!("xxh: {err}");
    err.exit_code()
}

#[derive(Parser)]
#[command(name = "xxh", version, about = "Portable shell environment over SSH")]
struct Cli {
    /// Host to connect to (compatible with ~/.ssh/config), when no subcommand is given.
    host: Option<String>,

    /// Shell to use for this session (overrides config).
    #[arg(long, global = true)]
    shell: Option<String>,

    /// Keep the environment on the host between sessions.
    #[arg(long, global = true)]
    keep: bool,

    /// Transport backend.
    #[arg(long, global = true, value_parser = ["russh", "ssh"])]
    transport: Option<String>,

    /// Connect timeout in seconds (default 10).
    #[arg(long, global = true)]
    connect_timeout: Option<u64>,

    /// Increase verbosity (-v info, -vv debug).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Maximum verbosity.
    #[arg(long, global = true)]
    debug: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Inspect configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage plugins (install/enable/disable/update/remove/list).
    Plugin,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the canonical config file path.
    Path,
    /// Show the effective configuration (with precedence applied).
    Show {
        /// Resolve as for this host alias.
        #[arg(long)]
        host: Option<String>,
    },
}

fn verbosity(cli: &Cli) -> Verbosity {
    if cli.debug {
        Verbosity::Debug
    } else {
        match cli.verbose {
            0 => Verbosity::Normal,
            1 => Verbosity::Verbose,
            _ => Verbosity::VeryVerbose,
        }
    }
}

fn cli_overrides(cli: &Cli) -> CliOverrides {
    CliOverrides {
        shell: cli.shell.clone(),
        cleanup: if cli.keep {
            Some(CleanupMode::Keep)
        } else {
            None
        },
        transport: cli.transport.as_deref().map(|t| match t {
            "ssh" => TransportBackend::Ssh,
            _ => TransportBackend::Russh,
        }),
        connect_timeout_s: cli.connect_timeout,
    }
}

fn load_config() -> Result<Config, u8> {
    match Config::default_path() {
        Some(p) => Config::load(&p).map_err(|e| report(&e)),
        None => Ok(Config::default()),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    xxh_core::init_observability(verbosity(&cli));

    let code = run(&cli);
    ExitCode::from(code)
}

fn run(cli: &Cli) -> u8 {
    match &cli.command {
        Some(Command::Config { action }) => run_config(action),
        Some(Command::Plugin) => {
            eprintln!("xxh: `plugin` management is not yet implemented (T038)");
            exit::USAGE
        }
        None => match &cli.host {
            Some(host) => run_connect(host, cli),
            None => {
                eprintln!("xxh: no host given. Try `xxh <host>` or `xxh --help`.");
                exit::USAGE
            }
        },
    }
}

fn run_config(action: &ConfigAction) -> u8 {
    match action {
        ConfigAction::Path => match Config::default_path() {
            Some(p) => {
                println!("{}", p.display());
                exit::OK
            }
            None => {
                eprintln!("xxh: cannot determine config directory");
                exit::CONFIG
            }
        },
        ConfigAction::Show { host } => {
            let cfg = match load_config() {
                Ok(c) => c,
                Err(code) => return code,
            };
            let alias = host.as_deref().unwrap_or("<global>");
            let eff = cfg.resolve(alias, &CliOverrides::default());
            // Effective settings only — no secrets are part of the config (Принцип V).
            println!("host          = {alias}");
            println!("shell         = {}", eff.shell);
            println!("transport     = {:?}", eff.transport);
            println!("cleanup       = {:?}", eff.cleanup);
            println!("connect_timeout_s = {}", eff.connect_timeout_s);
            println!("enabled_plugins   = {:?}", eff.enabled_plugins);
            exit::OK
        }
    }
}

fn run_connect(host: &str, cli: &Cli) -> u8 {
    let cfg = match load_config() {
        Ok(c) => c,
        Err(code) => return code,
    };
    let eff = cfg.resolve(host, &cli_overrides(cli));

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("xxh: cannot start runtime: {e}");
            return exit::TRANSPORT;
        }
    };

    // Select the transport backend (Принцип III); the rest of the flow is identical.
    let result = rt.block_on(async {
        match eff.transport {
            TransportBackend::Ssh => {
                let t = match SshCliTransport::new() {
                    Ok(t) => t,
                    Err(e) => return Err(SessionError::from(e)),
                };
                connect_and_run(t, host, &eff).await
            }
            TransportBackend::Russh => {
                connect_and_run(RusshTransport::new(), host, &eff).await
            }
        }
    });

    match result {
        Ok(code) => u8::try_from(code.clamp(0, 255)).unwrap_or(1),
        Err(e) => report(&e),
    }
}

/// Establish and run one interactive session over the given transport.
async fn connect_and_run<T: Transport>(
    transport: T,
    host: &str,
    eff: &Effective,
) -> Result<i32, SessionError> {
    let mut target = ResolvedSshTarget::new(host);
    target.connect_timeout_s = eff.connect_timeout_s;

    // MVP environment: a minimal config bundle proving delivery; real dotfiles/
    // plugins/shell packages extend this (T020, US4).
    let fmt = "gz"; // safe default until host caps are known
    let env = match xxh_core::session::minimal_env_component(fmt) {
        Ok(c) => vec![c],
        Err(e) => return Err(SessionError::from(e)),
    };

    let mut session = Session::establish(transport, &target, eff, &env).await?;
    // The requested shell is launched over a PTY; on exit the remote trap cleans up.
    let code = session.run_interactive(&eff.shell).await?;
    session.finish().await?;
    Ok(code)
}
