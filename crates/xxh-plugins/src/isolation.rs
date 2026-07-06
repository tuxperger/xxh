//! Lifecycle-hook isolation (T037, §FR-019/020, C-M3/C-M4).
//!
//! Hooks run as **subprocesses** with a deliberately minimal environment:
//! only `XXH_*` variables supplied by the caller, a safe `PATH`, and `HOME` —
//! never the client's full environment (no SSH agent sockets, no tokens,
//! Принцип V). A hook that exits non-zero or overruns its timeout yields a
//! [`PluginError`]; the *caller* decides that the session continues.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use xxh_plugin_api::{HookSpec, PluginError};

/// Fallback PATH for hook subprocesses when the client has none.
const FALLBACK_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Run one hook of plugin `name` from its package directory.
///
/// `env` entries are exported to the hook **only** when their key starts with
/// `XXH_` (defence in depth: a caller cannot accidentally leak a secret under
/// an arbitrary name).
pub async fn run_hook(
    name: &str,
    package_dir: &Path,
    hook: &HookSpec,
    env: &BTreeMap<String, String>,
) -> Result<(), PluginError> {
    let program = package_dir.join(&hook.run);
    if !program.is_file() {
        return Err(PluginError::Other(format!(
            "plugin `{name}`: hook program `{}` not found in its package",
            hook.run
        )));
    }

    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg(&program)
        .current_dir(package_dir)
        .env_clear()
        // PATH is not a secret and hooks need it to find their interpreters;
        // everything else from the client environment stays out.
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| FALLBACK_PATH.into()),
        )
        .env("XXH_PLUGIN", name)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(home) = std::env::var_os("HOME") {
        cmd.env("HOME", home);
    }
    for (k, v) in env {
        if k.starts_with("XXH_") {
            cmd.env(k, v);
        }
    }

    let timeout = Duration::from_secs(u64::from(hook.timeout_s));
    let spawned = cmd
        .spawn()
        .map_err(|e| PluginError::Other(format!("plugin `{name}`: spawning hook: {e}")))?;
    let out = tokio::time::timeout(timeout, spawned.wait_with_output())
        .await
        .map_err(|_| {
            PluginError::Other(format!(
                "plugin `{name}`: hook `{}` timed out after {}s",
                hook.run, hook.timeout_s
            ))
        })?
        .map_err(|e| PluginError::Other(format!("plugin `{name}`: waiting for hook: {e}")))?;

    if out.status.success() {
        Ok(())
    } else {
        Err(PluginError::Other(format!(
            "plugin `{name}`: hook `{}` failed ({}): {}",
            hook.run,
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook_pkg(script: &str) -> std::path::PathBuf {
        // Unique per call: concurrent tests in one process can observe the same
        // SystemTime nanosecond value, which made two tests share (and clobber)
        // one package dir — a counter cannot collide.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("xxh-iso-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hook.sh"), script).unwrap();
        dir
    }

    fn spec(timeout_s: u32) -> HookSpec {
        HookSpec {
            run: "hook.sh".into(),
            timeout_s,
        }
    }

    #[tokio::test]
    async fn hook_env_is_restricted_to_xxh_vars() {
        // The hook fails if it can see a non-XXH variable, succeeds if XXH_OK is
        // set. `CARGO` is ambient in the test process (set by the test runner)
        // and must be stripped by env_clear; a non-XXH key in the `env` map must
        // be dropped by the filter. No `std::env::set_var` here: mutating the
        // process env while sibling tests fork hook subprocesses is UB and made
        // this suite flaky (child deadlock → hook timeout).
        let dir = hook_pkg(
            "[ -z \"${LEAKY_SECRET:-}\" ] && [ -z \"${CARGO:-}\" ] && [ \"$XXH_OK\" = 1 ]\n",
        );
        let mut env = BTreeMap::new();
        env.insert("XXH_OK".to_string(), "1".to_string());
        env.insert("LEAKY_SECRET".to_string(), "hunter2".to_string()); // must be dropped
        run_hook("t", &dir, &spec(10), &env).await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn failing_hook_is_a_plugin_error() {
        let dir = hook_pkg("echo boom >&2; exit 3\n");
        let err = run_hook("t", &dir, &spec(10), &BTreeMap::new())
            .await
            .expect_err("non-zero exit must surface");
        assert!(err.to_string().contains("boom"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn hook_timeout_is_enforced() {
        let dir = hook_pkg("sleep 30\n");
        let err = run_hook("t", &dir, &spec(1), &BTreeMap::new())
            .await
            .expect_err("timeout must surface");
        assert!(err.to_string().contains("timed out"), "got: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
