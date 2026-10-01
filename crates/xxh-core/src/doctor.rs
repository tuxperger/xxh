//! Diagnosing a target before a login (006 T004–T006; §FR-002, §FR-004..006).
//!
//! The target is only *read*: streamed `detect`, `probe` and `status` calls and one
//! `command -v` for the shell (contracts/cli-doctor.md C-D3). Every verdict is a
//! pure function of those answers and of the login plan (005 `plan_components`),
//! so the diagnosis cannot disagree with what a login would do (SC-001), and each
//! failed or doubtful check names its cause and the fix (§FR-006).

use std::collections::BTreeSet;

use serde::Serialize;
use xxh_config::Effective;
use xxh_transport::Transport;

use crate::ShellError;
use crate::deploy::Component;
use crate::platform::Platform;
use crate::remote_env;
use crate::session::{
    BOOTSTRAP_SH, SessionError, SessionPlugin, detect_platform, plan_components, silent_progress,
};
use crate::shellpkg::{self, ShellLookup};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

/// One diagnosis line (data-model.md). `action` is set for every warn and fail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub id: String,
    pub status: CheckStatus,
    pub message: String,
    pub action: Option<String>,
}

impl Check {
    pub fn pass(id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            status: CheckStatus::Pass,
            message: message.into(),
            action: None,
        }
    }
    pub fn warn(
        id: impl Into<String>,
        message: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            status: CheckStatus::Warn,
            message: message.into(),
            action: Some(action.into()),
        }
    }
    pub fn fail(
        id: impl Into<String>,
        message: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            status: CheckStatus::Fail,
            message: message.into(),
            action: Some(action.into()),
        }
    }
}

/// The target half of a diagnosis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetReport {
    pub label: String,
    /// `os/arch/libc`, when detection succeeded.
    pub platform: Option<String>,
    pub checks: Vec<Check>,
}

/// What `probe` reported (contracts/bootstrap-probe.md).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    /// Tool name → its path on the target, in the order probed.
    pub tools: Vec<(String, Option<String>)>,
    /// The root a login would use, and whether it already exists.
    pub root: Option<(String, bool)>,
    pub free_kb: Option<u64>,
}

/// The host contract: a login fails without any of these.
pub const REQUIRED_TOOLS: [&str; 6] = ["sh", "cat", "mkdir", "chmod", "tar", "gzip"];

/// Tools a login does without, and what their absence costs.
const OPTIONAL_TOOLS: [(&str, &str); 3] = [
    ("zstd", "deliveries fall back to gzip: larger and slower"),
    ("du", "`xxh status` cannot show sizes"),
    ("df", "free space cannot be checked"),
];

/// Parse the `probe` reply (C-P2).
pub fn parse_probe(out: &str) -> Result<Probe, SessionError> {
    let bad = |line: &str| -> SessionError {
        ShellError::Other(format!("unexpected probe reply from the target: {line:?}")).into()
    };
    let mut p = Probe::default();
    for line in out.lines().filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.splitn(3, '\t').collect();
        match f.as_slice() {
            ["tool", name, path] => p.tools.push((
                (*name).to_string(),
                (*path != "-").then(|| (*path).to_string()),
            )),
            ["root", "-"] => p.root = None,
            ["root", kind @ ("existing" | "new"), path] => {
                p.root = Some(((*path).to_string(), *kind == "existing"));
            }
            ["free", kb] => p.free_kb = kb.parse().ok(),
            _ => return Err(bad(line)),
        }
    }
    Ok(p)
}

/// Ask the target what a login needs from it, writing nothing (C-P1).
pub async fn probe<T: Transport + ?Sized>(transport: &mut T) -> Result<Probe, SessionError> {
    let out = transport
        .upload_stream("sh -s -- probe", BOOTSTRAP_SH.as_bytes().to_vec())
        .await?;
    if out.exit_code != 0 {
        return Err(ShellError::Other(format!(
            "probing the target failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .into());
    }
    parse_probe(&String::from_utf8_lossy(&out.stdout))
}

fn has_tool(p: &Probe, name: &str) -> bool {
    p.tools.iter().any(|(n, path)| n == name && path.is_some())
}

/// Required tools: one pass line, or a failure per missing tool; optional ones:
/// a warning per missing tool with what it costs.
pub fn tool_checks(p: &Probe) -> Vec<Check> {
    let mut checks = Vec::new();
    let missing: Vec<&str> = REQUIRED_TOOLS
        .into_iter()
        .filter(|t| !has_tool(p, t))
        .collect();
    if missing.is_empty() {
        checks.push(Check::pass(
            "tools",
            format!("required tools present: {}", REQUIRED_TOOLS.join(", ")),
        ));
    }
    for t in missing {
        checks.push(Check::fail(
            format!("tool:{t}"),
            format!("`{t}` is missing on the target; a login needs it to unpack the environment"),
            format!("install `{t}` on the target (xxh needs sh, cat, mkdir, chmod, tar, gzip)"),
        ));
    }
    for (t, cost) in OPTIONAL_TOOLS {
        if !has_tool(p, t) {
            checks.push(Check::warn(
                format!("tool:{t}"),
                format!("`{t}` is missing on the target: {cost}"),
                format!("optional — install `{t}` on the target if that matters"),
            ));
        }
    }
    checks
}

/// Where a login would put the environment (C-P3).
pub fn root_check(p: &Probe) -> Check {
    match &p.root {
        Some((path, true)) => Check::pass("root", format!("environment directory {path} exists")),
        Some((path, false)) => {
            Check::pass("root", format!("environment will be created in {path}"))
        }
        None => Check::fail(
            "root",
            "no writable place for the environment ($HOME, $TMPDIR and /tmp all refuse)",
            "make $HOME or /tmp writable for this user, or log in as another user (-l)",
        ),
    }
}

/// Free space against what the login would unpack (research R3).
pub fn space_check(p: &Probe, need_kb: u64) -> Check {
    let Some((root, _)) = &p.root else {
        return Check::warn(
            "space",
            "free space not checked: no place for the environment",
            "fix the `root` check first",
        );
    };
    match p.free_kb {
        None => Check::warn(
            "space",
            format!("free space not checked; the login needs about {need_kb} KiB in {root}"),
            format!("make sure {need_kb} KiB are free in {root}"),
        ),
        Some(free) if free < need_kb => Check::fail(
            "space",
            format!("the login needs {need_kb} KiB in {root}, only {free} KiB are free"),
            format!("free at least {} KiB on the target", need_kb - free),
        ),
        Some(free) => Check::pass(
            "space",
            format!("{need_kb} KiB needed, {free} KiB free in {root}"),
        ),
    }
}

/// Whether the chosen shell will run on this platform (§FR-004, §FR-009).
pub fn shell_check(shell: &str, target: &str, lookup: &ShellLookup, host_has: bool) -> Check {
    let fetch = |available: &[String]| {
        let has = if available.is_empty() {
            "no builds yet".to_string()
        } else {
            format!("has {}", available.join(", "))
        };
        format!("the {shell} package has no build for {target} ({has})")
    };
    match (lookup, host_has) {
        (ShellLookup::Found(_), _) => Check::pass(
            "shell",
            format!("{shell}: packaged build for {target} will be delivered"),
        ),
        (ShellLookup::NoBuild { available }, true) => Check::warn(
            "shell",
            format!(
                "{}; the target's own {shell} will be used",
                fetch(available)
            ),
            format!("fetch the {target} build of the {shell} package (its fetch.sh)"),
        ),
        (ShellLookup::NoBuild { available }, false) => Check::fail(
            "shell",
            format!("{}, and the target has no {shell}", fetch(available)),
            format!(
                "fetch the {target} build of the {shell} package (its fetch.sh), \
                 or pick another shell with --shell"
            ),
        ),
        (ShellLookup::NotInstalled, true) => {
            Check::pass("shell", format!("the target's own {shell} will be used"))
        }
        (ShellLookup::NotInstalled, false) => Check::fail(
            "shell",
            format!("{shell} is neither packaged on this machine nor present on the target"),
            format!(
                "install a {shell} shell package into ~/.local/share/xxh/shells, \
                 or pick another shell with --shell"
            ),
        ),
    }
}

/// Plugins the login would skip on this platform (C-M5, §FR-004).
pub fn plugin_target_checks(plugins: &[SessionPlugin], platform: &Platform) -> Vec<Check> {
    // The full triple: a mask may exclude a target by its libc alone.
    let key = format!(
        "{}/{}/{}",
        platform.os_str(),
        platform.arch_str(),
        platform.libc_str()
    );
    let skipped: Vec<Check> = plugins
        .iter()
        .filter(|p| {
            !p.manifest
                .supports(platform.os_str(), platform.arch_str(), platform.libc_str())
        })
        .map(|p| {
            Check::warn(
                format!("plugin-target:{}", p.manifest.name),
                format!(
                    "plugin {} does not target {key}; the login will skip it",
                    p.manifest.name
                ),
                format!("disable it for this host, or install a build of it for {key}"),
            )
        })
        .collect();
    if skipped.is_empty() && !plugins.is_empty() {
        return vec![Check::pass(
            "plugins",
            format!("all {} enabled plugin(s) target {key}", plugins.len()),
        )];
    }
    skipped
}

/// KiB the login would unpack into `root`: components of `plan` it does not
/// hold yet (research R3).
pub fn needed_kb(plan: &[Component], present: &BTreeSet<String>) -> Result<u64, ShellError> {
    let mut bytes = 0u64;
    for c in plan.iter().filter(|c| !present.contains(&c.hash)) {
        bytes += c.size_hint()?;
    }
    Ok(bytes.div_ceil(1024))
}

/// Diagnose a connected target (§FR-002, §FR-004): platform, tools, the
/// environment directory, the shell, plugin compatibility and free space.
/// Nothing is written (C-D3). An unsupported platform stops at that check.
pub async fn diagnose_target<T: Transport + ?Sized>(
    transport: &mut T,
    label: &str,
    eff: &Effective,
    env: &[Component],
    plugins: &[SessionPlugin],
) -> Result<TargetReport, SessionError> {
    let mut report = TargetReport {
        label: label.to_string(),
        platform: None,
        checks: Vec::new(),
    };
    let platform = match detect_platform(transport).await {
        Ok(p) => p,
        Err(SessionError::Shell(e @ ShellError::Unsupported(_))) => {
            report.checks.push(Check::fail(
                "platform",
                e.to_string(),
                "xxh supports Linux (x86_64, aarch64, armv7; glibc or musl) targets",
            ));
            return Ok(report);
        }
        Err(e) => return Err(e),
    };
    let key = format!(
        "{}/{}/{}",
        platform.os_str(),
        platform.arch_str(),
        platform.libc_str()
    );
    report
        .checks
        .push(Check::pass("platform", format!("{key} is supported")));
    report.platform = Some(key);

    let p = probe(transport).await?;
    report.checks.extend(tool_checks(&p));
    report.checks.push(root_check(&p));

    let lookup = shellpkg::lookup(&eff.shell, &platform)?;
    let host_has = match lookup {
        ShellLookup::Found(_) => true,
        _ => {
            transport
                .exec(&format!("command -v {} >/dev/null 2>&1", eff.shell))
                .await?
                .exit_code
                == 0
        }
    };
    report.checks.push(shell_check(
        &eff.shell,
        &platform.target_key(),
        &lookup,
        host_has,
    ));
    report
        .checks
        .extend(plugin_target_checks(plugins, &platform));

    let plan = plan_components(&platform, eff, env, plugins, silent_progress())?;
    let present: BTreeSet<String> = match &p.root {
        Some((root, true)) => remote_env::inspect(transport)
            .await?
            .into_iter()
            .find(|e| &e.root == root)
            .map(|e| e.components.into_iter().map(|c| c.hash).collect())
            .unwrap_or_default(),
        _ => BTreeSet::new(),
    };
    report
        .checks
        .push(space_check(&p, needed_kb(&plan.components, &present)?));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = "tool\tsh\t/bin/sh\ntool\tcat\t/bin/cat\ntool\tmkdir\t/bin/mkdir\n\
        tool\tchmod\t/bin/chmod\ntool\ttar\t/bin/tar\ntool\tgzip\t/bin/gzip\n\
        tool\tzstd\t-\ntool\tdu\t/usr/bin/du\ntool\tdf\t/bin/df\n\
        root\tnew\t/home/u/.xxh\nfree\t5000\n";

    fn ids(checks: &[Check], status: CheckStatus) -> Vec<&str> {
        checks
            .iter()
            .filter(|c| c.status == status)
            .map(|c| c.id.as_str())
            .collect()
    }

    #[test]
    fn probe_reply_is_parsed() {
        let p = parse_probe(FULL).unwrap();
        assert_eq!(p.tools.len(), 9);
        assert_eq!(p.tools[6], ("zstd".to_string(), None));
        assert_eq!(p.root, Some(("/home/u/.xxh".into(), false)));
        assert_eq!(p.free_kb, Some(5000));

        let p = parse_probe("root\t-\nfree\t-\n").unwrap();
        assert_eq!((p.root, p.free_kb), (None, None));
        let p = parse_probe("root\texisting\t/tmp/my dir/.xxh\n").unwrap();
        assert_eq!(p.root, Some(("/tmp/my dir/.xxh".into(), true)));
        assert!(parse_probe("tool\tsh\n").is_err());
        assert!(parse_probe("root\tweird\t/x\n").is_err());
    }

    #[test]
    fn tools_split_into_required_and_optional() {
        let checks = tool_checks(&parse_probe(FULL).unwrap());
        assert_eq!(ids(&checks, CheckStatus::Pass), ["tools"]);
        assert_eq!(ids(&checks, CheckStatus::Warn), ["tool:zstd"]);
        assert!(ids(&checks, CheckStatus::Fail).is_empty());

        let no_gzip = FULL.replace("tool\tgzip\t/bin/gzip", "tool\tgzip\t-");
        let checks = tool_checks(&parse_probe(&no_gzip).unwrap());
        assert_eq!(ids(&checks, CheckStatus::Fail), ["tool:gzip"]);
        assert!(ids(&checks, CheckStatus::Pass).is_empty());
    }

    #[test]
    fn root_and_space_verdicts() {
        let p = parse_probe(FULL).unwrap();
        assert_eq!(root_check(&p).status, CheckStatus::Pass);
        assert_eq!(space_check(&p, 4000).status, CheckStatus::Pass);
        let tight = space_check(&p, 6000);
        assert_eq!(tight.status, CheckStatus::Fail);
        assert!(tight.message.contains("6000") && tight.message.contains("5000"));
        assert!(tight.action.unwrap().contains("1000"));

        let nowhere = parse_probe("root\t-\nfree\t-\n").unwrap();
        assert_eq!(root_check(&nowhere).status, CheckStatus::Fail);
        assert_eq!(space_check(&nowhere, 1).status, CheckStatus::Warn);
        let no_df = parse_probe("root\tnew\t/h/.xxh\nfree\t-\n").unwrap();
        assert_eq!(space_check(&no_df, 1).status, CheckStatus::Warn);
    }

    #[test]
    fn shell_verdicts_cover_package_build_and_host() {
        let none = ShellLookup::NotInstalled;
        let no_build = ShellLookup::NoBuild {
            available: vec!["linux-x86_64".into()],
        };
        let t = "linux-aarch64";
        assert_eq!(shell_check("zsh", t, &none, true).status, CheckStatus::Pass);
        assert_eq!(
            shell_check("zsh", t, &none, false).status,
            CheckStatus::Fail
        );
        let warn = shell_check("zsh", t, &no_build, true);
        assert_eq!(warn.status, CheckStatus::Warn);
        assert!(warn.message.contains("has linux-x86_64"), "{warn:?}");
        let fail = shell_check("zsh", t, &no_build, false);
        assert_eq!(fail.status, CheckStatus::Fail);
        assert!(fail.action.unwrap().contains("--shell"));
    }

    #[test]
    fn incompatible_plugins_are_named() {
        let plugin = |name: &str, targets: &str| SessionPlugin {
            manifest: xxh_plugin_api::Manifest::parse(&format!(
                "name = \"{name}\"\nversion = \"1.0.0\"\napi_version = \"1.0.0\"\n{targets}"
            ))
            .unwrap(),
            dir: std::path::PathBuf::from("/nonexistent"),
        };
        let x86 = Platform::parse_detect("Linux x86_64 | tar gzip").unwrap();
        let ok = plugin_target_checks(&[plugin("a", "")], &x86);
        assert_eq!(ids(&ok, CheckStatus::Pass), ["plugins"]);
        let arm = plugin("b", "targets = [\"linux/aarch64\"]\n");
        let skipped = plugin_target_checks(&[plugin("a", ""), arm], &x86);
        assert_eq!(ids(&skipped, CheckStatus::Warn), ["plugin-target:b"]);
        assert!(plugin_target_checks(&[], &x86).is_empty());
    }

    /// §FR-006: nothing doubtful without a way out.
    #[test]
    fn every_warning_and_failure_has_an_action() {
        let p = parse_probe("tool\tsh\t-\nroot\t-\nfree\t-\n").unwrap();
        let mut all = tool_checks(&p);
        all.push(root_check(&p));
        all.push(space_check(&p, 1));
        all.push(shell_check(
            "zsh",
            "linux-x86_64",
            &ShellLookup::NotInstalled,
            false,
        ));
        assert!(all.iter().any(|c| c.status == CheckStatus::Fail));
        for c in all.iter().filter(|c| c.status != CheckStatus::Pass) {
            assert!(c.action.as_deref().is_some_and(|a| !a.is_empty()), "{c:?}");
        }
    }
}
