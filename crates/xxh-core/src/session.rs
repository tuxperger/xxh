//! Session orchestration (T016–T018, T024/T025): connect → detect → resolve shell →
//! deploy → run → cleanup.
//!
//! Zero-footprint by construction (Принцип I): platform detection streams the
//! bootstrap script over stdin and creates nothing on the host; the requested shell
//! is resolved (packaged locally or present on the host) *before* anything is
//! written, so a missing shell fails with no partial deployment (§FR-011). Cleanup
//! is guaranteed by the remote `trap` (bootstrap.sh) plus a reconcile sweep on the
//! next connect.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use xxh_config::{CleanupMode, Effective};
use xxh_plugin_api::{LifecycleStage, Manifest};
use xxh_plugins::PluginError;
use xxh_transport::{AuthPolicy, ResolvedTarget, Transport};

use crate::ShellError;
use crate::deploy::{Component, ComponentKind};
use crate::platform::Platform;
use crate::shellpkg;

/// The embedded reference bootstrap script (Принцип I; contracts/bootstrap-protocol.md).
const BOOTSTRAP_SH: &str = include_str!("../../../bootstrap/bootstrap.sh");

/// Errors distinguishable by class for the CLI (§FR-026).
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Transport(#[from] xxh_transport::TransportError),
    #[error(transparent)]
    Shell(#[from] ShellError),
    #[error(transparent)]
    Plugin(#[from] PluginError),
}

/// Stage-progress callback (§FR-025): called with a short human-readable line for
/// each session stage (connect → detect → deliver → plugins → shell).
pub type Progress<'a> = &'a (dyn Fn(&str) + Sync);

/// A no-op progress sink for tests and non-interactive callers.
pub fn silent_progress() -> Progress<'static> {
    &|_| {}
}

/// How many components were actually transferred vs already cached (§FR-013/014).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeliveryReport {
    pub delivered: usize,
    pub reused: usize,
}

/// An enabled plugin ready for the session, in resolved load order (T039):
/// its parsed manifest plus the local package directory to deliver.
#[derive(Debug, Clone)]
pub struct SessionPlugin {
    pub manifest: Manifest,
    pub dir: PathBuf,
}

/// Run every hook of `plugins` attached to `stage`, each in an isolated
/// subprocess. A failing hook is reported and **skipped** — one broken plugin
/// must not take the session down (§FR-019/020, C-M4).
async fn run_stage_hooks(plugins: &[SessionPlugin], stage: LifecycleStage, progress: Progress<'_>) {
    for p in plugins {
        let Some(hook) = p.manifest.hooks.get(&stage) else {
            continue;
        };
        let mut env = BTreeMap::new();
        env.insert("XXH_STAGE".to_string(), format!("{stage:?}"));
        if let Err(e) = xxh_plugins::isolation::run_hook(&p.manifest.name, &p.dir, hook, &env).await
        {
            tracing::warn!(plugin = %p.manifest.name, error = %e, "plugin hook failed; continuing");
            progress(&format!(
                "plugin {}: hook failed ({e}); session continues",
                p.manifest.name
            ));
        }
    }
}

/// How the requested shell will be launched on the host.
enum ShellLaunch {
    /// Delivered as a packaged component; the path is resolved from its cache hash.
    Packaged { hash: String, bin_rel: String },
    /// Present on the host already; launched by name.
    HostBinary(String),
}

/// What a one-command run executes on the target (004, data-model.md).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecCommand {
    /// An exact argument list: reaches the target unchanged, never re-interpreted
    /// by a shell (004 §FR-006).
    Argv(Vec<String>),
    /// A command line for the session's shell — pipelines, redirections, aliases
    /// (004 §FR-011).
    ShellLine(String),
}

/// Single-quote `s` for a POSIX shell: the only way a value survives every
/// metacharacter, whitespace and newline unchanged.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The command line that follows `exec` on the target for `cmd`.
fn exec_line(cmd: &ExecCommand, shell_cmd: &str) -> String {
    match cmd {
        ExecCommand::Argv(args) => args
            .iter()
            .map(|a| sh_quote(a))
            .collect::<Vec<_>>()
            .join(" "),
        ExecCommand::ShellLine(line) => format!("{shell_cmd} -c {}", sh_quote(line)),
    }
}

/// A connected, prepared session ready to launch a shell.
pub struct Session<T: Transport> {
    transport: T,
    platform: Platform,
    keep: bool,
    /// The resolved writable environment root on the target (FR-011/C-C12): the
    /// bootstrap script and the content cache live here, and cleanup removes it.
    remote_root: String,
    session_id: String,
    /// Shell-launch command resolved during establish (§FR-008..011).
    shell_cmd: String,
    /// Prelude sourced before the shell starts (PATH for packaged shells, env.sh
    /// of config/plugin components, in delivery order).
    prelude: String,
    report: DeliveryReport,
    /// Plugins active in this session (platform-filtered, resolved order) —
    /// kept for their `pre_exit` hooks (T039).
    plugins: Vec<SessionPlugin>,
}

impl<T: Transport> Session<T> {
    /// Establish a session: connect, detect platform (leaving the host untouched on
    /// failure), resolve the shell (no partial deployment on a missing shell),
    /// reconcile stale artefacts, and deliver the environment.
    pub async fn establish(
        mut transport: T,
        target: &ResolvedTarget,
        eff: &Effective,
        env_components: &[Component],
        plugins: &[SessionPlugin],
        progress: Progress<'_>,
    ) -> Result<Self, SessionError> {
        run_stage_hooks(plugins, LifecycleStage::PreConnect, progress).await;
        progress(&format!("connect {}", target.label()));
        transport.connect(target, &AuthPolicy::default()).await?;

        // 1) Detect platform by streaming the script over stdin — creates nothing on
        //    the host, so an unsupported platform leaves it clean (C-B1, §FR-007).
        progress("detect platform");
        let detect = transport
            .upload_stream("sh -s -- detect", BOOTSTRAP_SH.as_bytes().to_vec())
            .await?;
        if detect.exit_code != 0 {
            return Err(ShellError::Other(format!(
                "platform detection failed: {}",
                String::from_utf8_lossy(&detect.stderr)
            ))
            .into());
        }
        let platform = Platform::parse_detect(&detect.stdout_str())?;
        let fmt = platform.preferred_archive_fmt();

        // 2) Resolve the requested shell BEFORE any write to the host (§FR-011):
        //    a local package payload wins; otherwise the shell must already exist
        //    on the host; otherwise fail with a clear shell-class error.
        let mut components: Vec<Component> = Vec::new();
        let launch = match shellpkg::find(&eff.shell, &platform)? {
            Some(pkg) => {
                let comp = Component::pack_dir(ComponentKind::Shell, &pkg.tree, fmt)?;
                let launch = ShellLaunch::Packaged {
                    hash: comp.hash.clone(),
                    bin_rel: pkg.bin_rel,
                };
                components.push(comp);
                launch
            }
            None => {
                let probe = transport
                    .exec(&format!("command -v {} >/dev/null 2>&1", eff.shell))
                    .await?;
                if probe.exit_code != 0 {
                    return Err(ShellError::NotAvailable(eff.shell.clone()).into());
                }
                ShellLaunch::HostBinary(eff.shell.clone())
            }
        };
        components.extend(env_components.iter().cloned());

        // 2b) Plugins: filter by the detected platform (skip with a message, C-M5),
        //     keep the resolved order, and pack each package for delivery (T039).
        let mut active: Vec<SessionPlugin> = Vec::new();
        for p in plugins {
            if !p
                .manifest
                .supports(platform.os_str(), platform.arch_str(), platform.libc_str())
            {
                progress(&format!(
                    "plugin {}: skipped (does not target {})",
                    p.manifest.name,
                    platform.target_key()
                ));
                continue;
            }
            components.push(Component::pack_dir(ComponentKind::Plugin, &p.dir, fmt)?);
            active.push(p.clone());
        }
        if !active.is_empty() {
            progress(&format!("plugins: {} active", active.len()));
        }

        // 3) Resolve the single writable root on the target ($HOME → $TMPDIR →
        //    /tmp; C-C12/FR-011). Streamed like detect, so no writable location
        //    anywhere fails here with NO partial deployment. Every later bootstrap
        //    call carries this exact root so the client and script never diverge.
        let root_out = transport
            .upload_stream("sh -s -- root", BOOTSTRAP_SH.as_bytes().to_vec())
            .await?;
        if root_out.exit_code != 0 {
            return Err(ShellError::Other(format!(
                "no writable directory on the target for the xxh environment: {}",
                String::from_utf8_lossy(&root_out.stderr).trim()
            ))
            .into());
        }
        let remote_root = root_out.stdout_str();
        let remote_boot = format!("{remote_root}/boot.sh");
        let boot = |args: &str| format!("XXH_ROOT={remote_root} sh {remote_boot} {args}");

        // Install the bootstrap script into the resolved root and sweep any stale
        // artefacts from a previously crashed session (§FR-006).
        transport
            .upload_stream(
                &format!("mkdir -p {remote_root} && cat > {remote_boot} && chmod +x {remote_boot}"),
                BOOTSTRAP_SH.as_bytes().to_vec(),
            )
            .await?;
        transport.exec(&boot("reconcile")).await?;

        // 4) Deliver only components missing from the host cache (§FR-013, VI).
        let host_hashes = list_cache(&mut transport, &boot("list-cache")).await?;
        let to_send = super::deploy::missing(&components, &host_hashes);
        let report = DeliveryReport {
            delivered: to_send.len(),
            reused: components.len() - to_send.len(),
        };
        progress(&format!(
            "deliver components: sending {}, reused {}",
            report.delivered, report.reused
        ));
        tracing::info!(
            delivered = report.delivered,
            reused = report.reused,
            "component delivery (reused {} components)",
            report.reused
        );
        for comp in to_send {
            transport
                .upload_stream(
                    &boot(&format!("recv {} {}", comp.hash, comp.fmt)),
                    comp.payload.clone(),
                )
                .await?;
        }
        run_stage_hooks(&active, LifecycleStage::PostDeploy, progress).await;

        // 5) Assemble the prelude in delivery order: packaged shells extend PATH,
        //    config/plugin components contribute their env.sh.
        let mut prelude = String::new();
        for comp in &components {
            match comp.kind {
                // Shell packages extend PATH and may ship an env.sh of their own
                // (e.g. zsh-bin exports FPATH at its delivered functions dir).
                ComponentKind::Shell => prelude.push_str(&format!(
                    "export PATH=\"{root}/cache/{h}/bin:$PATH\"; \
                     XXH_COMPONENT_DIR={root}/cache/{h}; export XXH_COMPONENT_DIR; \
                     . {root}/cache/{h}/env.sh 2>/dev/null || true; ",
                    root = remote_root,
                    h = comp.hash
                )),
                // XXH_COMPONENT_DIR lets an env.sh reference its own cache dir
                // (PATH for tool packages, TERMINFO/SSL_CERT_FILE for nix ones).
                ComponentKind::Config | ComponentKind::Plugin => prelude.push_str(&format!(
                    "XXH_COMPONENT_DIR={root}/cache/{h}; export XXH_COMPONENT_DIR; \
                     . {root}/cache/{h}/env.sh 2>/dev/null || true; ",
                    root = remote_root,
                    h = comp.hash
                )),
            }
        }

        let shell_cmd = match launch {
            ShellLaunch::Packaged { hash, bin_rel } => {
                format!("{remote_root}/cache/{hash}/{bin_rel}")
            }
            ShellLaunch::HostBinary(name) => name,
        };

        Ok(Self {
            transport,
            platform,
            keep: matches!(eff.cleanup, CleanupMode::Keep),
            remote_root,
            session_id: session_id(),
            shell_cmd,
            prelude,
            report,
            plugins: active,
        })
    }

    /// Detected host platform.
    pub fn platform(&self) -> &Platform {
        &self.platform
    }

    /// Transfer statistics for this establish (§FR-014, SC-004).
    pub fn delivery_report(&self) -> DeliveryReport {
        self.report
    }

    /// Build the remote command that sources the prelude then execs `shell_cmd`.
    fn shell_invocation(&self, shell_cmd: &str) -> String {
        self.invocation("", shell_cmd)
    }

    /// [`shell_invocation`](Self::shell_invocation) with `before_exec` — shell text
    /// run after the prelude and right before the `exec`.
    fn invocation(&self, before_exec: &str, shell_cmd: &str) -> String {
        let keep = if self.keep { "1" } else { "0" };
        // bootstrap `run` installs the cleanup trap, then execs the given argv.
        // XXH_ROOT is pinned so the script targets exactly the resolved root.
        // Set via `env`, not a bare assignment prefix: PTY transports prepend
        // `exec`, which would treat `XXH_ROOT=…` as the command name.
        format!(
            "env XXH_ROOT={root} sh {root}/boot.sh run {} {keep} sh -c '{}{}exec {}'",
            self.session_id,
            self.prelude.replace('\'', "'\\''"),
            before_exec.replace('\'', "'\\''"),
            shell_cmd.replace('\'', "'\\''"),
            root = self.remote_root,
        )
    }

    /// Launch the resolved interactive shell over a PTY (real use).
    pub async fn run_interactive(&mut self) -> Result<i32, SessionError> {
        let cmd = self.shell_invocation(&self.shell_cmd.clone());
        let spec = xxh_transport::PtySpec {
            term: std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".into()),
            cols: 80,
            rows: 24,
            shell_cmd: cmd,
            env: Default::default(),
        };
        Ok(self.transport.open_pty(&spec).await?)
    }

    /// Run a non-interactive command inside the prepared environment (tests / scripts).
    pub async fn run_command(&mut self, shell_cmd: &str) -> Result<i32, SessionError> {
        let cmd = self.shell_invocation(shell_cmd);
        let out = self.transport.exec(&cmd).await?;
        Ok(out.exit_code)
    }

    /// Run one command in the prepared environment with the client's stdio
    /// attached and return its exit code (004, contracts/exec-transport.md
    /// C-X8..C-X11). Same prelude and same cleanup trap as the interactive shell.
    ///
    /// The command's pid is recorded on the target so that, if this client is
    /// interrupted (SIGINT/SIGTERM), the command is stopped there and the cleanup
    /// trap gets to run before we return `128 + signal` — whatever the transport,
    /// since neither a PTY-less SSH channel nor `docker exec -i` forwards signals.
    pub async fn run_exec(&mut self, cmd: &ExecCommand, tty: bool) -> Result<i32, SessionError> {
        let sessions = format!("{}/sessions", self.remote_root);
        let cmd_marker = format!("{sessions}/{}.cmd", self.session_id);
        // `$$` of the inner shell is the command's own pid once it `exec`s.
        let invocation = self.invocation(
            &format!("echo $$ >{cmd_marker}; "),
            &exec_line(cmd, &self.shell_cmd),
        );

        if tty {
            // A terminal was asked for: signals travel through it as keystrokes.
            let spec = xxh_transport::PtySpec {
                term: std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".into()),
                cols: 80,
                rows: 24,
                shell_cmd: invocation,
                env: Default::default(),
            };
            return Ok(self.transport.open_pty(&spec).await?);
        }

        use tokio::signal::unix::{SignalKind, signal};
        let io = |e: std::io::Error| SessionError::Transport(e.into());
        let mut interrupt = signal(SignalKind::interrupt()).map_err(io)?;
        let mut terminate = signal(SignalKind::terminate()).map_err(io)?;
        let signo = tokio::select! {
            code = self.transport.exec_stream(&invocation) => return Ok(code?),
            _ = interrupt.recv() => 2,
            _ = terminate.recv() => 15,
        };

        // The stream is cancelled; stop the command and give the remote trap up to
        // five seconds to finish cleaning (it removes the session marker last).
        // Where /proc tells us the command's process group, the whole group is
        // signalled so that children of a `-c` command line die too; otherwise only
        // the command itself.
        let stop = format!(
            "p=$(cat {cmd_marker} 2>/dev/null) || exit 0; g=; \
             [ -r \"/proc/$p/stat\" ] && read -r _ _ _ _ g _ <\"/proc/$p/stat\"; \
             case \"$g\" in ''|0|1|*[!0-9]*) kill -TERM \"$p\" 2>/dev/null;; \
             *) kill -TERM -- \"-$g\" 2>/dev/null || kill -TERM \"$p\" 2>/dev/null;; esac; \
             i=0; while [ -e {sessions}/{sid} ] && [ \"$i\" -lt 50 ]; do \
             sleep 0.1 2>/dev/null || sleep 1; i=$((i+1)); done",
            sid = self.session_id
        );
        let _ = self.transport.exec(&stop).await;
        Ok(128 + signo)
    }

    /// Close the transport. Host cleanup is guaranteed by the remote trap during
    /// `run`; this also runs plugin `pre_exit` hooks and disconnects cleanly.
    pub async fn finish(mut self) -> Result<(), SessionError> {
        run_stage_hooks(&self.plugins, LifecycleStage::PreExit, silent_progress()).await;
        self.transport.disconnect().await?;
        Ok(())
    }
}

async fn list_cache<T: Transport>(
    t: &mut T,
    list_cmd: &str,
) -> Result<BTreeSet<String>, SessionError> {
    let out = t.exec(list_cmd).await?;
    Ok(out
        .stdout_str()
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

fn session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("s{n:x}")
}

/// Build a minimal environment component that marks the xxh session and adds a demo
/// alias, proving config delivery end-to-end. Real dotfiles/plugins/shell packages
/// extend this set (T020, US4).
pub fn minimal_env_component(fmt: &str) -> Result<Component, ShellError> {
    let dir = std::env::temp_dir().join(format!("xxh-env-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| ShellError::Other(e.to_string()))?;
    std::fs::write(
        dir.join("env.sh"),
        b"export XXH_SESSION=1\nalias xxh-hello='echo hello-from-xxh'\n",
    )
    .map_err(|e| ShellError::Other(e.to_string()))?;
    let comp = Component::pack_dir(ComponentKind::Config, &dir, fmt);
    let _ = std::fs::remove_dir_all(&dir);
    comp
}

/// Build a component carrying the client's compiled terminfo entry for `$TERM`.
///
/// Modern terminals (ghostty, kitty, wezterm, …) set a `TERM` most hosts have no
/// terminfo for, which breaks line editing (backspace/cursor artefacts in zle).
/// The client has the entry — deliver it and point `TERMINFO_DIRS` at the copy.
/// `None` when `$TERM` is unset or the entry cannot be found locally (then the
/// host's own database is the best we can do).
pub fn terminfo_component(fmt: &str) -> Option<Component> {
    let term = std::env::var("TERM").ok()?;
    let src = find_local_terminfo(&term)?;
    let first = term.chars().next()?;

    let dir = std::env::temp_dir().join(format!("xxh-ti-{}", std::process::id()));
    let entry_dir = dir.join("terminfo").join(first.to_string());
    std::fs::create_dir_all(&entry_dir).ok()?;
    std::fs::copy(&src, entry_dir.join(&term)).ok()?;
    // Trailing empty element keeps the host's compiled-in default search path.
    std::fs::write(
        dir.join("env.sh"),
        "export TERMINFO_DIRS=\"$XXH_COMPONENT_DIR/terminfo:${TERMINFO_DIRS:-}\"\n",
    )
    .ok()?;
    let comp = Component::pack_dir(ComponentKind::Config, &dir, fmt).ok();
    let _ = std::fs::remove_dir_all(&dir);
    comp
}

/// Locate the compiled terminfo entry for `term` in the standard client-side
/// locations (`$TERMINFO`, `~/.terminfo`, `$TERMINFO_DIRS`, system dirs).
fn find_local_terminfo(term: &str) -> Option<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("TERMINFO") {
        dirs.push(d.into());
    }
    if let Some(h) = std::env::var_os("HOME") {
        dirs.push(std::path::Path::new(&h).join(".terminfo"));
    }
    if let Some(list) = std::env::var_os("TERMINFO_DIRS") {
        dirs.extend(std::env::split_paths(&list).filter(|p| !p.as_os_str().is_empty()));
    }
    dirs.extend(
        ["/usr/share/terminfo", "/lib/terminfo", "/etc/terminfo"]
            .iter()
            .map(std::path::PathBuf::from),
    );
    find_terminfo_in(&dirs, term)
}

fn find_terminfo_in(dirs: &[std::path::PathBuf], term: &str) -> Option<std::path::PathBuf> {
    let first = term.chars().next()?;
    for d in dirs {
        // Linux layout: <dir>/<first-char>/<term>; macOS uses a hex directory.
        for sub in [first.to_string(), format!("{:x}", first as u32)] {
            let p = d.join(sub).join(term);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    //! Shell-selection unit tests (T026): packaged shell wins; host binary is the
    //! fallback; a missing shell fails with NO writes to the host (§FR-011).

    use super::*;
    use std::sync::{Arc, Mutex};
    use xxh_transport::{ExecOutput, PtySpec, ResolvedSshTarget, TransportError};

    /// SSH target wrapped as the generalized `ResolvedTarget` the trait now takes.
    fn ssh_target(alias: &str) -> ResolvedTarget {
        ResolvedTarget::Ssh(ResolvedSshTarget::new(alias))
    }

    /// Scripted transport: `detect` answers with a Linux host, `command -v` is
    /// answered from `host_shells`, everything else succeeds; every command that
    /// could write to the host is recorded.
    #[derive(Clone, Default)]
    struct MockTransport {
        host_shells: Vec<&'static str>,
        commands: Arc<Mutex<Vec<String>>>,
    }

    impl MockTransport {
        fn ok(stdout: &str) -> ExecOutput {
            ExecOutput {
                exit_code: 0,
                stdout: stdout.as_bytes().to_vec(),
                stderr: vec![],
            }
        }
        fn writes(&self) -> Vec<String> {
            self.commands
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.contains("mkdir") || c.contains("recv") || c.contains(" run "))
                .cloned()
                .collect()
        }
    }

    #[async_trait::async_trait]
    impl Transport for MockTransport {
        async fn connect(
            &mut self,
            _t: &ResolvedTarget,
            _a: &AuthPolicy,
        ) -> Result<(), TransportError> {
            Ok(())
        }
        async fn exec(&mut self, cmd: &str) -> Result<ExecOutput, TransportError> {
            self.commands.lock().unwrap().push(cmd.to_string());
            if cmd.starts_with("command -v") {
                let found = self.host_shells.iter().any(|s| cmd.contains(s));
                return Ok(ExecOutput {
                    exit_code: if found { 0 } else { 1 },
                    stdout: vec![],
                    stderr: vec![],
                });
            }
            Ok(Self::ok(""))
        }
        async fn upload_stream(
            &mut self,
            cmd: &str,
            _data: Vec<u8>,
        ) -> Result<ExecOutput, TransportError> {
            self.commands.lock().unwrap().push(cmd.to_string());
            if cmd.contains("detect") {
                return Ok(Self::ok("Linux x86_64 | tar gzip"));
            }
            if cmd.contains("-- root") {
                return Ok(Self::ok("/home/mock/.xxh"));
            }
            Ok(Self::ok(""))
        }
        async fn exec_stream(&mut self, cmd: &str) -> Result<i32, TransportError> {
            self.commands.lock().unwrap().push(cmd.to_string());
            Ok(7)
        }
        async fn open_pty(&mut self, _spec: &PtySpec) -> Result<i32, TransportError> {
            Ok(0)
        }
        async fn disconnect(&mut self) -> Result<(), TransportError> {
            Ok(())
        }
    }

    fn eff(shell: &str) -> Effective {
        Effective {
            shell: shell.into(),
            enabled_plugins: vec![],
            cleanup: CleanupMode::Ephemeral,
            transport: xxh_config::TransportBackend::Russh,
            connect_timeout_s: 10,
            user: None,
            identity: None,
            container_runtime: xxh_config::RuntimeSetting::Auto,
        }
    }

    /// Hermetic shell resolution: point `XXH_SHELLS_DIR` at an empty directory
    /// so neither the machine's installed packages nor the concurrent
    /// `crate::shellpkg` tests (which mutate the same variable) are visible.
    fn no_shell_packages() -> crate::shellpkg::testenv::ShellsDirGuard {
        let dir = std::env::temp_dir().join(format!("xxh-noshells-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        crate::shellpkg::testenv::shells_dir(&dir)
    }

    #[test]
    fn terminfo_lookup_checks_char_and_hex_layouts() {
        let base = std::env::temp_dir().join(format!("xxh-ti-test-{}", std::process::id()));
        // Linux layout: x/xterm-ghostty; macOS layout: 78/xterm-ghostty.
        std::fs::create_dir_all(base.join("linux/x")).unwrap();
        std::fs::write(base.join("linux/x/xterm-ghostty"), b"ti").unwrap();
        std::fs::create_dir_all(base.join("mac/78")).unwrap();
        std::fs::write(base.join("mac/78/xterm-ghostty"), b"ti").unwrap();

        let found = find_terminfo_in(&[base.join("linux")], "xterm-ghostty");
        assert!(found.is_some(), "char-dir layout must resolve");
        let found = find_terminfo_in(&[base.join("mac")], "xterm-ghostty");
        assert!(found.is_some(), "hex-dir layout must resolve");
        assert!(find_terminfo_in(&[base.join("linux")], "kitty").is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn host_shell_is_used_when_no_package_exists() {
        let _env = no_shell_packages();
        let t = MockTransport {
            host_shells: vec!["bash"],
            ..Default::default()
        };
        let s = Session::establish(
            t,
            &ssh_target("h"),
            &eff("bash"),
            &[],
            &[],
            silent_progress(),
        )
        .await
        .expect("host bash should be accepted");
        assert_eq!(s.shell_cmd, "bash");
    }

    #[tokio::test]
    async fn missing_shell_fails_without_partial_deployment() {
        let _env = no_shell_packages();
        let t = MockTransport::default(); // no shells on host, no packages locally
        let probe = t.clone();
        let err = match Session::establish(
            t,
            &ssh_target("h"),
            &eff("zsh"),
            &[],
            &[],
            silent_progress(),
        )
        .await
        {
            Err(e) => e,
            Ok(_) => panic!("zsh is nowhere to be found — establish must fail"),
        };
        assert!(matches!(
            err,
            SessionError::Shell(ShellError::NotAvailable(_))
        ));
        assert!(
            probe.writes().is_empty(),
            "no write may reach the host on a missing shell (§FR-011): {:?}",
            probe.writes()
        );
    }

    #[tokio::test]
    async fn delivery_report_counts_reused_components() {
        let _env = no_shell_packages();
        let t = MockTransport {
            host_shells: vec!["sh"],
            ..Default::default()
        };
        let env = vec![minimal_env_component("gz").unwrap()];
        let s = Session::establish(
            t,
            &ssh_target("h"),
            &eff("sh"),
            &env,
            &[],
            silent_progress(),
        )
        .await
        .unwrap();
        // Mock host cache is empty ⇒ everything is delivered, nothing reused.
        assert_eq!(
            s.delivery_report(),
            DeliveryReport {
                delivered: 1,
                reused: 0
            }
        );
    }

    /// A plugin built for another platform is skipped with a message and never
    /// packed for delivery; the rest of the environment is unaffected (C-M5,
    /// 003 §FR-012 — how a flake program built for one architecture stays off
    /// hosts of another).
    #[tokio::test]
    async fn plugin_for_another_platform_is_skipped_not_delivered() {
        let _env = no_shell_packages();
        let base = std::env::temp_dir().join(format!("xxh-skip-{}", std::process::id()));
        let plugin = |name: &str, target: &str| {
            let dir = base.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let text = format!(
                "name = \"{name}\"\nversion = \"1.0.0\"\napi_version = \"1.0.0\"\n\
                 targets = [\"{target}\"]\n"
            );
            std::fs::write(dir.join("plugin.toml"), &text).unwrap();
            SessionPlugin {
                manifest: Manifest::parse(&text).unwrap(),
                dir,
            }
        };
        let plugins = [
            plugin("native", "linux/x86_64"),
            plugin("foreign", "linux/aarch64"),
        ];

        let said = Mutex::new(Vec::<String>::new());
        let progress = |line: &str| said.lock().unwrap().push(line.to_string());
        let t = MockTransport {
            host_shells: vec!["sh"],
            ..Default::default()
        };
        let env = vec![minimal_env_component("gz").unwrap()];
        let s = Session::establish(t, &ssh_target("h"), &eff("sh"), &env, &plugins, &progress)
            .await
            .unwrap();

        // env + the native plugin; the foreign one never becomes a component.
        assert_eq!(s.delivery_report().delivered, 2);
        let said = said.lock().unwrap();
        assert!(
            said.iter()
                .any(|l| l.contains("plugin foreign: skipped") && l.contains("linux-x86_64")),
            "the skip must be reported: {said:?}"
        );
        assert!(!said.iter().any(|l| l.contains("plugin native: skipped")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn argv_reaches_the_target_exactly() {
        let argv = |a: &[&str]| ExecCommand::Argv(a.iter().map(|s| s.to_string()).collect());
        assert_eq!(exec_line(&argv(&["ls", "-la"]), "zsh"), "'ls' '-la'");
        // Whitespace, both quote kinds, `$`, `;`, newlines and empty arguments all
        // survive as data, never as shell syntax (004 §FR-006).
        assert_eq!(
            exec_line(
                &argv(&["printf", "%s\n", "a b", "c'd", "\"e\"", "$HOME;x", ""]),
                "zsh"
            ),
            "'printf' '%s\n' 'a b' 'c'\\''d' '\"e\"' '$HOME;x' ''"
        );
        // A command line goes to the session's shell as one quoted argument.
        assert_eq!(
            exec_line(
                &ExecCommand::ShellLine("echo 'a' | wc -c".into()),
                "/r/bin/zsh"
            ),
            "/r/bin/zsh -c 'echo '\\''a'\\'' | wc -c'"
        );
    }

    /// What `sh` makes of an [`exec_line`]: the round trip through a real shell is
    /// the actual contract.
    #[test]
    fn quoting_roundtrips_through_a_real_shell() {
        let args = [
            "a b",
            "c'd",
            "\"e\"",
            "$HOME;x",
            "",
            "tab\there",
            "nl\nx",
            "*",
        ];
        let line = exec_line(
            &ExecCommand::Argv(
                std::iter::once("printf")
                    .chain(std::iter::once("[%s]"))
                    .chain(args)
                    .map(str::to_string)
                    .collect(),
            ),
            "sh",
        );
        let out = std::process::Command::new("sh")
            .args(["-c", &format!("exec {line}")])
            .output()
            .unwrap();
        let expected: String = args
            .iter()
            .map(|a| format!("[{a}]"))
            .collect::<Vec<_>>()
            .concat();
        assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
    }

    #[tokio::test]
    async fn run_exec_streams_the_command_under_the_cleanup_trap() {
        let _env = no_shell_packages();
        let t = MockTransport {
            host_shells: vec!["sh"],
            ..Default::default()
        };
        let log = t.commands.clone();
        let env = vec![minimal_env_component("gz").unwrap()];
        let mut s = Session::establish(
            t,
            &ssh_target("h"),
            &eff("sh"),
            &env,
            &[],
            silent_progress(),
        )
        .await
        .unwrap();

        let cmd = ExecCommand::Argv(vec!["echo".into(), "it's".into()]);
        // The transport's exit code is the session's (004 §FR-004).
        assert_eq!(s.run_exec(&cmd, false).await.unwrap(), 7);

        let log = log.lock().unwrap();
        let sent = log.last().unwrap();
        // Same bootstrap `run` (cleanup trap) as the interactive shell (C-X8)…
        assert!(sent.contains("/boot.sh run "), "got: {sent}");
        // …with the command's pid recorded right before the exec (C-X9)…
        let marker = format!(
            "echo $$ >/home/mock/.xxh/sessions/{}.cmd; exec ",
            s.session_id
        );
        assert!(sent.contains(&marker), "got: {sent}");
        // …and the argv quoted twice: once for the command, once for `sh -c '…'`.
        assert!(
            sent.ends_with("exec '\\''echo'\\'' '\\''it'\\''\\'\\'''\\''s'\\'''"),
            "got: {sent}"
        );
    }
}
