//! The config's keys, as the types define them (009 T002, research R1,
//! contracts/config-commands.md C-G1/C-G2).
//!
//! `xxh config validate|get|set|unset` and completion need to know which keys
//! exist, what kind of value each takes and which values are allowed. That is
//! read from the JSON Schema generated from [`crate::Config`] — the same single
//! source of truth as `nix/config-schema.json` (Принцип XI) — so a new setting is
//! known here without another table to maintain.

use serde_json::Value;

use crate::ConfigError;

/// What a key holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    String,
    Integer,
    /// A list of strings (`enabled_plugins`).
    StringList,
    /// One of these values.
    Enum(Vec<String>),
    /// A table with these keys (`container`, a host's overrides).
    Table(Vec<String>),
    /// A table keyed by user-chosen names (`hosts`, `plugins`, `shells`).
    Map,
}

/// A key path the schema does not know, with the closest key it does know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unknown {
    pub key: String,
    pub suggestion: Option<String>,
}

fn root() -> Value {
    serde_json::to_value(schemars::schema_for!(crate::Config)).expect("schema always serializes")
}

/// Follow `$ref` and unwrap `anyOf [T, null]` (an optional field).
fn resolve<'a>(root: &'a Value, mut node: &'a Value) -> &'a Value {
    loop {
        if let Some(target) = node.get("$ref").and_then(Value::as_str) {
            let name = target.rsplit('/').next().unwrap_or_default();
            match root.get("$defs").and_then(|d| d.get(name)) {
                Some(def) => node = def,
                None => return node,
            }
        } else if let Some(options) = node.get("anyOf").and_then(Value::as_array) {
            match options
                .iter()
                .find(|o| o.get("type").and_then(Value::as_str) != Some("null"))
            {
                Some(inner) => node = inner,
                None => return node,
            }
        } else {
            return node;
        }
    }
}

/// The node's JSON type, ignoring `null` (an optional field lists both).
fn type_of(node: &Value) -> Option<&str> {
    match node.get("type")? {
        Value::String(t) => Some(t),
        Value::Array(types) => types
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null"),
        _ => None,
    }
}

fn enum_values(node: &Value) -> Option<Vec<String>> {
    let strings = |v: &Value| -> Vec<String> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    };
    if let Some(values) = node.get("enum") {
        return Some(strings(values));
    }
    let options = node.get("oneOf")?.as_array()?;
    let mut out = Vec::new();
    for o in options {
        match (o.get("const").and_then(Value::as_str), o.get("enum")) {
            (Some(c), _) => out.push(c.to_string()),
            (None, Some(values)) => out.extend(strings(values)),
            _ => {}
        }
    }
    Some(out)
}

fn properties(node: &Value) -> Vec<String> {
    node.get("properties")
        .and_then(Value::as_object)
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default()
}

fn kind_of(node: &Value) -> Kind {
    if let Some(values) = enum_values(node) {
        return Kind::Enum(values);
    }
    match type_of(node) {
        Some("integer") => Kind::Integer,
        Some("array") => Kind::StringList,
        Some("object") if node.get("additionalProperties").is_some() => Kind::Map,
        Some("object") => Kind::Table(properties(node)),
        _ => Kind::String,
    }
}

/// Edit distance, for "did you mean".
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { prev } else { prev + 1 };
            prev = row[j + 1];
            row[j + 1] = cost.min(row[j] + 1).min(prev + 1);
        }
    }
    row[b.len()]
}

/// The candidate closest to `name`, if it is close enough to be a typo of it.
fn nearest<'a>(name: &str, candidates: &'a [String]) -> Option<&'a String> {
    let limit = (name.chars().count() / 3).max(2);
    candidates
        .iter()
        .map(|c| (distance(name, c), c))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// Split a dotted key into its segments; a segment with a dot or a space is
/// quoted as in TOML: `hosts."web.example.com".user` (C-G1).
pub fn parse_path(key: &str) -> Result<Vec<String>, ConfigError> {
    let invalid = || ConfigError::InvalidKey {
        key: key.to_string(),
    };
    let keys = toml_edit::Key::parse(key).map_err(|_| invalid())?;
    if keys.is_empty() {
        return Err(invalid());
    }
    Ok(keys.iter().map(|k| k.get().to_string()).collect())
}

/// The dotted form of a path, quoting the segments that need it.
pub fn display_path(path: &[String]) -> String {
    path.iter()
        .map(|s| toml_edit::Key::new(s).to_string())
        .collect::<Vec<_>>()
        .join(".")
}

/// The schema node of `path`, or which segment is unknown.
fn node_of<'a>(root: &'a Value, path: &[String]) -> Result<&'a Value, Unknown> {
    let mut node = resolve(root, root);
    for (i, segment) in path.iter().enumerate() {
        let next = match kind_of(node) {
            Kind::Map => node.get("additionalProperties"),
            Kind::Table(_) => node.get("properties").and_then(|p| p.get(segment)),
            _ => None,
        };
        match next {
            Some(n) => node = resolve(root, n),
            None => {
                let suggestion = nearest(segment, &properties(node)).map(|fixed| {
                    let mut path = path[..i].to_vec();
                    path.push(fixed.clone());
                    display_path(&path)
                });
                return Err(Unknown {
                    key: display_path(&path[..=i]),
                    suggestion,
                });
            }
        }
    }
    Ok(node)
}

/// What `path` holds (C-G2).
pub fn lookup(path: &[String]) -> Result<Kind, Unknown> {
    let root = root();
    node_of(&root, path).map(kind_of)
}

/// The built-in value of `path` when the file does not set it, as TOML.
pub fn default_of(path: &[String]) -> Option<String> {
    let root = root();
    let mut node = resolve(&root, &root);
    let mut default = None;
    for segment in path {
        let property = node.get("properties")?.get(segment)?;
        default = property.get("default").cloned();
        node = resolve(&root, property);
    }
    match default? {
        Value::String(s) => Some(s),
        other => Some(other.to_string()),
    }
}

/// Every key that can be set, for completion (C-G20): `names(map)` gives the
/// entries to list under a user-keyed table (`hosts`, `plugins`, `shells`).
pub fn settable_keys(names: &dyn Fn(&str) -> Vec<String>) -> Vec<String> {
    fn walk(
        root: &Value,
        node: &Value,
        path: &mut Vec<String>,
        names: &dyn Fn(&str) -> Vec<String>,
        out: &mut Vec<String>,
    ) {
        match kind_of(node) {
            Kind::Table(keys) => {
                for key in keys {
                    let Some(child) = node.get("properties").and_then(|p| p.get(&key)) else {
                        continue;
                    };
                    path.push(key);
                    walk(root, resolve(root, child), path, names, out);
                    path.pop();
                }
            }
            Kind::Map => {
                let Some(child) = node.get("additionalProperties") else {
                    return;
                };
                for name in names(&display_path(path)) {
                    path.push(name);
                    walk(root, resolve(root, child), path, names, out);
                    path.pop();
                }
            }
            _ => out.push(display_path(path)),
        }
    }
    let root = root();
    let mut out = Vec::new();
    walk(
        &root,
        resolve(&root, &root),
        &mut Vec::new(),
        names,
        &mut out,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(key: &str) -> Result<Kind, Unknown> {
        lookup(&parse_path(key).unwrap())
    }

    #[test]
    fn kinds_follow_the_config_types() {
        assert_eq!(kind("default_shell"), Ok(Kind::String));
        assert_eq!(kind("user"), Ok(Kind::String), "optional string");
        assert_eq!(kind("connect_timeout_s"), Ok(Kind::Integer));
        assert_eq!(kind("enabled_plugins"), Ok(Kind::StringList));
        assert_eq!(
            kind("cleanup"),
            Ok(Kind::Enum(vec!["ephemeral".into(), "keep".into()]))
        );
        assert_eq!(
            kind("container.runtime"),
            Ok(Kind::Enum(vec![
                "docker".into(),
                "podman".into(),
                "auto".into()
            ]))
        );
        assert_eq!(kind("hosts"), Ok(Kind::Map));
        assert!(
            matches!(kind("hosts.web"), Ok(Kind::Table(keys)) if keys.contains(&"cleanup".to_string()))
        );
        // Optional per-host fields unwrap to the same kinds.
        assert_eq!(
            kind("hosts.web.transport"),
            Ok(Kind::Enum(vec!["russh".into(), "ssh".into()]))
        );
        assert_eq!(kind("hosts.web.connect_timeout_s"), Ok(Kind::Integer));
        assert_eq!(kind("hosts.web.enabled_plugins"), Ok(Kind::StringList));
        assert_eq!(kind("plugins.neovim.source"), Ok(Kind::String));
        assert_eq!(kind("shells.zsh.source"), Ok(Kind::String));
    }

    #[test]
    fn unknown_keys_get_the_nearest_known_one() {
        assert_eq!(
            kind("defualt_shell"),
            Err(Unknown {
                key: "defualt_shell".into(),
                suggestion: Some("default_shell".into())
            })
        );
        assert_eq!(
            kind("hosts.web.clenup"),
            Err(Unknown {
                key: "hosts.web.clenup".into(),
                suggestion: Some("hosts.web.cleanup".into())
            })
        );
        // The first unknown segment is the one reported.
        assert_eq!(
            kind("contaner.runtime").unwrap_err().suggestion.as_deref(),
            Some("container")
        );
        // Nothing close: no guess. A leaf has no keys below it.
        assert_eq!(kind("zzzzzz").unwrap_err().suggestion, None);
        assert_eq!(kind("default_shell.x").unwrap_err().key, "default_shell.x");
    }

    #[test]
    fn paths_quote_what_needs_quoting() {
        let path = parse_path("hosts.\"web.example.com\".user").unwrap();
        assert_eq!(path, ["hosts", "web.example.com", "user"]);
        assert_eq!(display_path(&path), "hosts.\"web.example.com\".user");
        assert_eq!(lookup(&path), Ok(Kind::String));
        assert!(parse_path("").is_err());
        assert!(parse_path("a..b").is_err());
    }

    #[test]
    fn defaults_and_settable_keys() {
        assert_eq!(
            default_of(&["cleanup".into()]).as_deref(),
            Some("ephemeral")
        );
        assert_eq!(
            default_of(&["connect_timeout_s".into()]).as_deref(),
            Some("10")
        );
        assert_eq!(
            default_of(&["container".into(), "runtime".into()]).as_deref(),
            Some("auto")
        );
        assert_eq!(default_of(&["user".into()]), None);

        let keys = settable_keys(&|map| match map {
            "hosts" => vec!["web".to_string()],
            _ => Vec::new(),
        });
        for want in ["default_shell", "container.runtime", "hosts.web.cleanup"] {
            assert!(keys.iter().any(|k| k == want), "{want} in {keys:?}");
        }
        assert!(keys.iter().all(|k| !k.starts_with("plugins.")), "{keys:?}");
    }
}
