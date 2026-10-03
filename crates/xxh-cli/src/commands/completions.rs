//! `xxh completions <shell>` — print the completion script for a shell (007 T004,
//! contracts/completions-and-man.md C-K1..C-K3).
//!
//! The scripts are thin stubs that ask `xxh __complete` (see `crate::complete`)
//! for candidates: nothing in them depends on the xxh version, so an old script
//! keeps working with a newer binary.

use clap::ValueEnum;

/// The shells a completion script exists for (§FR-001).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

/// The script to load into `shell`.
pub fn script(shell: CompletionShell) -> &'static str {
    match shell {
        CompletionShell::Bash => include_str!("../../completions/xxh.bash"),
        CompletionShell::Zsh => include_str!("../../completions/xxh.zsh"),
        CompletionShell::Fish => include_str!("../../completions/xxh.fish"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every script calls back into the binary named on the command line and
    /// hides its stderr (C-K1, §FR-006).
    #[test]
    fn scripts_are_stubs() {
        for shell in CompletionShell::value_variants() {
            let text = script(*shell);
            assert!(text.contains("__complete"), "{shell:?}");
            assert!(text.contains("2>/dev/null"), "{shell:?}");
            assert!(!text.contains("/nix/store"), "{shell:?}");
        }
        assert!(script(CompletionShell::Zsh).starts_with("#compdef xxh\n"));
    }
}
