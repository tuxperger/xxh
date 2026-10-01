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
pub(crate) const BOOTSTRAP_SH: &str = include_str!("../../../bootstrap/bootstrap.sh");

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

/// What a login delivers to a host of a given platform, in delivery order
/// (005 research R5). `status` and `clean --stale` compare a host against it, so
/// the report can never disagree with what the next login does.
pub struct Plan {
    pub components: Vec<Component>,
    launch: ShellLaunch,
    /// The shell package has no build for this platform (the builds it has):
    /// the host's own shell must stand in (006 §FR-009).
    missing_build: Option<Vec<String>>,
    /// Plugins that target this platform, in resolved order.
    pub plugins: Vec<SessionPlugin>,
}

/// Detect the target's platform by streaming the bootstrap script over stdin —
/// nothing is written to the target (C-B1).
pub async fn detect_platform<T: Transport + ?Sized>(
    transport: &mut T,
) -> Result<Platform, SessionError> {
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
    Ok(Platform::parse_detect(&detect.stdout_str())?)
}

/// The components a login would deliver to `platform`: the packaged shell if one
/// exists locally for it, the environment components, and every plugin whose
/// target mask admits the platform (a skipped plugin is reported, C-M5). Only
/// addresses are computed — nothing is packed (023 C-A3).
pub fn plan_components(
    platform: &Platform,
    eff: &Effective,
    env_components: &[Component],
    plugins: &[SessionPlugin],
    progress: Progress<'_>,
) -> Result<Plan, SessionError> {
    let fmt = platform.preferred_archive_fmt();
    let mut components: Vec<Component> = Vec::new();
    let mut missing_build = None;
    let launch = match shellpkg::lookup(&eff.shell, platform)? {
        shellpkg::ShellLookup::Found(pkg) => {
            let comp = Component::pack_dir(ComponentKind::Shell, &pkg.tree, fmt)?
                .with_label(format!("shell {}", eff.shell));
            let launch = ShellLaunch::Packaged {
                hash: comp.hash.clone(),
                bin_rel: pkg.bin_rel,
            };
            components.push(comp);
            launch
        }
        shellpkg::ShellLookup::NoBuild { available } => {
            missing_build = Some(available);
            ShellLaunch::HostBinary(eff.shell.clone())
        }
        shellpkg::ShellLookup::NotInstalled => ShellLaunch::HostBinary(eff.shell.clone()),
    };
    components.extend(env_components.iter().cloned());

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
        components.push(
            Component::pack_dir(ComponentKind::Plugin, &p.dir, fmt)?
                .with_label(format!("plugin {}", p.manifest.name)),
        );
        active.push(p.clone());
    }
    if !active.is_empty() {
        progress(&format!("plugins: {} active", active.len()));
    }
    Ok(Plan {
        components,
        launch,
        missing_build,
        plugins: active,
    })
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
    /// Things the user should know about this session (006 §FR-009).
    notes: Vec<String>,
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
        let platform = detect_platform(&mut transport).await?;

        // 2) Resolve the requested shell and the plugins BEFORE any write to the
        //    host (§FR-011): a local package payload wins; otherwise the shell must
        //    already exist on the host; otherwise fail with a clear shell-class error.
        let Plan {
            components,
            launch,
            missing_build,
            plugins: active,
        } = plan_components(&platform, eff, env_components, plugins, progress)?;
        let mut notes = Vec::new();
        if let ShellLaunch::HostBinary(name) = &launch {
            let probe = transport
                .exec(&format!("command -v {name} >/dev/null 2>&1"))
                .await?;
            // A package without this platform's build is said out loud either
            // way, not just in the debug log (006 §FR-009, C-D7/C-D8).
            match (probe.exit_code == 0, missing_build) {
                (true, Some(available)) => notes.push(format!(
                    "the {name} package has no build for {t} ({}); using the host's {name} \
                     (`xxh shell fetch {name} --platform {t}` brings the package's)",
                    if available.is_empty() {
                        "no builds yet".to_string()
                    } else {
                        format!("has {}", available.join(", "))
                    },
                    t = platform.target_key(),
                )),
                (true, None) => {}
                (false, Some(available)) => {
                    return Err(ShellError::NoBuild {
                        shell: name.clone(),
                        target: platform.target_key(),
                        available,
                    }
                    .into());
                }
                (false, None) => return Err(ShellError::NotAvailable(name.clone()).into()),
            }
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

        // Sweep stale artefacts from a previously crashed session (§FR-006) —
        // streamed and *before* the script is installed: the sweep may remove
        // the whole root, and with it an installed boot.sh (014 found later
        // calls silently failing after a crash). Then install the script.
        transport
            .upload_stream(
                &format!("env XXH_ROOT={remote_root} sh -s -- reconcile"),
                BOOTSTRAP_SH.as_bytes().to_vec(),
            )
            .await?;
        transport
            .upload_stream(
                &format!("mkdir -p {remote_root} && cat > {remote_boot} && chmod +x {remote_boot}"),
                BOOTSTRAP_SH.as_bytes().to_vec(),
            )
            .await?;

        // 4) Nothing kept is trusted unchecked (014): the target describes every
        //    component this session needs and only those matching the client's
        //    copy are reused. The rest is discarded and sent again — before any
        //    of it is sourced or run (§FR-001/002).
        let all: Vec<&str> = components.iter().map(|c| c.hash.as_str()).collect();
        let verify = verify_on_target(&mut transport, &boot, &all).await?;
        if !verify.root_own {
            return Err(ShellError::Other(format!(
                "the environment directory {remote_root} belongs to another user; \
                 refusing to use it (remove it or log in as its owner)"
            ))
            .into());
        }
        let mut distrust = false;
        // Only a root that holds something can have been tampered with: a root
        // just created under a permissive umask (docker exec's 0000) is merely
        // narrowed by list-cache below.
        let holds_kept = verify.components.values().any(Option::is_some);
        if verify.root_too_open() && holds_kept {
            notes.push(format!(
                "the environment directory {remote_root} was writable by others ({}); \
                 its permissions are narrowed and kept components sent again",
                verify.root_perm
            ));
            distrust = true;
        }
        if let Some(tool) = &verify.unverifiable {
            notes.push(format!(
                "kept components cannot be checked on this target (no `{tool}`); \
                 they are sent again instead of trusted"
            ));
            distrust = true;
        }
        let mut host_hashes = BTreeSet::new();
        let mut discard: Vec<&str> = Vec::new();
        for comp in &components {
            if distrust {
                discard.push(&comp.hash);
                continue;
            }
            let Some(Some(actual)) = verify.components.get(&comp.hash) else {
                continue; // not on the target
            };
            match crate::integrity::compare(&comp.expected_listing()?, actual) {
                None => {
                    host_hashes.insert(comp.hash.clone());
                }
                Some(why) => {
                    notes.push(format!(
                        "the kept {} on the target was modified ({why}); it is sent again",
                        comp.label
                    ));
                    discard.push(&comp.hash);
                }
            }
        }
        if !discard.is_empty() {
            transport
                .exec(&boot(&format!("discard {}", discard.join(" "))))
                .await?;
        }

        // 5) Deliver what is missing or was discarded (§FR-013, VI); list-cache
        //    also narrows the root to its owner.
        list_cache(&mut transport, &boot("list-cache")).await?;
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
        for comp in &to_send {
            let out = transport
                .upload_stream(
                    &boot(&format!("recv {} {}", comp.hash, comp.fmt)),
                    comp.payload()?,
                )
                .await?;
            // A failed unpack is never a delivery (014 C-V10, US2).
            if out.exit_code != 0 {
                return Err(component_error(
                    comp,
                    &format!(
                        "unpacking it on the target failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    ),
                ));
            }
        }
        // What was just delivered is checked too (§FR-004); a second mismatch
        // means something on the target keeps changing it (C-V7).
        if !to_send.is_empty() && verify.unverifiable.is_none() {
            let sent: Vec<&str> = to_send.iter().map(|c| c.hash.as_str()).collect();
            let check = verify_on_target(&mut transport, &boot, &sent).await?;
            for comp in &to_send {
                let why = match check.components.get(&comp.hash) {
                    Some(Some(actual)) => {
                        crate::integrity::compare(&comp.expected_listing()?, actual)
                    }
                    _ => Some("it is missing after delivery".to_string()),
                };
                if let Some(why) = why {
                    return Err(component_error(
                        comp,
                        &format!("it does not match on the target after delivery: {why}"),
                    ));
                }
            }
        }
        run_stage_hooks(&active, LifecycleStage::PostDeploy, progress).await;

        // 5) Assemble the prelude in delivery order: packaged shells extend PATH,
        //    config/plugin components contribute their env.sh. A failed `.` is
        //    fatal to a POSIX sh (dash, BusyBox: exit 2, `|| true` never runs),
        //    so a component without env.sh is skipped, not sourced (013 found
        //    packages without one killing every login).
        let source = |h: &str| {
            format!(
                "XXH_COMPONENT_DIR={root}/cache/{h}; export XXH_COMPONENT_DIR; \
                 if [ -f {root}/cache/{h}/env.sh ]; then . {root}/cache/{h}/env.sh; fi; ",
                root = remote_root
            )
        };
        let mut prelude = String::new();
        for comp in &components {
            if comp.kind == ComponentKind::Shell {
                // Shell packages extend PATH and may ship an env.sh of their own
                // (e.g. zsh-bin exports FPATH at its delivered functions dir).
                prelude.push_str(&format!(
                    "export PATH=\"{remote_root}/cache/{h}/bin:$PATH\"; ",
                    h = comp.hash
                ));
            }
            // XXH_COMPONENT_DIR lets an env.sh reference its own cache dir
            // (PATH for tool packages, TERMINFO/SSL_CERT_FILE for nix ones).
            prelude.push_str(&source(&comp.hash));
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
            notes,
            plugins: active,
        })
    }

    /// Detected host platform.
    pub fn platform(&self) -> &Platform {
        &self.platform
    }

    /// Warnings worth showing even when stage progress is silent — e.g. a shell
    /// package without a build for this platform (006 C-D7).
    pub fn notes(&self) -> &[String] {
        &self.notes
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
        // `exec`, which would treat `XXH_ROOT=…` as the command name. XXH_NOW is
        // the client clock, recorded as the kept environment's last use (005 C-R9).
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        format!(
            "env XXH_ROOT={root} XXH_NOW={now} sh {root}/boot.sh run {} {keep} sh -c '{}{}exec {}'",
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

/// Ask the target to describe `hashes` (014 C-V1..C-V3).
async fn verify_on_target<T: Transport>(
    t: &mut T,
    boot: &dyn Fn(&str) -> String,
    hashes: &[&str],
) -> Result<crate::integrity::VerifyReply, SessionError> {
    let out = t
        .exec(&boot(&format!("verify {}", hashes.join(" "))))
        .await?;
    if out.exit_code != 0 {
        return Err(ShellError::Other(format!(
            "checking the kept environment failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .into());
    }
    crate::integrity::parse_verify(&String::from_utf8_lossy(&out.stdout))
}

/// A delivery problem in the class of the component it hit (014 US2): a
/// plugin's is a plugin error, everything else a shell error.
fn component_error(comp: &Component, why: &str) -> SessionError {
    let msg = format!("{} ({}): {why}", comp.label, &comp.hash[..12]);
    match comp.kind {
        ComponentKind::Plugin => PluginError::Other(msg).into(),
        ComponentKind::Shell | ComponentKind::Config => ShellError::Other(msg).into(),
    }
}

fn session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("s{n:x}")
}

/// A fresh temporary directory path, unique per call: generated components are
/// built concurrently (tests, several targets), and a shared per-process name lets
/// one call delete the tree another is still hashing.
fn scratch_dir(prefix: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{prefix}-{}-{n}", std::process::id()))
}

/// Build a minimal environment component that marks the xxh session and adds a demo
/// alias, proving config delivery end-to-end. Real dotfiles/plugins/shell packages
/// extend this set (T020, US4).
pub fn minimal_env_component(fmt: &str) -> Result<Component, ShellError> {
    let dir = scratch_dir("xxh-env");
    std::fs::create_dir_all(&dir).map_err(|e| ShellError::Other(e.to_string()))?;
    std::fs::write(
        dir.join("env.sh"),
        b"export XXH_SESSION=1\nalias xxh-hello='echo hello-from-xxh'\n",
    )
    .map_err(|e| ShellError::Other(e.to_string()))?;
    let comp =
        Component::pack_dir_eager(ComponentKind::Config, &dir, fmt).map(|c| c.with_label("env"));
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

    let dir = scratch_dir("xxh-ti");
    let entry_dir = dir.join("terminfo").join(first.to_string());
    std::fs::create_dir_all(&entry_dir).ok()?;
    std::fs::copy(&src, entry_dir.join(&term)).ok()?;
    // Trailing empty element keeps the host's compiled-in default search path.
    std::fs::write(
        dir.join("env.sh"),
        "export TERMINFO_DIRS=\"$XXH_COMPONENT_DIR/terminfo:${TERMINFO_DIRS:-}\"\n",
    )
    .ok()?;
    let comp = Component::pack_dir_eager(ComponentKind::Config, &dir, fmt)
        .ok()
        .map(|c| c.with_label("terminfo"));
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
        /// Component addresses the mock host reports as already cached.
        host_cache: Vec<String>,
        commands: Arc<Mutex<Vec<String>>>,
        /// What the mock target's cache holds, as `verify` describes it (014):
        /// filled by tests for kept components and by `recv` for delivered ones.
        kept: Arc<Mutex<BTreeMap<String, crate::integrity::Listing>>>,
        /// The `root` line of `verify` (default: owned, 0700).
        root_line: Option<&'static str>,
        /// `verify` answers `unverifiable` (no sha256sum on the target).
        unverifiable: bool,
        /// Every delivered component is corrupted on arrival.
        corrupt_on_recv: bool,
        /// `recv` fails with this exit code.
        recv_fails: bool,
    }

    /// The `verify` answer for one kept component.
    fn listing_reply(hash: &str, l: &crate::integrity::Listing) -> String {
        let mut s = format!("component\t{hash}\n");
        for d in &l.dirs {
            s.push_str(&format!("d\t{d}\n"));
        }
        for x in &l.execs {
            s.push_str(&format!("x\t{x}\n"));
        }
        for o in &l.others {
            s.push_str(&format!("o\t{o}\n"));
        }
        for (p, sha) in &l.files {
            s.push_str(&format!("f\t{sha}\t{p}\n"));
        }
        s
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
            if cmd.contains("list-cache") {
                return Ok(Self::ok(&self.host_cache.join("\n")));
            }
            if let Some(args) = cmd.split(" verify ").nth(1) {
                let mut out = format!("root\t{}\n", self.root_line.unwrap_or("own\tdrwx------"));
                if self.unverifiable {
                    out.push_str("unverifiable\tsha256sum\n");
                    return Ok(Self::ok(&out));
                }
                let kept = self.kept.lock().unwrap();
                for h in args.split_whitespace() {
                    match kept.get(h) {
                        Some(l) => out.push_str(&listing_reply(h, l)),
                        None => out.push_str(&format!("missing\t{h}\n")),
                    }
                }
                return Ok(Self::ok(&out));
            }
            if let Some(args) = cmd.split(" discard ").nth(1) {
                let mut kept = self.kept.lock().unwrap();
                for h in args.split_whitespace() {
                    kept.remove(h);
                }
            }
            Ok(Self::ok(""))
        }
        async fn upload_stream(
            &mut self,
            cmd: &str,
            data: Vec<u8>,
        ) -> Result<ExecOutput, TransportError> {
            self.commands.lock().unwrap().push(cmd.to_string());
            if let Some(args) = cmd.split(" recv ").nth(1) {
                if self.recv_fails {
                    return Ok(ExecOutput {
                        exit_code: 1,
                        stdout: vec![],
                        stderr: b"tar: short read".to_vec(),
                    });
                }
                let mut it = args.split_whitespace();
                let (h, fmt) = (it.next().unwrap(), it.next().unwrap_or("gz"));
                let mut l = crate::deploy::listing_from_archive(&data, fmt).unwrap();
                if self.corrupt_on_recv {
                    l.files.insert("./planted".into(), "0".repeat(64));
                }
                self.kept.lock().unwrap().insert(h.to_string(), l);
                return Ok(Self::ok(""));
            }
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

    /// A component the host already holds is neither packed nor sent (023 C-A4):
    /// its address is enough to know.
    #[tokio::test]
    async fn component_present_on_the_host_is_not_sent() {
        let _env = no_shell_packages();
        let dir = std::env::temp_dir().join(format!("xxh-present-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = "name = \"cached\"\nversion = \"1.0.0\"\napi_version = \"1.0.0\"\n";
        std::fs::write(dir.join("plugin.toml"), text).unwrap();
        let plugin = SessionPlugin {
            manifest: Manifest::parse(text).unwrap(),
            dir: dir.clone(),
        };
        let address = crate::deploy::tree_hash(&dir).unwrap();
        // Kept intact on the target (014: verified, then reused).
        let intact = Component::pack_dir(ComponentKind::Plugin, &dir, "gz")
            .unwrap()
            .expected_listing()
            .unwrap();

        let t = MockTransport {
            host_shells: vec!["sh"],
            host_cache: vec![address.clone()],
            ..Default::default()
        };
        t.kept.lock().unwrap().insert(address.clone(), intact);
        let log = t.commands.clone();
        let env = vec![minimal_env_component("gz").unwrap()];
        let s = Session::establish(
            t,
            &ssh_target("h"),
            &eff("sh"),
            &env,
            &[plugin],
            silent_progress(),
        )
        .await
        .unwrap();

        assert_eq!(
            s.delivery_report(),
            DeliveryReport {
                delivered: 1,
                reused: 1
            },
            "only the env component is new to this host"
        );
        let log = log.lock().unwrap();
        assert!(
            !log.iter().any(|c| c.contains(&format!("recv {address}"))),
            "the cached component must not be re-sent: {log:?}"
        );
        // …but it is still wired into the session from the host cache.
        assert!(s.prelude.contains(&address));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Generated components get the same address on every client run, so a kept
    /// environment reuses them (023 C-A8).
    #[test]
    fn generated_components_have_stable_addresses() {
        let a = minimal_env_component("gz").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let b = minimal_env_component("zst").unwrap();
        assert_eq!(a.hash, b.hash);
    }

    /// The plan is exactly what a login delivers: a plugin that does not target
    /// the platform is left out, and every planned address is the one `establish`
    /// sends (005 T008, research R5).
    #[tokio::test]
    async fn plan_matches_what_establish_delivers() {
        let _env = no_shell_packages();
        let base = std::env::temp_dir().join(format!("xxh-plan-{}", std::process::id()));
        let plugin = |name: &str, targets: &str| {
            let dir = base.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let text = format!(
                "name = \"{name}\"\nversion = \"1.0.0\"\napi_version = \"1.0.0\"\n{targets}"
            );
            std::fs::write(dir.join("plugin.toml"), &text).unwrap();
            SessionPlugin {
                manifest: Manifest::parse(&text).unwrap(),
                dir,
            }
        };
        let plugins = vec![
            plugin("anywhere", ""),
            plugin("arm-only", "targets = [\"linux/aarch64\"]\n"),
        ];
        let env = vec![minimal_env_component("gz").unwrap()];
        let platform = Platform::parse_detect("Linux x86_64 | tar gzip").unwrap();

        let plan =
            plan_components(&platform, &eff("sh"), &env, &plugins, silent_progress()).unwrap();
        let labels: Vec<&str> = plan.components.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["env", "plugin anywhere"]);
        assert_eq!(plan.plugins.len(), 1);

        let t = MockTransport {
            host_shells: vec!["sh"],
            ..Default::default()
        };
        let log = t.commands.clone();
        Session::establish(
            t,
            &ssh_target("h"),
            &eff("sh"),
            &env,
            &plugins,
            silent_progress(),
        )
        .await
        .unwrap();
        let sent: Vec<String> = log
            .lock()
            .unwrap()
            .iter()
            .filter_map(|c| c.split("recv ").nth(1))
            .map(|rest| rest.split(' ').next().unwrap().to_string())
            .collect();
        let planned: Vec<String> = plan.components.iter().map(|c| c.hash.clone()).collect();
        assert_eq!(sent, planned);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A shell package without a build for the target's platform is not silent:
    /// a note when the host has the shell, a shell-class error naming the builds
    /// when it does not (006 T007, §FR-009, C-D7/C-D8).
    #[tokio::test]
    async fn missing_shell_build_is_visible() {
        let base = std::env::temp_dir().join(format!("xxh-nobuild-{}", std::process::id()));
        let dir = base.join("zsh");
        std::fs::create_dir_all(dir.join("dist/linux-aarch64/bin")).unwrap();
        std::fs::write(dir.join("dist/linux-aarch64/bin/zsh"), b"#!/bin/sh\n").unwrap();
        std::fs::write(
            dir.join("manifest.toml"),
            "name = \"zsh\"\nversion = \"1.0.0\"\napi_version = \"1.0.0\"\n\
             [provides]\nshell = \"zsh\"\n",
        )
        .unwrap();
        let _g = crate::shellpkg::testenv::shells_dir(&base);
        let env = vec![minimal_env_component("gz").unwrap()];

        let with_zsh = MockTransport {
            host_shells: vec!["zsh"],
            ..Default::default()
        };
        let s = Session::establish(
            with_zsh,
            &ssh_target("h"),
            &eff("zsh"),
            &env,
            &[],
            silent_progress(),
        )
        .await
        .unwrap();
        assert_eq!(s.notes().len(), 1);
        assert!(
            s.notes()[0].contains("no build for linux-x86_64 (has linux-aarch64)"),
            "{:?}",
            s.notes()
        );

        let without = MockTransport::default();
        let log = without.commands.clone();
        let err = Session::establish(
            without,
            &ssh_target("h"),
            &eff("zsh"),
            &env,
            &[],
            silent_progress(),
        )
        .await
        .err()
        .expect("no shell anywhere");
        assert!(
            matches!(err, SessionError::Shell(ShellError::NoBuild { .. })),
            "{err:?}"
        );
        assert!(err.to_string().contains("has linux-aarch64"), "{err}");
        // Still decided before anything is written (§FR-011).
        assert!(
            !log.lock().unwrap().iter().any(|c| c.contains("recv")),
            "nothing delivered"
        );
        drop(_g);
        let _ = std::fs::remove_dir_all(&base);
    }

    async fn login(t: MockTransport) -> Result<Session<MockTransport>, SessionError> {
        let env = vec![minimal_env_component("gz").unwrap()];
        Session::establish(
            t,
            &ssh_target("h"),
            &eff("sh"),
            &env,
            &[],
            silent_progress(),
        )
        .await
    }

    fn env_listing() -> (String, crate::integrity::Listing) {
        let env = minimal_env_component("gz").unwrap();
        (env.hash.clone(), env.expected_listing().unwrap())
    }

    /// A kept component that was modified is named, discarded and sent again;
    /// an intact one is reused (014 T005, §FR-001..003, §FR-008).
    #[tokio::test]
    async fn tampered_kept_component_is_replaced() {
        let _env = no_shell_packages();
        let (hash, mut listing) = env_listing();
        listing.files.insert("./env.sh".into(), "e".repeat(64));
        let t = MockTransport {
            host_shells: vec!["sh"],
            ..Default::default()
        };
        t.kept.lock().unwrap().insert(hash.clone(), listing);
        let log = t.commands.clone();
        let s = login(t).await.unwrap();
        assert_eq!(s.delivery_report().delivered, 1);
        assert!(
            s.notes()
                .iter()
                .any(|n| n.contains("env") && n.contains("./env.sh was changed")),
            "{:?}",
            s.notes()
        );
        let log = log.lock().unwrap();
        let discard = log
            .iter()
            .position(|c| c.contains(&format!("discard {hash}")));
        let recv = log.iter().position(|c| c.contains(&format!("recv {hash}")));
        assert!(
            discard.is_some() && recv.is_some() && discard < recv,
            "{log:?}"
        );
    }

    /// Without the tools to check, nothing kept is trusted (014 §FR-006).
    #[tokio::test]
    async fn unverifiable_target_trusts_nothing_kept() {
        let _env = no_shell_packages();
        let (hash, listing) = env_listing();
        let t = MockTransport {
            host_shells: vec!["sh"],
            unverifiable: true,
            ..Default::default()
        };
        t.kept.lock().unwrap().insert(hash, listing);
        let s = login(t).await.unwrap();
        assert_eq!(s.delivery_report().delivered, 1, "re-sent, not reused");
        assert!(s.notes().iter().any(|n| n.contains("cannot be checked")));
    }

    /// Someone else's root is refused before anything is sent; a root open to
    /// others is narrowed and its content re-sent (014 §FR-005).
    #[tokio::test]
    async fn environment_root_ownership_and_permissions() {
        let _env = no_shell_packages();
        let t = MockTransport {
            host_shells: vec!["sh"],
            root_line: Some("foreign\tdrwx------"),
            ..Default::default()
        };
        let log = t.commands.clone();
        let err = login(t).await.err().expect("foreign root");
        assert!(err.to_string().contains("another user"), "{err}");
        assert!(!log.lock().unwrap().iter().any(|c| c.contains("recv")));

        let (hash, listing) = env_listing();
        let t = MockTransport {
            host_shells: vec!["sh"],
            root_line: Some("own\tdrwxrwxrwx"),
            ..Default::default()
        };
        t.kept.lock().unwrap().insert(hash, listing);
        let s = login(t).await.unwrap();
        assert_eq!(s.delivery_report().delivered, 1);
        assert!(s.notes().iter().any(|n| n.contains("writable by others")));

        // A fresh root under a permissive umask holds nothing to distrust.
        let t = MockTransport {
            host_shells: vec!["sh"],
            root_line: Some("own\tdrwxrwxrwx"),
            ..Default::default()
        };
        let s = login(t).await.unwrap();
        assert!(s.notes().is_empty(), "{:?}", s.notes());
    }

    /// A failed unpack and a delivery that does not match are errors in the
    /// component's class (014 T008, C-V7, C-V10).
    #[tokio::test]
    async fn failed_or_corrupted_delivery_is_an_error() {
        let _env = no_shell_packages();
        let t = MockTransport {
            host_shells: vec!["sh"],
            recv_fails: true,
            ..Default::default()
        };
        let err = login(t).await.err().expect("recv failed");
        assert!(matches!(err, SessionError::Shell(_)), "{err:?}");
        assert!(err.to_string().contains("short read"), "{err}");

        let t = MockTransport {
            host_shells: vec!["sh"],
            corrupt_on_recv: true,
            ..Default::default()
        };
        let err = login(t).await.err().expect("corrupted on arrival");
        assert!(
            err.to_string().contains("after delivery") && err.to_string().contains("env"),
            "{err}"
        );

        // A plugin's delivery problem is a plugin error.
        let dir = std::env::temp_dir().join(format!("xxh-int-plugin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = "name = \"p\"\nversion = \"1.0.0\"\napi_version = \"1.0.0\"\n";
        std::fs::write(dir.join("plugin.toml"), text).unwrap();
        let plugin = Component::pack_dir(ComponentKind::Plugin, &dir, "gz")
            .unwrap()
            .with_label("plugin p");
        let e = component_error(&plugin, "boom");
        assert!(matches!(e, SessionError::Plugin(_)), "{e:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
