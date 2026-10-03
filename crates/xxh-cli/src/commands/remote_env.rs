//! `xxh status <target>` and `xxh clean <target>` — see and remove what xxh left
//! on a target (005 T009/T013; contracts/cli-status-clean.md).
//!
//! Both reach the target exactly like a login (same factory, same auth) and talk
//! to it only through streamed bootstrap calls, so `status` writes nothing
//! (§FR-012). Rendering is kept pure for unit tests; the caller maps outcomes to
//! exit codes.

use std::fmt::Write as _;

use xxh_config::Effective;
use xxh_core::remote_env::{self, CleanOutcome, ComponentState, PlanSummary, RemoteEnv, classify};
use xxh_core::session::{Progress, SessionError, detect_platform, plan_components};
use xxh_transport::{AuthPolicy, ResolvedTarget, Transport};

use super::connect::env_components;
use super::target_io::open_transport;

/// What `status` found: the environments, and the plan summary when the client's
/// plan could be built (C-S4).
pub struct StatusReport {
    pub envs: Vec<RemoteEnv>,
    pub plan: Option<PlanSummary>,
}

async fn connected(
    target: ResolvedTarget,
    eff: &Effective,
    progress: Progress<'_>,
) -> Result<Box<dyn Transport>, SessionError> {
    let (mut transport, target) = open_transport(target, eff, progress).await?;
    progress(&format!("connect {}", target.label()));
    transport.connect(&target, &AuthPolicy::default()).await?;
    Ok(transport)
}

/// The addresses a login would deliver to this target now (005 research R5).
async fn plan_hashes(
    transport: &mut dyn Transport,
    eff: &Effective,
    progress: Progress<'_>,
) -> Result<Vec<xxh_core::deploy::Component>, SessionError> {
    let platform = detect_platform(transport).await?;
    let plugins = super::plugin::session_plugins(eff)?;
    let env = env_components(eff)?;
    Ok(plan_components(&platform, eff, &env.components, &plugins, progress)?.components)
}

/// `xxh status`: inspect, then classify against the client's plan. A plan that
/// cannot be built (a broken plugin, say) only costs the classification.
pub async fn status(
    target: ResolvedTarget,
    eff: &Effective,
    progress: Progress<'_>,
) -> Result<StatusReport, SessionError> {
    let mut transport = connected(target, eff, progress).await?;
    let mut envs = remote_env::inspect(&mut *transport).await?;
    let plan = match plan_hashes(&mut *transport, eff, progress).await {
        Ok(plan) => Some(plan),
        Err(e) => {
            eprintln!("xxh: warning: cannot tell current from stale components: {e}");
            None
        }
    };
    let _ = transport.disconnect().await;
    let summary = classify(&mut envs, plan.as_deref());
    Ok(StatusReport {
        envs,
        plan: summary,
    })
}

/// `xxh clean`: remove everything, or with `stale` only what the current plan
/// does not use (§FR-001, §FR-008). The plan must be known before pruning —
/// a guess could delete what the next login needs (C-C3).
pub async fn clean(
    target: ResolvedTarget,
    eff: &Effective,
    force: bool,
    stale: bool,
    progress: Progress<'_>,
) -> Result<CleanOutcome, SessionError> {
    let mut transport = connected(target, eff, progress).await?;
    let outcome = if stale {
        let keep: Vec<String> = plan_hashes(&mut *transport, eff, progress)
            .await?
            .into_iter()
            .map(|c| c.hash)
            .collect();
        remote_env::prune(&mut *transport, force, &keep).await?
    } else {
        remote_env::clean(&mut *transport, force).await?
    };
    let _ = transport.disconnect().await;
    Ok(outcome)
}

/// KiB as a short human size; `None` when the target has no `du`.
pub fn human_size(kb: Option<u64>) -> String {
    let Some(kb) = kb else {
        return "size unknown".into();
    };
    #[allow(clippy::cast_precision_loss)]
    let k = kb as f64;
    if kb < 1024 {
        format!("{kb} KiB")
    } else if kb < 1024 * 1024 {
        format!("{:.1} MiB", k / 1024.0)
    } else {
        format!("{:.1} GiB", k / (1024.0 * 1024.0))
    }
}

/// How long ago `then` was, relative to `now` (both Unix seconds).
pub fn ago(then: Option<u64>, now: u64) -> String {
    let Some(then) = then else {
        return "unknown".into();
    };
    let d = now.saturating_sub(then);
    match d {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", d / 60),
        3600..86_400 => format!("{} h ago", d / 3600),
        _ => format!("{} days ago", d / 86_400),
    }
}

fn short(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

/// The human-readable `status` (C-S2).
pub fn render_status(label: &str, report: &StatusReport, now: u64) -> String {
    let mut out = String::new();
    if report.envs.is_empty() {
        let _ = writeln!(out, "nothing from xxh on {label}");
    }
    for env in &report.envs {
        let kind = if env.kept {
            format!("kept, last used {}", ago(env.last_used, now))
        } else {
            "not kept (left by an interrupted session)".into()
        };
        let _ = writeln!(out, "{} — {kind}, {}", env.root, human_size(env.size_kb));
        let active: Vec<_> = env.sessions.iter().filter(|s| s.active).collect();
        let dead = env.sessions.len() - active.len();
        if !env.sessions.is_empty() {
            let mut line = format!("  sessions: {} active", active.len());
            for s in &active {
                let pid = s.pid.map_or("-".into(), |p| p.to_string());
                let _ = write!(line, " ({} pid {pid})", s.id);
            }
            if dead > 0 {
                let _ = write!(line, ", {dead} left by a crash");
            }
            let _ = writeln!(out, "{line}");
        }
        if !env.components.is_empty() {
            let _ = writeln!(out, "  components:");
        }
        for c in &env.components {
            let state = match c.state {
                ComponentState::Current => "current",
                ComponentState::Stale => "stale",
                ComponentState::Unknown => "unknown",
            };
            let _ = writeln!(
                out,
                "    {state:<8} {}  {:<20} {}",
                short(&c.hash),
                c.label.as_deref().unwrap_or("-"),
                human_size(c.size_kb)
            );
        }
        if !env.other.is_empty() {
            let _ = writeln!(out, "  other: {}", env.other.join(", "));
        }
    }
    if let Some(plan) = &report.plan {
        let _ = write!(
            out,
            "next login: reuses {}, delivers {}",
            plan.reuse.len(),
            plan.deliver.len()
        );
        if !plan.deliver.is_empty() {
            let _ = write!(out, " ({})", plan.deliver.join(", "));
        }
        out.push('\n');
    }
    out
}

/// The machine-readable `status` (C-S3).
pub fn status_json(label: &str, report: &StatusReport) -> serde_json::Value {
    serde_json::json!({
        "target": label,
        "envs": report.envs,
        "plan": report.plan,
    })
}

/// The human-readable `clean` result for stdout (C-C1). Refusals and leftovers
/// are reported separately by the caller, on stderr, with exit code 50.
pub fn render_clean(label: &str, outcome: &CleanOutcome, stale: bool) -> String {
    let mut out = String::new();
    // A refusal removed nothing on purpose; it is explained on stderr only.
    if !outcome.refused.is_empty() {
        return out;
    }
    if outcome.removed.is_empty() && outcome.left.is_empty() {
        let what = if stale {
            "no stale components"
        } else {
            "nothing to clean"
        };
        let _ = writeln!(out, "{what} on {label}");
        return out;
    }
    for r in &outcome.removed {
        let _ = writeln!(out, "removed {} ({})", r.path, human_size(r.size_kb));
    }
    if !outcome.removed.is_empty() {
        let _ = writeln!(out, "freed {}", human_size(outcome.freed_kb()));
    }
    out
}

/// Why `clean` ends with code 50, for stderr (C-C2, C-C4); `None` on success.
pub fn clean_problem(label: &str, outcome: &CleanOutcome) -> Option<String> {
    if !outcome.refused.is_empty() {
        let mut msg = format!(
            "{} active session(s) on {label}; nothing was removed:",
            outcome.refused.len()
        );
        for s in &outcome.refused {
            let pid = s.pid.map_or("-".into(), |p| p.to_string());
            let _ = write!(msg, "\n  {} (pid {pid}) in {}", s.id, s.root);
        }
        msg.push_str("\nrerun with --force to remove it anyway");
        return Some(msg);
    }
    if !outcome.left.is_empty() {
        let mut msg = "could not remove everything; still on the target:".to_string();
        for p in &outcome.left {
            let _ = write!(msg, "\n  {p}");
        }
        msg.push_str("\ncheck the permissions there and rerun `xxh clean`");
        return Some(msg);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use xxh_core::remote_env::{
        BlockingSession, Removed, SessionMarker, StoredComponent, parse_status,
    };

    fn addr(c: char) -> String {
        std::iter::repeat_n(c, 64).collect()
    }

    fn kept_env() -> RemoteEnv {
        RemoteEnv {
            root: "/home/u/.xxh".into(),
            kept: true,
            last_used: Some(1_000),
            size_kb: Some(2048),
            sessions: vec![
                SessionMarker {
                    id: "s1".into(),
                    pid: Some(42),
                    active: true,
                },
                SessionMarker {
                    id: "s0".into(),
                    pid: None,
                    active: false,
                },
            ],
            components: vec![
                StoredComponent {
                    hash: addr('a'),
                    size_kb: Some(2000),
                    state: ComponentState::Current,
                    label: Some("shell zsh".into()),
                },
                StoredComponent {
                    hash: addr('b'),
                    size_kb: None,
                    state: ComponentState::Stale,
                    label: None,
                },
            ],
            other: vec!["run".into()],
        }
    }

    #[test]
    fn sizes_and_ages_read_naturally() {
        assert_eq!(human_size(None), "size unknown");
        assert_eq!(human_size(Some(12)), "12 KiB");
        assert_eq!(human_size(Some(2048)), "2.0 MiB");
        assert_eq!(human_size(Some(3 * 1024 * 1024)), "3.0 GiB");
        assert_eq!(ago(None, 100), "unknown");
        assert_eq!(ago(Some(100), 130), "just now");
        assert_eq!(ago(Some(100), 50), "just now", "client clock behind");
        assert_eq!(ago(Some(0), 600), "10 min ago");
        assert_eq!(ago(Some(0), 7200), "2 h ago");
        assert_eq!(ago(Some(0), 3 * 86_400), "3 days ago");
    }

    #[test]
    fn empty_target_says_so() {
        let r = StatusReport {
            envs: vec![],
            plan: None,
        };
        assert_eq!(render_status("web", &r, 0), "nothing from xxh on web\n");
    }

    #[test]
    fn kept_environment_is_described() {
        let r = StatusReport {
            envs: vec![kept_env()],
            plan: Some(PlanSummary {
                reuse: vec!["shell zsh".into()],
                deliver: vec!["plugin p".into()],
            }),
        };
        let text = render_status("web", &r, 1_000 + 300);
        assert!(
            text.starts_with("/home/u/.xxh — kept, last used 5 min ago, 2.0 MiB\n"),
            "{text}"
        );
        assert!(
            text.contains("sessions: 1 active (s1 pid 42), 1 left by a crash"),
            "{text}"
        );
        assert!(text.contains("current  aaaaaaaaaaaa  shell zsh"), "{text}");
        assert!(text.contains("stale    bbbbbbbbbbbb  -"), "{text}");
        assert!(text.contains("size unknown"), "{text}");
        assert!(text.contains("other: run"), "{text}");
        assert!(
            text.ends_with("next login: reuses 1, delivers 1 (plugin p)\n"),
            "{text}"
        );
    }

    #[test]
    fn unknown_time_and_unkept_root_are_explicit() {
        let mut envs = parse_status("root\t/tmp/.xxh\nkeep\t-\nsize\t-\n").unwrap();
        let text = render_status(
            "c",
            &StatusReport {
                envs: envs.clone(),
                plan: None,
            },
            0,
        );
        assert!(
            text.contains("kept, last used unknown, size unknown"),
            "{text}"
        );
        assert!(!text.contains("next login"), "no plan, no summary");
        envs[0].kept = false;
        let text = render_status("c", &StatusReport { envs, plan: None }, 0);
        assert!(text.contains("not kept"), "{text}");
    }

    /// The JSON keeps the C-S3 shape: names, nulls and lowercase states.
    #[test]
    fn json_follows_the_contract() {
        let r = StatusReport {
            envs: vec![kept_env()],
            plan: None,
        };
        let v = status_json("web", &r);
        assert_eq!(v["target"], "web");
        assert!(v["plan"].is_null());
        let env = &v["envs"][0];
        assert_eq!(env["root"], "/home/u/.xxh");
        assert_eq!(env["kept"], true);
        assert_eq!(env["last_used"], 1_000);
        assert_eq!(env["size_kb"], 2048);
        assert_eq!(env["sessions"][0]["pid"], 42);
        assert!(env["sessions"][1]["pid"].is_null());
        assert_eq!(env["sessions"][0]["active"], true);
        assert_eq!(env["components"][0]["state"], "current");
        assert_eq!(env["components"][0]["label"], "shell zsh");
        assert_eq!(env["components"][1]["state"], "stale");
        assert!(env["components"][1]["size_kb"].is_null());
        assert_eq!(env["other"][0], "run");

        let with_plan = StatusReport {
            envs: vec![],
            plan: Some(PlanSummary {
                reuse: vec![],
                deliver: vec!["env".into()],
            }),
        };
        assert_eq!(status_json("web", &with_plan)["plan"]["deliver"][0], "env");
    }

    #[test]
    fn clean_results_render() {
        let none = CleanOutcome::default();
        assert_eq!(
            render_clean("web", &none, false),
            "nothing to clean on web\n"
        );
        assert_eq!(
            render_clean("web", &none, true),
            "no stale components on web\n"
        );
        assert_eq!(clean_problem("web", &none), None);

        let done = CleanOutcome {
            removed: vec![Removed {
                path: "/home/u/.xxh".into(),
                size_kb: Some(10),
            }],
            ..Default::default()
        };
        assert_eq!(
            render_clean("web", &done, false),
            "removed /home/u/.xxh (10 KiB)\nfreed 10 KiB\n"
        );

        let refused = CleanOutcome {
            refused: vec![BlockingSession {
                root: "/home/u/.xxh".into(),
                id: "s1".into(),
                pid: Some(42),
            }],
            ..Default::default()
        };
        assert_eq!(
            render_clean("web", &refused, false),
            "",
            "not \"nothing to clean\""
        );
        let msg = clean_problem("web", &refused).unwrap();
        assert!(msg.contains("1 active session(s) on web; nothing was removed"));
        assert!(msg.contains("s1 (pid 42) in /home/u/.xxh"));
        assert!(msg.contains("--force"));

        let partial = CleanOutcome {
            left: vec!["/home/u/.xxh".into()],
            ..Default::default()
        };
        assert!(
            clean_problem("web", &partial)
                .unwrap()
                .contains("still on the target:\n  /home/u/.xxh")
        );
    }
}
