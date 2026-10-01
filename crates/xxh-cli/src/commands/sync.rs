//! `xxh sync` — install exactly the plugins and shells the config declares, at
//! the versions the lock file pins (013 T008; contracts/lock-and-sync.md C-L9).

use std::fmt::Write as _;
use std::path::Path;

use xxh_core::sync::{Outcome, Report};

/// The report for stdout (C-L9). Failures, a read-only lock and dropped
/// entries are the caller's to put on stderr / the exit code.
pub fn render(report: &Report, lock_path: &Path) -> String {
    let mut out = String::new();
    if report.lines.is_empty() {
        out.push_str("nothing declared: add [plugins.<name>] or [shells.<name>] with a source to the config\n");
    }
    for l in &report.lines {
        let what = match &l.outcome {
            Outcome::Unchanged => "unchanged".to_string(),
            Outcome::Installed(v) => format!("installed ({v})"),
            Outcome::Updated { from, to } => format!("updated {from} → {to}"),
            Outcome::Failed(why) => format!("failed: {why}"),
        };
        let _ = writeln!(out, "{} {}: {what}", l.kind, l.name);
    }
    for d in &report.dropped {
        let _ = writeln!(out, "lock: dropped {d} (no longer declared)");
    }
    if report.lock_written {
        let _ = writeln!(out, "lock: written {}", lock_path.display());
    }
    out
}

/// What to say when the lock could not be written (C-L8).
pub fn unwritable_notice(text: &str, lock_path: &Path) -> String {
    format!(
        "xxh: note: the lock file {} is read-only (managed elsewhere, e.g. by Nix); \
         put this into it to pin the current versions:\n{text}",
        lock_path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use xxh_core::sync::Line;

    #[test]
    fn report_lines() {
        let r = Report {
            lines: vec![
                Line {
                    kind: "plugin",
                    name: "a".into(),
                    outcome: Outcome::Unchanged,
                },
                Line {
                    kind: "plugin",
                    name: "b".into(),
                    outcome: Outcome::Updated {
                        from: "111".into(),
                        to: "222".into(),
                    },
                },
                Line {
                    kind: "shell",
                    name: "zsh".into(),
                    outcome: Outcome::Failed("boom".into()),
                },
            ],
            dropped: vec!["plugin old".into()],
            lock_written: true,
            lock_unwritable: None,
        };
        let text = render(&r, Path::new("/c/xxh.lock"));
        assert_eq!(
            text,
            "plugin a: unchanged\nplugin b: updated 111 → 222\nshell zsh: failed: boom\n\
             lock: dropped plugin old (no longer declared)\nlock: written /c/xxh.lock\n"
        );
        assert!(render(&Report::default(), Path::new("/x")).starts_with("nothing declared"));
    }
}
