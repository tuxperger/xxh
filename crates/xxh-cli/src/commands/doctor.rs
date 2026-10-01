//! `xxh doctor [target]` — what would stop a login, found before trying one
//! (006 T008/T012; contracts/cli-doctor.md).
//!
//! Client checks need no network: configuration, enabled plugins, the shell
//! package, plugin sources and container runtimes (research R5). With a target,
//! the target is diagnosed read-only by `xxh_core::doctor` through the same
//! transport factory a login uses. Rendering is pure for unit tests.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::Path;

use xxh_config::Effective;
use xxh_core::doctor::{Check, CheckStatus, TargetReport, diagnose_target};
use xxh_core::session::{Progress, SessionError, SessionPlugin};
use xxh_plugins::registry::Registry;
use xxh_plugins::resolver;
use xxh_transport::{AuthPolicy, ResolvedTarget};

use super::connect::env_components;
use super::target_io::open_transport;

/// The whole diagnosis (data-model.md).
pub struct DoctorReport {
    pub client: Vec<Check>,
    pub target: Option<TargetReport>,
}

impl DoctorReport {
    pub fn failed(&self) -> bool {
        self.checks().any(|c| c.status == CheckStatus::Fail)
    }
    fn checks(&self) -> impl Iterator<Item = &Check> {
        self.client
            .iter()
            .chain(self.target.iter().flat_map(|t| t.checks.iter()))
    }
}

/// What the client checks look at, gathered by the caller so tests can point
/// them at a scratch registry and `PATH`.
pub struct ClientEnv {
    pub config: Result<Effective, String>,
    pub registry: Result<Registry, String>,
    pub path: Option<OsString>,
}

/// An executable named `prog` on `path` — nothing is run (research R5).
fn in_path(path: Option<&OsString>, prog: &str) -> bool {
    let Some(path) = path else {
        return false;
    };
    std::env::split_paths(path).any(|dir| is_executable(&dir.join(prog)))
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// Client checks (§FR-003) and the enabled plugins that are installed — the
/// ones a login would take, for the target's compatibility check.
pub fn client_checks(env: &ClientEnv) -> (Vec<Check>, Vec<SessionPlugin>) {
    let mut checks = Vec::new();
    let eff = match &env.config {
        Ok(eff) => {
            checks.push(Check::pass("config", "configuration is valid"));
            eff
        }
        Err(e) => {
            checks.push(Check::fail(
                "config",
                format!("configuration is invalid: {e}"),
                "fix the file named above; `xxh config` prints its location",
            ));
            return (checks, Vec::new());
        }
    };

    let plugins = plugin_checks(eff, &env.registry, &mut checks);
    checks.push(shell_check(&eff.shell));

    let path = env.path.as_ref();
    let mut sources = vec![("git", "git plugins")];
    if cfg!(feature = "nix-source") {
        sources.push(("nix", "nixpkgs and flake plugins"));
    }
    for (prog, what) in sources {
        if in_path(path, prog) {
            checks.push(Check::pass(
                format!("source:{prog}"),
                format!("{prog} is available"),
            ));
        } else {
            checks.push(Check::warn(
                format!("source:{prog}"),
                format!("{prog} is not installed: {what} cannot be added or updated"),
                format!("install {prog} if you use {what}; installed plugins keep working"),
            ));
        }
    }

    let runtimes: Vec<&str> = ["docker", "podman"]
        .into_iter()
        .filter(|r| in_path(path, r))
        .collect();
    if runtimes.is_empty() {
        checks.push(Check::warn(
            "runtime",
            "neither docker nor podman is installed: container targets are unavailable",
            "install docker or podman to use docker:/podman:/container: targets",
        ));
    } else {
        checks.push(Check::pass(
            "runtime",
            format!("container runtime: {}", runtimes.join(", ")),
        ));
    }
    (checks, plugins)
}

fn plugin_checks(
    eff: &Effective,
    registry: &Result<Registry, String>,
    checks: &mut Vec<Check>,
) -> Vec<SessionPlugin> {
    if eff.enabled_plugins.is_empty() {
        checks.push(Check::pass("plugins", "no plugins enabled"));
        return Vec::new();
    }
    let registry = match registry {
        Ok(r) => r,
        Err(e) => {
            checks.push(Check::fail(
                "plugins",
                format!("the plugin registry cannot be read: {e}"),
                "check the permissions of ~/.local/share/xxh/plugins",
            ));
            return Vec::new();
        }
    };
    let mut installed = Vec::new();
    for name in &eff.enabled_plugins {
        match (registry.manifest(name), registry.package_dir(name)) {
            (Ok(manifest), Ok(dir)) => installed.push(SessionPlugin { manifest, dir }),
            _ => checks.push(Check::fail(
                format!("plugin:{name}"),
                format!("plugin {name} is enabled but not installed"),
                format!(
                    "install it with `xxh plugin add <source>`, or disable it with \
                     `xxh plugin disable {name}`"
                ),
            )),
        }
    }
    let manifests: Vec<_> = installed.iter().map(|p| p.manifest.clone()).collect();
    match resolver::resolve(&manifests) {
        Ok(_) if installed.len() == eff.enabled_plugins.len() => checks.push(Check::pass(
            "plugins",
            format!(
                "{} enabled plugin(s) installed and resolvable",
                installed.len()
            ),
        )),
        Ok(_) => {}
        Err(e) => checks.push(Check::fail(
            "plugins",
            format!("enabled plugins cannot be ordered: {e}"),
            "install the missing dependency or disable the plugin that needs it",
        )),
    }
    installed
}

fn shell_check(shell: &str) -> Check {
    if shell == "sh" {
        return Check::pass("shell", "sh: every target has one");
    }
    match xxh_core::shellpkg::available_targets(shell) {
        Ok(Some(t)) if !t.is_empty() => {
            Check::pass("shell", format!("{shell} package builds: {}", t.join(", ")))
        }
        Ok(Some(_)) => Check::warn(
            "shell",
            format!("the {shell} package has no builds yet: the target's own {shell} is needed"),
            format!("run the {shell} package's fetch.sh for the platforms you log into"),
        ),
        Ok(None) => Check::warn(
            "shell",
            format!("no {shell} package installed: only targets that have {shell} will work"),
            format!(
                "install a {shell} shell package into ~/.local/share/xxh/shells to bring \
                 {shell} along"
            ),
        ),
        Err(e) => Check::fail(
            "shell",
            format!("the {shell} package is broken: {e}"),
            format!("reinstall the {shell} shell package"),
        ),
    }
}

/// Diagnose `target` (§FR-002): the error is returned only when the target cannot
/// be reached — the caller still prints the client half (C-D6).
pub async fn target_report(
    target: ResolvedTarget,
    eff: &Effective,
    plugins: &[SessionPlugin],
    progress: Progress<'_>,
) -> Result<TargetReport, SessionError> {
    let label = target.label().to_string();
    let (mut transport, target) = open_transport(target, eff, progress).await?;
    progress(&format!("connect {label}"));
    transport.connect(&target, &AuthPolicy::default()).await?;
    let env = env_components()?;
    let report = diagnose_target(&mut *transport, &label, eff, &env, plugins).await;
    let _ = transport.disconnect().await;
    report
}

/// The check for an unreachable target (C-D6).
pub fn unreachable(label: &str, err: &SessionError) -> TargetReport {
    TargetReport {
        label: label.to_string(),
        platform: None,
        checks: vec![Check::fail(
            "connect",
            format!("cannot reach {label}: {err}"),
            "check the address, the network and your credentials (`xxh -v` shows details)",
        )],
    }
}

fn render_checks(out: &mut String, checks: &[Check]) {
    for c in checks {
        let tag = match c.status {
            CheckStatus::Pass => "ok",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "FAIL",
        };
        let _ = writeln!(out, "  {tag:<5} {}", c.message);
        if let Some(action) = &c.action {
            let _ = writeln!(out, "        → {action}");
        }
    }
}

/// The text report (C-D4).
pub fn render(report: &DoctorReport) -> String {
    let mut out = String::from("client\n");
    render_checks(&mut out, &report.client);
    if let Some(t) = &report.target {
        let platform = t.platform.as_deref().unwrap_or("platform unknown");
        let _ = writeln!(out, "target {} ({platform})", t.label);
        render_checks(&mut out, &t.checks);
    }
    let count = |s| report.checks().filter(|c| c.status == s).count();
    let _ = writeln!(
        out,
        "{} passed, {} warnings, {} failed",
        count(CheckStatus::Pass),
        count(CheckStatus::Warn),
        count(CheckStatus::Fail)
    );
    out
}

/// The JSON report (C-D5).
pub fn json(report: &DoctorReport) -> serde_json::Value {
    serde_json::json!({ "client": report.client, "target": report.target })
}

#[cfg(test)]
mod tests {
    use super::*;
    use xxh_config::{CleanupMode, RuntimeSetting, TransportBackend};

    fn eff(shell: &str, plugins: &[&str]) -> Effective {
        Effective {
            shell: shell.into(),
            enabled_plugins: plugins.iter().map(|p| p.to_string()).collect(),
            cleanup: CleanupMode::Ephemeral,
            transport: TransportBackend::Russh,
            connect_timeout_s: 10,
            user: None,
            identity: None,
            container_runtime: RuntimeSetting::Auto,
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("xxh-doctor-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn status_of(checks: &[Check], id: &str) -> Option<CheckStatus> {
        checks.iter().find(|c| c.id == id).map(|c| c.status)
    }

    /// US2 scenario 1: an enabled plugin that is not installed is a named failure
    /// with a way out.
    #[test]
    fn enabled_but_missing_plugin_fails_by_name() {
        let dir = scratch("reg");
        let env = ClientEnv {
            config: Ok(eff("sh", &["ghost"])),
            registry: Ok(Registry::open(&dir)),
            path: None,
        };
        let (checks, plugins) = client_checks(&env);
        assert!(plugins.is_empty());
        let c = checks.iter().find(|c| c.id == "plugin:ghost").unwrap();
        assert_eq!(c.status, CheckStatus::Fail);
        assert!(c.message.contains("ghost"));
        assert!(
            c.action
                .as_deref()
                .unwrap()
                .contains("xxh plugin disable ghost")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// US2 scenarios 2–3: missing optional tools only warn, so a sound setup
    /// passes (exit 0).
    #[test]
    fn missing_sources_and_runtimes_only_warn() {
        let empty = scratch("path");
        let env = ClientEnv {
            config: Ok(eff("sh", &[])),
            registry: Err("unused".into()),
            path: Some(empty.clone().into_os_string()),
        };
        let (checks, _) = client_checks(&env);
        assert_eq!(status_of(&checks, "source:git"), Some(CheckStatus::Warn));
        assert_eq!(status_of(&checks, "runtime"), Some(CheckStatus::Warn));
        assert_eq!(status_of(&checks, "shell"), Some(CheckStatus::Pass));
        let report = DoctorReport {
            client: checks,
            target: None,
        };
        assert!(!report.failed());

        // With git on PATH the source passes.
        let git = empty.join("git");
        std::fs::write(&git, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let (checks, _) = client_checks(&env);
        assert_eq!(status_of(&checks, "source:git"), Some(CheckStatus::Pass));
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn broken_config_stops_the_client_checks() {
        let env = ClientEnv {
            config: Err("line 3: unknown key".into()),
            registry: Err("unused".into()),
            path: None,
        };
        let (checks, _) = client_checks(&env);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Fail);
    }

    fn sample() -> DoctorReport {
        DoctorReport {
            client: vec![Check::pass("config", "configuration is valid")],
            target: Some(TargetReport {
                label: "web".into(),
                platform: Some("linux/x86_64/musl".into()),
                checks: vec![
                    Check::pass("platform", "linux/x86_64/musl is supported"),
                    Check::warn("tool:zstd", "`zstd` is missing", "optional"),
                    Check::fail("space", "needs 10 KiB, 5 free", "free 5 KiB"),
                ],
            }),
        }
    }

    #[test]
    fn text_report_shows_verdicts_actions_and_totals() {
        let text = render(&sample());
        assert!(
            text.starts_with("client\n  ok    configuration is valid\n"),
            "{text}"
        );
        assert!(text.contains("target web (linux/x86_64/musl)\n"), "{text}");
        assert!(text.contains("  warn  `zstd` is missing\n        → optional\n"));
        assert!(text.contains("  FAIL  needs 10 KiB, 5 free\n        → free 5 KiB\n"));
        assert!(text.ends_with("2 passed, 1 warnings, 1 failed\n"), "{text}");
        assert!(sample().failed());
    }

    #[test]
    fn json_report_follows_the_contract() {
        let v = json(&sample());
        assert_eq!(v["client"][0]["status"], "pass");
        assert!(v["client"][0]["action"].is_null());
        assert_eq!(v["target"]["label"], "web");
        assert_eq!(v["target"]["platform"], "linux/x86_64/musl");
        assert_eq!(v["target"]["checks"][2]["status"], "fail");
        assert_eq!(v["target"]["checks"][2]["action"], "free 5 KiB");
        let none = DoctorReport {
            client: vec![],
            target: None,
        };
        assert!(json(&none)["target"].is_null());
    }

    #[test]
    fn unreachable_target_is_a_failed_connect_check() {
        let err = SessionError::Transport(xxh_transport::TransportError::Connect("refused".into()));
        let t = unreachable("web", &err);
        assert_eq!(t.checks[0].id, "connect");
        assert_eq!(t.checks[0].status, CheckStatus::Fail);
        assert!(t.checks[0].message.contains("refused"));
    }
}
