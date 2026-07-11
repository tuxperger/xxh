//! xxh — CLI entry point.
//!
//! Parses arguments (clap), applies the config precedence, and dispatches to
//! `commands::{connect, plugin, config}`. Every error class renders as
//! «class: причина: действие» and maps to its distinguishable exit code
//! (T005/T043, §FR-026, contracts/cli-commands.md).

mod commands;
mod target;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use commands::config::ConfigAction;
use commands::plugin::{PluginAction, PluginCmdError};
use target::{CliTargetFlags, ParsedTarget};
use xxh_config::{CleanupMode, CliOverrides, Effective, RuntimeSetting, TransportBackend};
use xxh_core::Verbosity;
use xxh_core::session::SessionError;
use xxh_transport::{ContainerTarget, ResolvedSshTarget, ResolvedTarget};

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
    /// Target to connect to when no subcommand is given: an SSH `[user@]host`
    /// (compatible with ~/.ssh/config), or a container `docker:<ref>` /
    /// `podman:<ref>` / `container:<ref>`.
    host: Option<String>,

    /// Shell to use for this session (overrides config).
    #[arg(long, global = true)]
    shell: Option<String>,

    /// Login user on the remote host (overrides `user@host` and config).
    #[arg(short = 'l', long, global = true)]
    user: Option<String>,

    /// Private key (identity file) for authentication, used exclusively.
    #[arg(short = 'i', long, global = true, value_name = "PATH")]
    identity: Option<std::path::PathBuf>,

    /// Keep the environment on the host between sessions.
    #[arg(long, global = true)]
    keep: bool,

    /// Transport backend (SSH targets only).
    #[arg(long, global = true, value_parser = ["russh", "ssh"])]
    transport: Option<String>,

    /// Container runtime for `container:` targets (container targets only).
    #[arg(long, global = true, value_parser = ["auto", "docker", "podman"])]
    runtime: Option<String>,

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
        user: cli.user.clone(),
        identity: cli.identity.clone(),
        container_runtime: cli.runtime.as_deref().map(|r| match r {
            "docker" => RuntimeSetting::Docker,
            "podman" => RuntimeSetting::Podman,
            _ => RuntimeSetting::Auto,
        }),
    }
}

/// Split a `[user@]host` command-line target. The user prefix ranks as a CLI
/// override (below `-l`, above config); config lookups use the bare alias.
fn split_user_host(target: &str) -> (Option<&str>, &str) {
    match target.split_once('@') {
        Some((user, host)) if !user.is_empty() && !host.is_empty() => (Some(user), host),
        _ => (None, target),
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

fn run_connect(raw_target: &str, cli: &Cli) -> u8 {
    let cfg = match commands::config::load() {
        Ok(c) => c,
        Err(e) => return report("config", &e, exit::CONFIG),
    };

    // Parse the target family first (pure grammar), then reject flags that do not
    // apply to that family before touching the network (C-A2/C-A5).
    let parsed = match target::parse(raw_target) {
        Ok(p) => p,
        Err(e) => return report("config", &e, exit::CONFIG),
    };
    let flags = CliTargetFlags {
        identity_set: cli.identity.is_some(),
        transport_set: cli.transport.is_some(),
        runtime_set: cli.runtime.is_some(),
    };
    if let Err(e) = target::validate_flags(&parsed, &flags) {
        return report("config", &e, exit::CONFIG);
    }

    // Resolve effective settings against the right alias, then build the target.
    let (eff, resolved) = match resolve_target(&cfg, cli, parsed) {
        Ok(pair) => pair,
        Err(e) => return report("config", &e, exit::CONFIG),
    };

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return report("transport", &e, exit::TRANSPORT),
    };
    let outcome = rt.block_on(commands::connect::run(resolved, &eff));
    // The PTY stdin forwarder may still be blocked reading the local terminal;
    // a plain runtime drop would wait for that read (i.e. hang until a
    // keypress after `exit`). Shut down without waiting instead.
    rt.shutdown_background();
    match outcome {
        Ok(code) => u8::try_from(code.clamp(0, 255)).unwrap_or(1),
        Err(e) => {
            let (class, code) = classify(&e);
            report(class, &e, code)
        }
    }
}

/// Apply config precedence and assemble the `ResolvedTarget` for the parsed family.
/// SSH resolves against the bare host alias (honouring `user@` and ssh-config);
/// container resolves against the reference and applies the runtime precedence.
fn resolve_target(
    cfg: &xxh_config::Config,
    cli: &Cli,
    parsed: ParsedTarget,
) -> Result<(Effective, ResolvedTarget), target::TargetError> {
    match parsed {
        ParsedTarget::Ssh { host } => {
            let (prefix_user, bare) = split_user_host(&host);
            let mut overrides = cli_overrides(cli);
            // `-l` beats the `user@` prefix; both beat config (§FR-024 precedence).
            if overrides.user.is_none() {
                overrides.user = prefix_user.map(str::to_string);
            }
            let eff = cfg.resolve(bare, &overrides);
            let mut ssh = ResolvedSshTarget::new(bare);
            ssh.connect_timeout_s = eff.connect_timeout_s;
            ssh.user = eff.user.clone();
            ssh.identity = eff.identity.clone();
            Ok((eff, ResolvedTarget::Ssh(ssh)))
        }
        ParsedTarget::Container { scheme, reference } => {
            let overrides = cli_overrides(cli);
            let eff = cfg.resolve(&reference, &overrides);
            let selector = target::resolve_runtime_selector(
                scheme,
                eff.container_runtime,
                overrides.container_runtime,
            )?;
            let ct = ContainerTarget {
                reference,
                runtime: selector,
                // The shared `user` key is the exec-session user (`-u`) for
                // containers (C-A5); `None` keeps the container's own user.
                exec_user: eff.user.clone(),
                connect_timeout_s: eff.connect_timeout_s,
            };
            Ok((eff, ResolvedTarget::Container(ct)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::split_user_host;

    #[test]
    fn user_host_prefix_is_split() {
        assert_eq!(split_user_host("web"), (None, "web"));
        assert_eq!(split_user_host("deploy@web"), (Some("deploy"), "web"));
        // Degenerate forms stay untouched — let ssh resolution reject them.
        assert_eq!(split_user_host("@web"), (None, "@web"));
        assert_eq!(split_user_host("deploy@"), (None, "deploy@"));
    }
}
