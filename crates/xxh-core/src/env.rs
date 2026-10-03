//! Session environment variables (011 T001, contracts/env.md C-E2, research R1/R5).
//!
//! The user's variables reach the target as shell text: one assignment per
//! variable, read by the session prelude. Names are checked rather than escaped,
//! values only ever appear inside single quotes, where `sh` interprets nothing.
//! No message here carries a value (§FR-008): a value may be a secret.

use std::collections::BTreeMap;

/// Variables of xxh itself: the bootstrap and the components find their paths
/// through them (`XXH_ROOT`, `XXH_COMPONENT_DIR`, `XXH_SESSION`; §FR-007).
const RESERVED_PREFIX: &str = "XXH_";

/// Whether `name` may be set in the session (C-E2).
pub fn check_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let head = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if !head || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!(
            "env: `{name}` is not a variable name (letters, digits and `_`, not starting \
             with a digit)"
        ));
    }
    if name.starts_with(RESERVED_PREFIX) {
        return Err(format!(
            "env: `{name}` belongs to xxh itself — variables starting with `{RESERVED_PREFIX}` \
             cannot be set"
        ));
    }
    Ok(())
}

/// Whether a value can be carried at all: neither an environment variable nor a
/// `sh` string can hold a NUL byte. The value itself is never quoted back.
pub fn check_value(name: &str, value: &str) -> Result<(), String> {
    if value.contains('\0') {
        return Err(format!("env: the value of `{name}` contains a NUL byte"));
    }
    Ok(())
}

/// [`check_name`] and [`check_value`] for a whole set; the first problem is reported.
pub fn check(vars: &BTreeMap<String, String>) -> Result<(), String> {
    vars.iter()
        .try_for_each(|(name, value)| check_name(name).and_then(|()| check_value(name, value)))
}

/// The set as `sh` text: `NAME='value'; export NAME`, one per line. Inside single
/// quotes only `'` itself needs care; it becomes `'\''`. Call [`check`] first.
pub fn render(vars: &BTreeMap<String, String>) -> String {
    vars.iter()
        .map(|(name, value)| format!("{name}='{}'; export {name}\n", value.replace('\'', "'\\''")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn names_follow_the_shell_rules() {
        for good in ["A", "_x", "EDITOR", "aws_profile2", "_"] {
            assert!(check_name(good).is_ok(), "{good}");
        }
        for bad in ["", "1X", "A-B", "A B", "A=B", "Ä", "$X", "a.b"] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn xxh_variables_are_reserved() {
        for name in ["XXH_ROOT", "XXH_COMPONENT_DIR", "XXH_SESSION", "XXH_"] {
            let e = check_name(name).unwrap_err();
            assert!(e.contains("belongs to xxh"), "{e}");
        }
        // Only the exact prefix: similar names are the user's.
        assert!(check_name("XXHX").is_ok());
        assert!(check_name("xxh_root").is_ok());
    }

    #[test]
    fn a_nul_is_refused_without_showing_the_value() {
        let e = check(&set(&[("TOKEN", "s3cr3t\0tail")])).unwrap_err();
        assert!(e.contains("TOKEN") && e.contains("NUL"), "{e}");
        assert!(!e.contains("s3cr3t"), "the value leaked: {e}");
        let e = check(&set(&[("1BAD", "s3cr3t")])).unwrap_err();
        assert!(!e.contains("s3cr3t"), "the value leaked: {e}");
    }

    #[test]
    fn empty_set_renders_nothing() {
        assert_eq!(render(&BTreeMap::new()), "");
    }

    /// What `sh` makes of [`render`]: every value comes back byte for byte.
    #[test]
    fn values_roundtrip_through_a_real_shell() {
        let vars = set(&[
            ("SPACES", "a b  c"),
            ("SINGLE", "it's 'quoted'"),
            ("DOUBLE", "say \"hi\""),
            ("DOLLAR", "$HOME ${PATH} $(id)"),
            ("BACKTICK", "`id`"),
            ("BACKSLASH", "a\\nb\\"),
            ("NEWLINES", "line1\nline2\n\n"),
            ("EMPTY", ""),
            ("UNICODE", "привет ✓"),
            ("GLOB", "* ? [a]"),
        ]);
        check(&vars).unwrap();
        let mut script = render(&vars);
        // Print each one NUL-terminated from a child: exported, not just set.
        for name in vars.keys() {
            script.push_str(&format!("sh -c 'printf \"%s\\0\" \"${name}\"'\n"));
        }
        let out = std::process::Command::new("sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let got: Vec<&[u8]> = out.stdout.split(|b| *b == 0).collect();
        for (i, value) in vars.values().enumerate() {
            assert_eq!(got[i], value.as_bytes(), "value #{i}");
        }
    }
}
