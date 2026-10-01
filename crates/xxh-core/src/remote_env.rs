//! What xxh left on a target, and removing it (005 T006; §FR-001..012).
//!
//! Talks to the target only through the bootstrap subcommands `status`, `clean`
//! and `prune`, always **streamed** (`sh -s -- …` with the script on stdin), so
//! inspecting writes nothing — not even the environment root (§FR-012). The script
//! decides which directories are ours and whether a session is alive; this module
//! parses its answers and compares them with the client's plan
//! (contracts/bootstrap-status-clean.md, data-model.md).

use std::collections::BTreeSet;

use serde::Serialize;
use xxh_transport::Transport;

use crate::ShellError;
use crate::deploy::Component;
use crate::session::{BOOTSTRAP_SH, SessionError};

/// A session marker on the target (data-model.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionMarker {
    pub id: String,
    pub pid: Option<u32>,
    /// The recorded process is alive; otherwise the marker is a crash leftover.
    pub active: bool,
}

/// Whether a stored component is part of what the client would deliver now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ComponentState {
    Current,
    Stale,
    /// The client's plan could not be built (C-S4).
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredComponent {
    pub hash: String,
    pub size_kb: Option<u64>,
    pub state: ComponentState,
    /// The plan's name for a current component (`shell zsh`, `plugin foo`).
    pub label: Option<String>,
}

/// One environment root found on the target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteEnv {
    pub root: String,
    /// Kept between sessions by `--keep`.
    pub kept: bool,
    /// Unix seconds of the last `--keep` login, by the client's clock (C-R9).
    pub last_used: Option<u64>,
    pub size_kb: Option<u64>,
    pub sessions: Vec<SessionMarker>,
    pub components: Vec<StoredComponent>,
    /// Anything else under the root or in its cache (temp files, `run/`).
    pub other: Vec<String>,
}

/// What the next login would reuse and deliver, by label.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PlanSummary {
    pub reuse: Vec<String>,
    pub deliver: Vec<String>,
}

/// A live session that stopped a removal (C-R4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockingSession {
    pub root: String,
    pub id: String,
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Removed {
    pub path: String,
    pub size_kb: Option<u64>,
}

/// Result of `clean`/`prune` (data-model.md). `refused` non-empty ⇒ nothing was
/// removed; `left` non-empty ⇒ the removal was incomplete.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CleanOutcome {
    pub refused: Vec<BlockingSession>,
    pub removed: Vec<Removed>,
    pub left: Vec<String>,
}

impl CleanOutcome {
    pub fn freed_kb(&self) -> Option<u64> {
        self.removed.iter().map(|r| r.size_kb).sum()
    }
}

/// Exit codes of the script's `clean`/`prune` (C-R4..C-R6).
const REFUSED: i32 = 3;
const INCOMPLETE: i32 = 4;

fn number<N: std::str::FromStr>(field: &str) -> Option<N> {
    if field == "-" {
        None
    } else {
        field.parse().ok()
    }
}

fn protocol_error(what: &str, line: &str) -> SessionError {
    ShellError::Other(format!("unexpected {what} reply from the target: {line:?}")).into()
}

/// Parse the `status` reply (C-R3 format). Unknown line kinds are skipped so an
/// older client can read a newer script.
pub fn parse_status(out: &str) -> Result<Vec<RemoteEnv>, SessionError> {
    let mut envs: Vec<RemoteEnv> = Vec::new();
    for line in out.lines().filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.splitn(4, '\t').collect();
        if f[0] == "root" {
            let root = f.get(1).ok_or_else(|| protocol_error("status", line))?;
            envs.push(RemoteEnv {
                root: (*root).to_string(),
                kept: false,
                last_used: None,
                size_kb: None,
                sessions: Vec::new(),
                components: Vec::new(),
                other: Vec::new(),
            });
            continue;
        }
        let env = envs
            .last_mut()
            .ok_or_else(|| protocol_error("status", line))?;
        match (f[0], f.len()) {
            ("keep", 2) => {
                env.kept = true;
                env.last_used = number(f[1]);
            }
            ("size", 2) => env.size_kb = number(f[1]),
            ("session", 4) => env.sessions.push(SessionMarker {
                id: f[1].to_string(),
                pid: number(f[2]),
                active: f[3] == "active",
            }),
            ("component", 3) => env.components.push(StoredComponent {
                hash: f[1].to_string(),
                size_kb: number(f[2]),
                state: ComponentState::Unknown,
                label: None,
            }),
            ("other", 2) => env.other.push(f[1].to_string()),
            ("keep" | "size" | "session" | "component" | "other", _) => {
                return Err(protocol_error("status", line));
            }
            _ => {}
        }
    }
    Ok(envs)
}

/// Parse a `clean`/`prune` reply by its exit code (C-R4, C-R5, C-R8).
pub fn parse_removal(code: i32, out: &str, err: &str) -> Result<CleanOutcome, SessionError> {
    if code != 0 && code != REFUSED && code != INCOMPLETE {
        return Err(ShellError::Other(format!(
            "cleanup on the target failed (exit {code}): {}",
            err.trim()
        ))
        .into());
    }
    let mut outcome = CleanOutcome::default();
    for line in out.lines().filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.splitn(4, '\t').collect();
        match (f[0], f.len()) {
            ("active", 4) => outcome.refused.push(BlockingSession {
                pid: number(f[1]),
                id: f[2].to_string(),
                root: f[3].to_string(),
            }),
            ("removed", 3) => outcome.removed.push(Removed {
                size_kb: number(f[1]),
                path: f[2].to_string(),
            }),
            ("left", 2) => outcome.left.push(f[1].to_string()),
            _ => return Err(protocol_error("cleanup", line)),
        }
    }
    if code == REFUSED && outcome.refused.is_empty() {
        return Err(protocol_error(
            "cleanup",
            "refused without naming a session",
        ));
    }
    Ok(outcome)
}

/// A component address as the host cache names it: 64 lowercase hex digits.
pub fn is_address(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

async fn streamed<T: Transport + ?Sized>(
    transport: &mut T,
    args: &str,
) -> Result<xxh_transport::ExecOutput, SessionError> {
    Ok(transport
        .upload_stream(
            &format!("sh -s -- {args}"),
            BOOTSTRAP_SH.as_bytes().to_vec(),
        )
        .await?)
}

/// Describe every environment root of the user on the target, changing nothing
/// (§FR-002, §FR-012). Component states are `Unknown` until [`classify`].
pub async fn inspect<T: Transport + ?Sized>(
    transport: &mut T,
) -> Result<Vec<RemoteEnv>, SessionError> {
    let out = streamed(transport, "status").await?;
    if out.exit_code != 0 {
        return Err(ShellError::Other(format!(
            "reading the environment on the target failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .into());
    }
    parse_status(&String::from_utf8_lossy(&out.stdout))
}

/// Remove every environment root of the user; refused while a session is alive
/// unless `force` (§FR-001, §FR-004, §FR-011).
pub async fn clean<T: Transport + ?Sized>(
    transport: &mut T,
    force: bool,
) -> Result<CleanOutcome, SessionError> {
    let out = streamed(transport, &format!("clean {}", u8::from(force))).await?;
    parse_removal(
        out.exit_code,
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    )
}

/// Remove every cached component whose address is not in `keep` (§FR-008), with
/// the same rule for live sessions as [`clean`].
pub async fn prune<T: Transport + ?Sized>(
    transport: &mut T,
    force: bool,
    keep: &[String],
) -> Result<CleanOutcome, SessionError> {
    // Checked before anything reaches the target: the list is spliced into the
    // command line, and the script would refuse it anyway (C-R6).
    if let Some(bad) = keep.iter().find(|h| !is_address(h)) {
        return Err(ShellError::Other(format!("not a component address: {bad:?}")).into());
    }
    let mut args = format!("prune {}", u8::from(force));
    for h in keep {
        args.push(' ');
        args.push_str(h);
    }
    let out = streamed(transport, &args).await?;
    parse_removal(
        out.exit_code,
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    )
}

/// Mark each stored component current or stale against the client's plan, and
/// say what the next login would reuse and deliver (§FR-007). The next login
/// uses the first root in resolution order, so that one decides reuse. Without a
/// plan every state stays `Unknown` and there is no summary (C-S4).
pub fn classify(envs: &mut [RemoteEnv], plan: Option<&[Component]>) -> Option<PlanSummary> {
    let plan = plan?;
    for env in envs.iter_mut() {
        for c in &mut env.components {
            match plan.iter().find(|p| p.hash == c.hash) {
                Some(p) => {
                    c.state = ComponentState::Current;
                    c.label = Some(p.label.clone());
                }
                None => c.state = ComponentState::Stale,
            }
        }
    }
    let present: BTreeSet<&str> = envs
        .first()
        .map(|e| e.components.iter().map(|c| c.hash.as_str()).collect())
        .unwrap_or_default();
    let mut summary = PlanSummary::default();
    for p in plan {
        if present.contains(p.hash.as_str()) {
            summary.reuse.push(p.label.clone());
        } else {
            summary.deliver.push(p.label.clone());
        }
    }
    Some(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use xxh_transport::{AuthPolicy, ExecOutput, PtySpec, ResolvedTarget, TransportError};

    fn addr(c: char) -> String {
        std::iter::repeat_n(c, 64).collect()
    }

    #[test]
    fn status_reply_is_parsed_per_root() {
        let out = format!(
            "root\t/home/u/.xxh\nkeep\t1700000000\nsize\t16\n\
             session\ts1\t42\tactive\nsession\ts2\t-\tdead\n\
             component\t{a}\t4\ncomponent\t{b}\t-\nother\tcache/.tmp.x\nother\trun\n\
             root\t/tmp/my dir/.xxh\nsize\t-\nfuture\tline\n",
            a = addr('a'),
            b = addr('b')
        );
        let envs = parse_status(&out).unwrap();
        assert_eq!(envs.len(), 2);
        let e = &envs[0];
        assert!(e.kept);
        assert_eq!(e.last_used, Some(1_700_000_000));
        assert_eq!(e.size_kb, Some(16));
        assert_eq!(
            e.sessions,
            vec![
                SessionMarker {
                    id: "s1".into(),
                    pid: Some(42),
                    active: true
                },
                SessionMarker {
                    id: "s2".into(),
                    pid: None,
                    active: false
                },
            ]
        );
        assert_eq!(e.components.len(), 2);
        assert_eq!(e.components[1].size_kb, None);
        assert_eq!(e.other, vec!["cache/.tmp.x", "run"]);
        // A path with spaces survives; unknown kinds are ignored.
        assert_eq!(envs[1].root, "/tmp/my dir/.xxh");
        assert!(!envs[1].kept);
        assert_eq!(envs[1].size_kb, None);
    }

    #[test]
    fn empty_status_means_nothing_left() {
        assert_eq!(parse_status("").unwrap(), vec![]);
        // A kept marker without a time is "unknown", not an error.
        let envs = parse_status("root\t/r\nkeep\t-\n").unwrap();
        assert!(envs[0].kept);
        assert_eq!(envs[0].last_used, None);
    }

    #[test]
    fn malformed_status_is_a_protocol_error() {
        assert!(parse_status("size\t1\n").is_err(), "line before any root");
        assert!(
            parse_status("root\t/r\nsession\tx\n").is_err(),
            "short line"
        );
    }

    #[test]
    fn removal_replies_map_to_outcomes() {
        let ok = parse_removal(0, "removed\t12\t/h/.xxh\nremoved\t-\t/tmp/.xxh\n", "").unwrap();
        assert_eq!(ok.removed.len(), 2);
        assert!(ok.refused.is_empty() && ok.left.is_empty());
        assert_eq!(ok.freed_kb(), None, "one size unknown ⇒ total unknown");
        assert_eq!(
            parse_removal(0, "removed\t12\t/a\nremoved\t30\t/b\n", "")
                .unwrap()
                .freed_kb(),
            Some(42)
        );

        let refused = parse_removal(3, "active\t42\ts1\t/h/.xxh\n", "").unwrap();
        assert_eq!(
            refused.refused,
            vec![BlockingSession {
                root: "/h/.xxh".into(),
                id: "s1".into(),
                pid: Some(42)
            }]
        );
        assert!(refused.removed.is_empty());

        let partial = parse_removal(4, "left\t/h/.xxh\n", "").unwrap();
        assert_eq!(partial.left, vec!["/h/.xxh"]);

        assert!(parse_removal(2, "", "bad address").is_err());
        assert!(
            parse_removal(3, "", "").is_err(),
            "refusal must name a session"
        );
        assert!(parse_removal(0, "deleted\t/x\n", "").is_err());
    }

    fn comp(label: &str, hash: &str) -> Component {
        let dir = std::env::temp_dir().join(format!("xxh-classify-{}-{label}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = Component::pack_dir(crate::deploy::ComponentKind::Config, &dir, "gz")
            .unwrap()
            .with_label(label);
        let _ = std::fs::remove_dir_all(&dir);
        c.hash = hash.to_string();
        c
    }

    #[test]
    fn components_are_classified_against_the_plan() {
        let out = format!(
            "root\t/h/.xxh\ncomponent\t{a}\t1\ncomponent\t{b}\t1\nroot\t/tmp/.xxh\ncomponent\t{c}\t1\n",
            a = addr('a'),
            b = addr('b'),
            c = addr('c')
        );
        let mut envs = parse_status(&out).unwrap();
        let plan = vec![comp("shell zsh", &addr('a')), comp("plugin p", &addr('c'))];

        let summary = classify(&mut envs, Some(&plan)).unwrap();
        let first = &envs[0].components;
        assert_eq!(first[0].state, ComponentState::Current);
        assert_eq!(first[0].label.as_deref(), Some("shell zsh"));
        assert_eq!(first[1].state, ComponentState::Stale);
        assert_eq!(first[1].label, None);
        assert_eq!(envs[1].components[0].state, ComponentState::Current);
        // Only the first root counts for the next login.
        assert_eq!(summary.reuse, vec!["shell zsh"]);
        assert_eq!(summary.deliver, vec!["plugin p"]);

        let mut fresh = parse_status(&out).unwrap();
        assert_eq!(classify(&mut fresh, None), None);
        assert!(
            fresh
                .iter()
                .flat_map(|e| &e.components)
                .all(|c| c.state == ComponentState::Unknown)
        );
    }

    /// Records every call; answers streamed bootstrap calls with `reply`.
    #[derive(Default)]
    struct Recorder {
        calls: Arc<Mutex<Vec<String>>>,
        reply: (i32, String),
    }

    #[async_trait::async_trait]
    impl Transport for Recorder {
        async fn connect(
            &mut self,
            _: &ResolvedTarget,
            _: &AuthPolicy,
        ) -> Result<(), TransportError> {
            Ok(())
        }
        async fn exec(&mut self, cmd: &str) -> Result<ExecOutput, TransportError> {
            self.calls.lock().unwrap().push(format!("exec {cmd}"));
            Ok(ExecOutput {
                exit_code: 0,
                stdout: vec![],
                stderr: vec![],
            })
        }
        async fn upload_stream(
            &mut self,
            cmd: &str,
            data: Vec<u8>,
        ) -> Result<ExecOutput, TransportError> {
            assert_eq!(data, BOOTSTRAP_SH.as_bytes(), "the script travels on stdin");
            self.calls.lock().unwrap().push(format!("stream {cmd}"));
            Ok(ExecOutput {
                exit_code: self.reply.0,
                stdout: self.reply.1.clone().into_bytes(),
                stderr: vec![],
            })
        }
        async fn exec_stream(&mut self, cmd: &str) -> Result<i32, TransportError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("exec_stream {cmd}"));
            Ok(0)
        }
        async fn open_pty(&mut self, _: &PtySpec) -> Result<i32, TransportError> {
            self.calls.lock().unwrap().push("pty".into());
            Ok(0)
        }
        async fn disconnect(&mut self) -> Result<(), TransportError> {
            Ok(())
        }
    }

    /// `inspect` is a single streamed `status` call — no exec that could write,
    /// no installed script (§FR-012).
    #[tokio::test]
    async fn inspect_only_streams_status() {
        let mut t = Recorder {
            reply: (0, "root\t/h/.xxh\n".into()),
            ..Default::default()
        };
        let envs = inspect(&mut t).await.unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(*t.calls.lock().unwrap(), vec!["stream sh -s -- status"]);
    }

    #[tokio::test]
    async fn clean_and_prune_pass_force_and_keep_list() {
        let mut t = Recorder {
            reply: (0, String::new()),
            ..Default::default()
        };
        clean(&mut t, true).await.unwrap();
        prune(&mut t, false, &[addr('a')]).await.unwrap();
        assert_eq!(
            *t.calls.lock().unwrap(),
            vec![
                "stream sh -s -- clean 1".to_string(),
                format!("stream sh -s -- prune 0 {}", addr('a')),
            ]
        );
    }

    /// Anything but an address is refused before reaching the target (C-R6).
    #[tokio::test]
    async fn prune_rejects_non_addresses_locally() {
        let mut t = Recorder::default();
        for bad in ["../x", "AAAA", "a; rm -rf /", &addr('g')] {
            assert!(
                prune(&mut t, true, &[bad.to_string()]).await.is_err(),
                "{bad}"
            );
        }
        assert!(t.calls.lock().unwrap().is_empty());
    }
}
