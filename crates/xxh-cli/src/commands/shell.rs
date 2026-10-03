//! `xxh shell …` — add/fetch/list/update/remove shell packages (008 T008/T010/T013;
//! contracts/cli-shell.md). The work is `xxh_core::shellmgr`; every failure is
//! shell-class (exit 20, §FR-010).

use std::fmt::Write as _;

use clap::Subcommand;
use clap_complete::engine::ArgValueCompleter;
use xxh_core::ShellError;

use crate::complete;
use xxh_core::shellmgr::{self, Builds, Fetched, Installed};
use xxh_plugins::source::SourceSpec;

#[derive(Subcommand)]
pub enum ShellAction {
    /// Install a shell package from a git URL or a local path, with its builds
    /// (by default every Linux build it declares).
    Add {
        source: String,
        /// Fetch only these platforms' builds (`os-arch`, repeatable).
        #[arg(long = "platform", value_name = "OS-ARCH")]
        platforms: Vec<String>,
        /// Install the package without any builds.
        #[arg(long, conflicts_with = "platforms")]
        no_builds: bool,
    },
    /// Download builds of an installed shell (by default every Linux build).
    Fetch {
        #[arg(add = ArgValueCompleter::new(complete::shells))]
        shell: String,
        /// Platforms to fetch (`os-arch`, repeatable).
        #[arg(long = "platform", value_name = "OS-ARCH")]
        platforms: Vec<String>,
        /// Every declared build, for any OS.
        #[arg(long, conflicts_with = "platforms")]
        all: bool,
    },
    /// List installed shells and the platforms they have builds for.
    List,
    /// Re-fetch shell packages from their sources (and changed builds).
    Update {
        #[arg(add = ArgValueCompleter::new(complete::shells))]
        shell: Option<String>,
    },
    /// Remove an installed shell package (a package linked in by hand loses only
    /// the link).
    Remove {
        #[arg(add = ArgValueCompleter::new(complete::shells))]
        shell: String,
    },
}

fn spec(source: &str) -> Result<SourceSpec, ShellError> {
    SourceSpec::parse(source).map_err(|e| ShellError::Package(e.to_string()))
}

fn builds(platforms: &[String], none: bool, all: bool) -> Builds {
    if none {
        Builds::Skip
    } else if all {
        Builds::All
    } else if platforms.is_empty() {
        Builds::Default
    } else {
        Builds::Only(platforms.to_vec())
    }
}

/// One `list` entry (C-SH3).
pub fn render_entry(i: &Installed) -> String {
    let origin = match (&i.source, i.linked) {
        (Some(s), _) => s.label(),
        (None, true) => "linked by hand".into(),
        (None, false) => "placed by hand".into(),
    };
    let mut out = format!("{} {} ({origin})\n", i.shell, i.manifest.version);
    let builds = if i.present.is_empty() {
        "none".to_string()
    } else {
        i.present.join(", ")
    };
    let _ = writeln!(out, "  builds: {builds}");
    if !i.missing.is_empty() {
        let _ = writeln!(out, "  not fetched: {}", i.missing.join(", "));
    }
    out
}

/// `default_shell` from the config, if it can be read — only for the removal
/// warning (C-SH5).
fn default_shell() -> Option<String> {
    let cfg = xxh_config::Config::load_default().ok()?;
    Some(cfg.resolve("", &xxh_config::CliOverrides::default()).shell)
}

pub async fn run(action: &ShellAction) -> Result<(), ShellError> {
    match action {
        ShellAction::Add {
            source,
            platforms,
            no_builds,
        } => {
            let i = shellmgr::add(&spec(source)?, &builds(platforms, *no_builds, false)).await?;
            print!("installed {}", render_entry(&i));
        }
        ShellAction::Fetch {
            shell,
            platforms,
            all,
        } => {
            for f in shellmgr::fetch(shell, &builds(platforms, false, *all)).await? {
                match f {
                    Fetched::Downloaded(p) => println!("{shell}: fetched {p}"),
                    Fetched::AlreadyPresent(p) => println!("{shell}: {p} already present"),
                }
            }
        }
        ShellAction::List => {
            let all = shellmgr::list()?;
            if all.is_empty() {
                println!("no shell packages installed");
            }
            for i in &all {
                print!("{}", render_entry(i));
            }
        }
        ShellAction::Update { shell } => {
            let (updated, skipped) = shellmgr::update(shell.as_deref()).await?;
            for i in &updated {
                print!("updated {}", render_entry(i));
            }
            for s in skipped {
                eprintln!("xxh: {s} was put into the shells directory by hand; not updated");
            }
        }
        ShellAction::Remove { shell } => {
            let linked = shellmgr::remove(shell)?;
            if linked {
                println!("removed the link to {shell} (its directory is untouched)");
            } else {
                println!("removed {shell}");
            }
            if default_shell().as_deref() == Some(shell.as_str()) {
                eprintln!(
                    "xxh: warning: {shell} is your default shell; logins now need it on the \
                     target, or set another one in the config"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use xxh_plugins::Manifest;

    fn installed(source: Option<SourceSpec>, linked: bool, missing: &[&str]) -> Installed {
        Installed {
            shell: "zsh".into(),
            dir: PathBuf::from("/x"),
            manifest: Manifest::parse(
                "name = \"zsh\"\nversion = \"5.8.0\"\napi_version = \"1.1.0\"\n",
            )
            .unwrap(),
            source,
            linked,
            present: vec!["linux-x86_64".into()],
            missing: missing.iter().map(|s| s.to_string()).collect(),
            hash: None,
            revision: None,
        }
    }

    #[test]
    fn list_entries_show_origin_and_builds() {
        let managed = installed(
            Some(SourceSpec::Local {
                path: PathBuf::from("/src/zsh"),
            }),
            false,
            &["darwin-aarch64"],
        );
        let text = render_entry(&managed);
        assert!(text.starts_with("zsh 5.8.0 ("), "{text}");
        assert!(text.contains("  builds: linux-x86_64\n"), "{text}");
        assert!(text.ends_with("  not fetched: darwin-aarch64\n"), "{text}");

        let linked = render_entry(&installed(None, true, &[]));
        assert!(
            linked.starts_with("zsh 5.8.0 (linked by hand)\n"),
            "{linked}"
        );
        assert!(!linked.contains("not fetched"));
    }

    #[test]
    fn build_selection_follows_flags() {
        assert_eq!(builds(&[], false, false), Builds::Default);
        assert_eq!(builds(&[], true, false), Builds::Skip);
        assert_eq!(builds(&[], false, true), Builds::All);
        assert_eq!(
            builds(&["linux-aarch64".into()], false, false),
            Builds::Only(vec!["linux-aarch64".into()])
        );
    }
}
