//! xxh-core — session orchestration: connect → detect → deploy → PTY → cleanup.
//!
//! Foundational layer. `platform` (T012) and observability (T006) land here;
//! deploy/bootstrap/cleanup/session modules follow in US1 (T015–T018).

pub mod deploy;
pub mod platform;
pub mod session;

use std::sync::atomic::{AtomicBool, Ordering};

/// Error class for shell delivery/build problems. Maps to CLI exit code 20 (§FR-026).
#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    /// Host platform is outside the supported matrix (§FR-007).
    #[error("unsupported host platform: {0}")]
    Unsupported(String),
    /// Requested shell is not available in the environment/cache (§FR-011).
    #[error("shell `{0}` is not available; add it as a plugin first")]
    NotAvailable(String),
    /// Generic delivery/assembly failure.
    #[error("shell error: {0}")]
    Other(String),
}

/// Verbosity level selected on the CLI (`-v`, `-vv`, `--debug`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Verbosity {
    #[default]
    Normal,
    Verbose,
    VeryVerbose,
    Debug,
}

impl Verbosity {
    fn env_filter(self) -> &'static str {
        match self {
            Verbosity::Normal => "warn",
            Verbosity::Verbose => "info",
            Verbosity::VeryVerbose => "debug",
            Verbosity::Debug => "trace",
        }
    }
}

static TRACING_INIT: AtomicBool = AtomicBool::new(false);

/// Initialise observability (T006). Secrets and private keys are NEVER logged, even
/// at debug/trace (§FR-028, Принцип V): the codebase relies on structured events
/// that carry no secret fields, and this helper deliberately exposes no way to log
/// raw key material. Use [`redact`] when a value might contain a secret.
pub fn init_observability(verbosity: Verbosity) {
    if TRACING_INIT.swap(true, Ordering::SeqCst) {
        return;
    }
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(verbosity.env_filter()));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Replace a possibly-secret value with a fixed marker so it can never reach logs
/// or user-facing output (§FR-028). Length and content are not revealed.
pub fn redact(_secret: &str) -> &'static str {
    "<redacted>"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_never_reveals_the_secret() {
        let secret = "hunter2-super-secret-key";
        let shown = redact(secret);
        assert!(!shown.contains("hunter2"));
        assert_eq!(shown, "<redacted>");
    }

    #[test]
    fn verbosity_maps_to_filters() {
        assert_eq!(Verbosity::Normal.env_filter(), "warn");
        assert_eq!(Verbosity::Debug.env_filter(), "trace");
    }
}
