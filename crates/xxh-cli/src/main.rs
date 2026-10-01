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
use xxh_core::session::{ExecCommand, SessionError};
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

    /// Run this command line with your shell on the target instead of opening a
    /// session (pipelines, redirections and aliases work).
    #[arg(
        short = 'c',
        long = "command",
        value_name = "STRING",
        conflicts_with = "args"
    )]
    shell_line: Option<String>,

    /// Allocate a terminal for the command given by `-c` or after `--`.
    #[arg(short = 't', long)]
    tty: bool,

    /// Command to run on the target instead of opening a session; the arguments
    /// after `--` are passed through exactly, with no shell interpretation.
    #[arg(last = true, value_name = "COMMAND")]
    args: Vec<String>,

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

/// A one-command run: what to execute and whether to give it a terminal.
type ExecRequest = (ExecCommand, bool);

/// Work out whether this invocation is a one-command run (004, C-XC2/C-XC3).
/// `bare_separator` tells a trailing `--` with nothing after it apart from no `--`
/// at all, which clap reports identically. Every misuse is rejected here, before
/// any connection.
fn exec_request(cli: &Cli, bare_separator: bool) -> Result<Option<ExecRequest>, String> {
    let cmd = match (&cli.shell_line, cli.args.is_empty()) {
        (Some(line), _) if line.trim().is_empty() => {
            return Err("`-c` needs a non-empty command line".into());
        }
        (Some(line), _) => ExecCommand::ShellLine(line.clone()),
        (None, false) => ExecCommand::Argv(cli.args.clone()),
        (None, true) if bare_separator => {
            return Err("`--` must be followed by the command to run".into());
        }
        (None, true) if cli.tty => {
            return Err("`-t` applies to a command: add `-c '<line>'` or `-- <command>`".into());
        }
        (None, true) => return Ok(None),
    };
    if cli.command.is_some() {
        return Err("a command can only be run on a target, not together with a subcommand".into());
    }
    if cli.host.is_none() {
        return Err("no target given for the command. Try `xxh <host> -- <command>`".into());
    }
    Ok(Some((cmd, cli.tty)))
}

/// The process exit code for a finished session. A one-command run passes the
/// command's code through, and anything outside 0..=255 is "unknown" = 255 — never
/// a silent success (004 §FR-004). The interactive path keeps its clamping.
fn session_exit_code(code: i32, exec: bool) -> u8 {
    match u8::try_from(code) {
        Ok(c) => c,
        Err(_) if exec => 255,
        Err(_) => u8::try_from(code.clamp(0, 255)).unwrap_or(1),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    xxh_core::init_observability(verbosity(&cli));

    let bare_separator = std::env::args().next_back().as_deref() == Some("--");
    let code = match exec_request(&cli, bare_separator) {
        Ok(exec) => run(&cli, exec),
        Err(msg) => {
            eprintln!("xxh: {msg}");
            exit::USAGE
        }
    };
    ExitCode::from(code)
}

fn run(cli: &Cli, exec: Option<ExecRequest>) -> u8 {
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
            Some(host) => run_connect(host, cli, exec),
            None => {
                eprintln!("xxh: no host given. Try `xxh <host>` or `xxh --help`.");
                exit::USAGE
            }
        },
    }
}

fn run_connect(raw_target: &str, cli: &Cli, exec: Option<ExecRequest>) -> u8 {
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
    // Stage progress would pollute a script's stderr: a one-command run shows it
    // only when asked for (004 §FR-012).
    let quiet = exec.is_some() && verbosity(cli) == Verbosity::Normal;
    let is_exec = exec.is_some();
    let outcome = rt.block_on(commands::connect::run(resolved, &eff, exec.as_ref(), quiet));
    // The PTY stdin forwarder may still be blocked reading the local terminal;
    // a plain runtime drop would wait for that read (i.e. hang until a
    // keypress after `exit`). Shut down without waiting instead.
    rt.shutdown_background();
    match outcome {
        Ok(code) => session_exit_code(code, is_exec),
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
    use super::*;

    fn request(args: &[&str], bare: bool) -> Result<Option<ExecRequest>, String> {
        let cli = Cli::try_parse_from(args).expect("clap accepts the arguments");
        exec_request(&cli, bare)
    }

    #[test]
    fn arguments_after_the_separator_are_an_exact_argv() {
        let got = request(
            &["xxh", "web", "--keep", "--", "ls", "-la", "a b", "--help"],
            false,
        );
        let argv = ["ls", "-la", "a b", "--help"].map(String::from).to_vec();
        assert_eq!(got, Ok(Some((ExecCommand::Argv(argv), false))));
    }

    #[test]
    fn command_line_flag_and_tty_are_recognised() {
        assert_eq!(
            request(&["xxh", "web", "-t", "-c", "a | b"], false),
            Ok(Some((ExecCommand::ShellLine("a | b".into()), true)))
        );
        // No command at all: a plain interactive session.
        assert_eq!(request(&["xxh", "web"], false), Ok(None));
    }

    #[test]
    fn misuse_is_rejected_before_any_connection() {
        // `-c` together with `-- …` is refused by the parser itself (C-XC2).
        assert!(Cli::try_parse_from(["xxh", "web", "-c", "x", "--", "y"]).is_err());
        // The rest is refused by `exec_request` (C-XC3).
        assert!(
            request(&["xxh", "web", "--"], true).is_err(),
            "bare separator"
        );
        assert!(
            request(&["xxh", "web", "-t"], false).is_err(),
            "-t without a command"
        );
        assert!(
            request(&["xxh", "web", "-c", "  "], false).is_err(),
            "empty line"
        );
        assert!(request(&["xxh", "-c", "x"], false).is_err(), "no target");
        assert!(
            request(&["xxh", "-c", "x", "config", "path"], false).is_err(),
            "with a subcommand"
        );
    }

    #[test]
    fn command_exit_codes_pass_through() {
        assert_eq!(session_exit_code(0, true), 0);
        assert_eq!(session_exit_code(7, true), 7);
        assert_eq!(session_exit_code(143, true), 143);
        // Unknown or out-of-range is never reported as success (004 §FR-004).
        assert_eq!(session_exit_code(-1, true), 255);
        assert_eq!(session_exit_code(4096, true), 255);
        // The interactive path keeps its historical clamping.
        assert_eq!(session_exit_code(-1, false), 0);
        assert_eq!(session_exit_code(300, false), 255);
    }

    #[test]
    fn user_host_prefix_is_split() {
        assert_eq!(split_user_host("web"), (None, "web"));
        assert_eq!(split_user_host("deploy@web"), (Some("deploy"), "web"));
        // Degenerate forms stay untouched — let ssh resolution reject them.
        assert_eq!(split_user_host("@web"), (None, "@web"));
        assert_eq!(split_user_host("deploy@"), (None, "deploy@"));
    }
}
