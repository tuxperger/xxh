//! Integrity of a kept environment (014; contracts/bootstrap-verify.md).
//!
//! The target describes each cached component — directories, executables,
//! anything that is neither a directory nor a regular file, and the SHA-256 of
//! every file — and the client compares that with what it expects from its own
//! copy ([`crate::deploy::Component::expected_listing`]). The comparison is by
//! sets, so `find`/`sort` differences between BusyBox and coreutils do not
//! matter (research R2).

use std::collections::{BTreeMap, BTreeSet};

use crate::ShellError;
use crate::session::SessionError;

/// What a component tree contains, paths as `./…` relative to its root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    pub dirs: BTreeSet<String>,
    pub execs: BTreeSet<String>,
    /// Symlinks, FIFOs, devices… — never part of a delivered component.
    pub others: BTreeSet<String>,
    /// File → SHA-256 (lowercase hex).
    pub files: BTreeMap<String, String>,
}

/// The target's answer to `verify` (C-V1..C-V3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerifyReply {
    pub root_own: bool,
    /// `ls -ld` permissions of the root, e.g. `drwx------`.
    pub root_perm: String,
    /// The tool the target lacks for a check, if any.
    pub unverifiable: Option<String>,
    /// Address → listing; `None` when the component is not on the target.
    pub components: BTreeMap<String, Option<Listing>>,
}

impl VerifyReply {
    /// The root is writable by group or others (C-V9).
    pub fn root_too_open(&self) -> bool {
        let b = self.root_perm.as_bytes();
        b.get(5) == Some(&b'w') || b.get(8) == Some(&b'w')
    }
}

fn protocol(line: &str) -> SessionError {
    ShellError::Other(format!("unexpected verify reply from the target: {line:?}")).into()
}

/// Parse the `verify` reply.
pub fn parse_verify(out: &str) -> Result<VerifyReply, SessionError> {
    let mut r = VerifyReply::default();
    let mut current: Option<(String, Listing)> = None;
    let flush = |cur: &mut Option<(String, Listing)>, r: &mut VerifyReply| {
        if let Some((h, l)) = cur.take() {
            r.components.insert(h, Some(l));
        }
    };
    for line in out.lines().filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.splitn(3, '\t').collect();
        match f.as_slice() {
            ["root", own, perm] => {
                r.root_own = *own == "own";
                r.root_perm = (*perm).to_string();
            }
            ["unverifiable", tool] => r.unverifiable = Some((*tool).to_string()),
            ["missing", h] => {
                flush(&mut current, &mut r);
                r.components.insert((*h).to_string(), None);
            }
            ["component", h] => {
                flush(&mut current, &mut r);
                current = Some(((*h).to_string(), Listing::default()));
            }
            [kind @ ("d" | "x" | "o"), path] => {
                let (_, l) = current.as_mut().ok_or_else(|| protocol(line))?;
                let set = match *kind {
                    "d" => &mut l.dirs,
                    "x" => &mut l.execs,
                    _ => &mut l.others,
                };
                set.insert((*path).to_string());
            }
            ["f", sha, path] => {
                let (_, l) = current.as_mut().ok_or_else(|| protocol(line))?;
                l.files.insert((*path).to_string(), (*sha).to_string());
            }
            _ => return Err(protocol(line)),
        }
    }
    flush(&mut current, &mut r);
    Ok(r)
}

/// The first difference between what the client expects and what the target
/// holds, worded for the user; `None` when they match (C-V6).
pub fn compare(expected: &Listing, actual: &Listing) -> Option<String> {
    if let Some(p) = actual.others.iter().next() {
        return Some(format!("{p} is not a regular file"));
    }
    for (path, sha) in &expected.files {
        match actual.files.get(path) {
            None => return Some(format!("{path} is missing")),
            Some(got) if got != sha => return Some(format!("{path} was changed")),
            Some(_) => {}
        }
    }
    if let Some(p) = actual
        .files
        .keys()
        .find(|p| !expected.files.contains_key(*p))
    {
        return Some(format!("{p} was added"));
    }
    if let Some(p) = actual.dirs.symmetric_difference(&expected.dirs).next() {
        return Some(if actual.dirs.contains(p) {
            format!("directory {p} was added")
        } else {
            format!("directory {p} is missing")
        });
    }
    if let Some(p) = actual.execs.symmetric_difference(&expected.execs).next() {
        return Some(format!("{p} changed its executable bit"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(c: char) -> String {
        std::iter::repeat_n(c, 64).collect()
    }

    fn sample() -> String {
        format!(
            "root\town\tdrwx------\ncomponent\t{a}\nd\t.\nd\t./bin\nx\t./bin/tool\n\
             f\t{s1}\t./bin/tool\nf\t{s2}\t./sp ace/f 1\nmissing\t{b}\n",
            a = addr('a'),
            b = addr('b'),
            s1 = "1".repeat(64),
            s2 = "2".repeat(64)
        )
    }

    #[test]
    fn verify_reply_is_parsed() {
        let r = parse_verify(&sample()).unwrap();
        assert!(r.root_own && !r.root_too_open());
        assert_eq!(r.unverifiable, None);
        let l = r.components[&addr('a')].as_ref().unwrap();
        assert_eq!(l.dirs.len(), 2);
        assert!(l.execs.contains("./bin/tool"));
        assert_eq!(l.files["./sp ace/f 1"], "2".repeat(64));
        assert_eq!(r.components[&addr('b')], None);

        let open = parse_verify("root\town\tdrwxrwxrwx\n").unwrap();
        assert!(open.root_too_open());
        let g = parse_verify("root\town\tdrwxrwx---\n").unwrap();
        assert!(g.root_too_open());
        let un = parse_verify("root\tforeign\tdrwx------\nunverifiable\tsha256sum\n").unwrap();
        assert!(!un.root_own);
        assert_eq!(un.unverifiable.as_deref(), Some("sha256sum"));
        assert!(
            parse_verify("f\tx\t./a\n").is_err(),
            "file before any component"
        );
    }

    #[test]
    fn every_kind_of_tampering_is_named() {
        let base = parse_verify(&sample()).unwrap().components[&addr('a')]
            .clone()
            .unwrap();
        assert_eq!(compare(&base, &base), None);

        let mut changed = base.clone();
        changed.files.insert("./bin/tool".into(), "9".repeat(64));
        assert_eq!(
            compare(&base, &changed).as_deref(),
            Some("./bin/tool was changed")
        );

        let mut added = base.clone();
        added.files.insert("./evil".into(), "3".repeat(64));
        assert_eq!(compare(&base, &added).as_deref(), Some("./evil was added"));

        let mut gone = base.clone();
        gone.files.remove("./sp ace/f 1");
        assert!(compare(&base, &gone).unwrap().contains("missing"));

        let mut link = base.clone();
        link.others.insert("./lnk".into());
        assert!(
            compare(&base, &link)
                .unwrap()
                .contains("not a regular file")
        );

        let mut exec = base.clone();
        exec.execs.clear();
        assert!(compare(&base, &exec).unwrap().contains("executable bit"));

        let mut dir = base.clone();
        dir.dirs.insert("./extra".into());
        assert!(compare(&base, &dir).unwrap().contains("directory ./extra"));
    }
}
