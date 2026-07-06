//! xxh — CLI entry point.
//!
//! Parses arguments (clap), applies the config precedence, and dispatches to
//! `commands::{connect, plugin, config}`. Every error class renders as
//! «class: причина: действие» and maps to its distinguishable exit code
//! (T005/T043, §FR-026, contracts/cli-commands.md).

mod commands;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use commands::config::ConfigAction;
use commands::plugin::{PluginAction, PluginCmdError};
use xxh_config::{CleanupMode, CliOverrides, TransportBackend};
use xxh_core::Verbosity;
use xxh_core::session::SessionError;

/// Exit-code taxonomy so error classes are distinguishable (§FR-026).
mod exit {
    pub const OK: u8 = 0;
    pub const TRANSPORT: u8 = 10;
    pub const SHELL: u8 = 20;
    pub const PLUGIN: u8 = 30;
    pub const CONFIG: u8 = 40;
    pub const USAGE: u8 = 2;
}

/// Map each error class to its class name and exit code (T005/T043, §FR-026).
fn classify(err: &SessionError) -> (&'static str, u8) {
    match err {
        SessionError::Transport(_) => ("transport", exit::TRANSPORT),
        SessionError::Shell(_) => ("shell", exit::SHELL),
        SessionError::Plugin(_) => ("plugin", exit::PLUGIN),
    }
}

/// Render an error as «class: причина» on stderr and return its exit code (T043).
/// The advised action is part of each error's Display (e.g. ShellError hints).
fn report(class: &str, err: &dyn std::fmt::Display, code: u8) -> u8 {
    eprintln!("xxh: {class}: {err}");
    code
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
    /// Manage plugins (add/remove/enable/disable/update/list).
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    xxh_core::init_observability(verbosity(&cli));

    let code = run(&cli);
    ExitCode::from(code)
}

fn run(cli: &Cli) -> u8 {
    match &cli.command {
        Some(Command::Config { action }) => {
            match commands::config::run(action, &cli_overrides(cli)) {
                Ok(()) => exit::OK,
                Err(e) => report("config", &e, exit::CONFIG),
            }
        }
        Some(Command::Plugin { action }) => {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => return report("transport", &e, exit::TRANSPORT),
            };
            match rt.block_on(commands::plugin::run(action)) {
                Ok(()) => exit::OK,
                Err(PluginCmdError::Plugin(e)) => report("plugin", &e, exit::PLUGIN),
                Err(PluginCmdError::Config(e)) => report("config", &e, exit::CONFIG),
            }
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

fn run_connect(host: &str, cli: &Cli) -> u8 {
    let cfg = match commands::config::load() {
        Ok(c) => c,
        Err(e) => return report("config", &e, exit::CONFIG),
    };
    let eff = cfg.resolve(host, &cli_overrides(cli));

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return report("transport", &e, exit::TRANSPORT),
    };
    match rt.block_on(commands::connect::run(host, &eff)) {
        Ok(code) => u8::try_from(code.clamp(0, 255)).unwrap_or(1),
        Err(e) => {
            let (class, code) = classify(&e);
            report(class, &e, code)
        }
    }
}
