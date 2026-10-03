//! `xxh man [--dir <DIR>]` — manual pages for the command and every subcommand
//! (007 T012, contracts/completions-and-man.md C-K13..C-K16).
//!
//! The pages are rendered from the same clap tree as `--help`, so the two cannot
//! disagree on commands and flags (SC-004). `xxh(1)` also documents what the help
//! text has no place for: exit codes and file locations.

use std::path::Path;

use clap_mangen::Man;
use clap_mangen::roff::{Roff, bold, roman};

/// Exit codes by error class (the taxonomy of `main.rs`, 001 §FR-026).
const EXIT_STATUS: [(&str, &str); 8] = [
    (
        "0",
        "Success. For a one-command run (-c or --), the exit status of that command.",
    ),
    ("1", "xxh doctor found at least one failed check."),
    ("2", "Usage error: bad arguments, nothing was attempted."),
    (
        "10",
        "Transport error: the target could not be reached or authenticated.",
    ),
    (
        "20",
        "Shell error: the shell could not be built, delivered or started.",
    ),
    (
        "30",
        "Plugin error: a plugin or its source could not be installed or loaded.",
    ),
    (
        "40",
        "Configuration error: invalid config or target address.",
    ),
    (
        "50",
        "Target error: xxh clean refused or left something behind.",
    ),
];

const FILES: [(&str, &str); 6] = [
    (
        "~/.config/xxh/config.toml",
        "The configuration file; the only source of settings besides command-line flags.",
    ),
    (
        "/etc/xxh/config.toml",
        "System-wide configuration, read when the per-user file is absent.",
    ),
    (
        "~/.config/xxh/xxh.lock",
        "Pinned versions of the declared plugins and shells, written by xxh sync.",
    ),
    (
        "~/.local/share/xxh/plugins",
        "Installed plugins (XXH_PLUGINS_DIR overrides).",
    ),
    (
        "~/.local/share/xxh/shells",
        "Installed shell packages (XXH_SHELLS_DIR overrides).",
    ),
    (
        "~/.ssh/config",
        "Honoured for SSH targets: host aliases, users, keys, ProxyJump.",
    ),
];

fn definitions(roff: &mut Roff, title: &str, items: &[(&str, &str)]) {
    roff.control("SH", [title]);
    for (term, text) in items {
        roff.control("TP", []);
        roff.text([bold(*term)]);
        roff.text([roman(*text)]);
    }
}

fn io<T>(r: std::io::Result<T>) -> T {
    // Rendering writes into memory; it cannot fail.
    r.expect("in-memory write")
}

/// The page of the top-level command: the generated sections plus EXIT STATUS
/// and FILES (C-K15).
fn main_page(cmd: &clap::Command) -> Vec<u8> {
    let man = Man::new(cmd.clone());
    let mut out = Vec::new();
    io(man.render_title(&mut out));
    io(man.render_name_section(&mut out));
    io(man.render_synopsis_section(&mut out));
    io(man.render_description_section(&mut out));
    io(man.render_options_section(&mut out));
    io(man.render_subcommands_section(&mut out));
    let mut extra = Roff::default();
    definitions(&mut extra, "EXIT STATUS", &EXIT_STATUS);
    definitions(&mut extra, "FILES", &FILES);
    io(extra.to_writer(&mut out));
    io(man.render_version_section(&mut out));
    out
}

fn subcommand_pages(cmd: &clap::Command, out: &mut Vec<(String, Vec<u8>)>) {
    for sub in cmd.get_subcommands().filter(|s| !s.is_hide_set()) {
        let man = Man::new(sub.clone());
        let mut page = Vec::new();
        io(man.render(&mut page));
        out.push((man.get_filename(), page));
        subcommand_pages(sub, out);
    }
}

/// Every page as `(file name, roff)`: `xxh.1` first, then one per visible
/// subcommand at any depth, named by the command path (`xxh-plugin-add.1`, C-K14).
pub fn pages(cmd: clap::Command) -> Vec<(String, Vec<u8>)> {
    let mut cmd = cmd.disable_help_subcommand(true);
    cmd.build();
    let mut out = vec![(format!("{}.1", cmd.get_name()), main_page(&cmd))];
    subcommand_pages(&cmd, &mut out);
    out
}

/// Write every page into `dir`, creating it if needed (C-K14).
pub fn write_to(cmd: clap::Command, dir: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("cannot create the directory {}: {e}", dir.display()))?;
    let pages = pages(cmd);
    for (name, page) in &pages {
        let path = dir.join(name);
        std::fs::write(&path, page).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    Ok(pages.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    fn text(page: &[u8]) -> String {
        // roff escapes dashes; undo it to compare with flag names.
        String::from_utf8_lossy(page).replace("\\-", "-")
    }

    /// SC-004: one page per visible command, each naming all of its flags.
    #[test]
    fn pages_follow_the_command_tree() {
        fn walk(cmd: &clap::Command, name: String, pages: &[(String, Vec<u8>)], seen: &mut usize) {
            let file = format!("{name}.1");
            let page = pages
                .iter()
                .find(|(n, _)| *n == file)
                .map(|(_, p)| text(p))
                .unwrap_or_else(|| panic!("no page {file}"));
            *seen += 1;
            for arg in cmd.get_arguments().filter(|a| !a.is_hide_set()) {
                if let Some(long) = arg.get_long() {
                    assert!(page.contains(&format!("--{long}")), "--{long} in {file}");
                }
            }
            for sub in cmd.get_subcommands().filter(|s| !s.is_hide_set()) {
                assert!(
                    page.contains(sub.get_name()),
                    "{} in {file}",
                    sub.get_name()
                );
                walk(sub, format!("{name}-{}", sub.get_name()), pages, seen);
            }
        }
        let pages = pages(crate::Cli::command());
        let mut cmd = crate::Cli::command().disable_help_subcommand(true);
        cmd.build();
        let mut seen = 0;
        walk(&cmd, "xxh".into(), &pages, &mut seen);
        assert_eq!(seen, pages.len(), "no page without a command");
        assert!(pages.iter().any(|(n, _)| n == "xxh-plugin-add.1"));
        assert!(pages.iter().all(|(n, _)| !n.contains("__complete")));
    }

    #[test]
    fn main_page_documents_exit_codes_and_files() {
        let pages = pages(crate::Cli::command());
        let (name, page) = &pages[0];
        assert_eq!(name, "xxh.1");
        let page = text(page);
        assert!(page.starts_with(".ie"), "{}", &page[..40]);
        assert!(page.contains(".SH \"EXIT STATUS\""));
        for (code, _) in EXIT_STATUS {
            assert!(page.contains(&format!("\\fB{code}\\fR")), "code {code}");
        }
        assert!(page.contains("~/.config/xxh/config.toml"));
    }
}
