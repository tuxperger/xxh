//! Reading and changing single values of the config text (009 T008/T010,
//! research R3/R4, contracts/config-commands.md C-G9..C-G15).
//!
//! The file is the user's: a change touches the one value it is about and leaves
//! every comment, blank line and ordering as they were (SC-003). Nothing is
//! returned for writing unless the result still parses as a config (§FR-007).

use std::path::Path;

use toml_edit::{Array, DocumentMut, Item, Table, Value};

use crate::keys::{self, Kind, Unknown};
use crate::{Config, ConfigError};

fn unknown(u: Unknown) -> ConfigError {
    ConfigError::UnknownKey {
        key: u.key,
        hint: u
            .suggestion
            .map(|s| format!(" (did you mean `{s}`?)"))
            .unwrap_or_default(),
    }
}

fn document(text: &str) -> Result<DocumentMut, ConfigError> {
    text.parse::<DocumentMut>()
        .map_err(|e| ConfigError::Unparsable(e.to_string().trim_end().to_string()))
}

/// The path and kind of a key that holds a value (not a table).
fn value_key(key: &str) -> Result<(Vec<String>, Kind), ConfigError> {
    let path = keys::parse_path(key)?;
    match keys::lookup(&path).map_err(unknown)? {
        Kind::Table(children) => Err(ConfigError::InvalidKey {
            key: format!(
                "{key}` is a table; use one of its keys: `{}",
                children
                    .iter()
                    .map(|c| format!("{key}.{c}"))
                    .collect::<Vec<_>>()
                    .join("`, `")
            ),
        }),
        Kind::Map => Err(ConfigError::InvalidKey {
            key: format!("{key}` is a table; name an entry and its key: `{key}.<name>.<key>"),
        }),
        kind => Ok((path, kind)),
    }
}

/// `value` as typed on the command line, converted to what `kind` holds (C-G10).
fn convert(key: &str, value: &str, kind: &Kind) -> Result<Value, ConfigError> {
    let invalid = |expected: String| ConfigError::InvalidValue {
        key: key.to_string(),
        value: value.to_string(),
        expected,
    };
    match kind {
        Kind::Integer => value
            .parse::<i64>()
            .ok()
            .filter(|n| *n >= 0)
            .map(Value::from)
            .ok_or_else(|| invalid("expected a non-negative whole number".into())),
        Kind::StringList => Ok(Value::Array(Array::from_iter(
            value.split(',').map(str::trim).filter(|s| !s.is_empty()),
        ))),
        Kind::Enum(allowed) if !allowed.iter().any(|a| a == value) => Err(invalid(format!(
            "expected one of `{}`",
            allowed.join("`, `")
        ))),
        _ => Ok(Value::from(value)),
    }
}

/// The item at `path`, if the document has it.
fn find<'a>(doc: &'a DocumentMut, path: &[String]) -> Option<&'a Item> {
    let mut item = doc.as_item();
    for segment in path {
        item = item.as_table_like()?.get(segment)?;
    }
    Some(item)
}

/// The value of `key` in the config `text`: a list is one element per line (C-G9).
pub fn get(text: &str, key: &str) -> Result<String, ConfigError> {
    let (path, _) = value_key(key)?;
    let doc = document(text)?;
    match find(&doc, &path).and_then(Item::as_value) {
        Some(Value::String(s)) => Ok(s.value().clone()),
        Some(Value::Array(items)) => Ok(items
            .iter()
            .map(|v| match v {
                Value::String(s) => s.value().clone(),
                other => other.to_string().trim().to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n")),
        Some(other) => Ok(other.clone().decorated("", "").to_string()),
        None => Err(ConfigError::NotSet {
            key: key.to_string(),
            default: keys::default_of(&path)
                .map(|d| format!(" (default: {d})"))
                .unwrap_or_default(),
        }),
    }
}

/// Put `value` at `path`, creating the tables on the way; an existing value is
/// replaced in place and keeps its comment.
fn put(doc: &mut DocumentMut, key: &str, path: &[String], value: Value) -> Result<(), ConfigError> {
    let not_a_table = || ConfigError::InvalidKey {
        key: format!("{key}` cannot be set: a parent of it is not a table in this file; `{key}"),
    };
    let (leaf, parents) = path.split_last().expect("a parsed path is not empty");
    let mut table = doc.as_table_mut() as &mut dyn toml_edit::TableLike;
    for (depth, segment) in parents.iter().enumerate() {
        if !table.contains_key(segment) {
            let mut new = Table::new();
            // `[hosts.web]` without an empty `[hosts]` above it.
            new.set_implicit(depth + 1 < parents.len());
            table.insert(segment, Item::Table(new));
        }
        table = table
            .get_mut(segment)
            .and_then(Item::as_table_like_mut)
            .ok_or_else(not_a_table)?;
    }
    match table.get_mut(leaf) {
        Some(Item::Value(old)) => {
            let decor = old.decor().clone();
            *old = value;
            *old.decor_mut() = decor;
        }
        Some(_) => return Err(not_a_table()),
        None => {
            table.insert(leaf, Item::Value(value));
        }
    }
    Ok(())
}

/// The text is still a config a login would accept.
fn accepted(text: String, key: &str, value: &str) -> Result<String, ConfigError> {
    match toml::from_str::<Config>(&text) {
        Ok(_) => Ok(text),
        Err(e) => Err(ConfigError::InvalidValue {
            key: key.to_string(),
            value: value.to_string(),
            expected: e.message().to_string(),
        }),
    }
}

/// `text` with `key` set to `value` (C-G10..C-G12).
pub fn set(text: &str, key: &str, value: &str) -> Result<String, ConfigError> {
    let (path, kind) = value_key(key)?;
    let converted = convert(key, value, &kind)?;
    let mut doc = document(text)?;
    put(&mut doc, key, &path, converted)?;
    accepted(doc.to_string(), key, value)
}

/// `text` with the list `key` set to `values` (for `xxh plugin enable|disable`).
pub fn set_list(text: &str, key: &str, values: &[String]) -> Result<String, ConfigError> {
    let (path, kind) = value_key(key)?;
    if kind != Kind::StringList {
        return Err(ConfigError::InvalidKey {
            key: format!("{key}` is not a list; `{key}"),
        });
    }
    let mut doc = document(text)?;
    put(&mut doc, key, &path, Value::Array(Array::from_iter(values)))?;
    accepted(doc.to_string(), key, &values.join(","))
}

/// `text` without `key`; a table may be named to drop it whole, and tables left
/// empty by the removal go with it (C-G13). An absent key is not an error.
pub fn unset(text: &str, key: &str) -> Result<String, ConfigError> {
    let path = keys::parse_path(key)?;
    keys::lookup(&path).map_err(unknown)?;
    let mut doc = document(text)?;
    fn remove(table: &mut dyn toml_edit::TableLike, path: &[String]) -> bool {
        let Some((first, rest)) = path.split_first() else {
            return false;
        };
        if rest.is_empty() {
            return table.remove(first).is_some();
        }
        let Some(inner) = table.get_mut(first).and_then(Item::as_table_like_mut) else {
            return false;
        };
        let removed = remove(inner, rest);
        if removed && inner.is_empty() {
            table.remove(first);
        }
        removed
    }
    if !remove(doc.as_table_mut(), &path) {
        return Ok(text.to_string());
    }
    accepted(doc.to_string(), key, "")
}

/// Refuse to change a config that something else owns (C-G15, §FR-009): a
/// symlink (Home Manager links the file into the Nix store) or a file or
/// directory this user cannot write. Checked before anything is changed.
pub fn ensure_writable(path: &Path) -> Result<(), ConfigError> {
    let managed = |reason: String| ConfigError::Managed {
        path: path.to_path_buf(),
        reason,
    };
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::read_link(path)
                .map(|t| t.display().to_string())
                .unwrap_or_else(|_| "?".into());
            return Err(managed(format!("a symlink to {target}")));
        }
        Ok(meta) if meta.permissions().readonly() => {
            return Err(managed("the file is read-only".into()));
        }
        _ => {}
    }
    // The nearest existing directory decides whether a write can happen at all.
    let mut dir = path.parent();
    while let Some(d) = dir {
        match std::fs::metadata(d) {
            Ok(meta) if meta.permissions().readonly() => {
                return Err(managed(format!(
                    "the directory {} is read-only",
                    d.display()
                )));
            }
            Ok(_) => break,
            Err(_) => dir = d.parent(),
        }
    }
    Ok(())
}

/// Replace `path` with `text` in one step: a reader sees the old file or the new
/// one, never a part (C-G14). The directory is created when missing.
pub fn write_atomic(path: &Path, text: &str) -> Result<(), ConfigError> {
    let io = |source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(io)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io(e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "\
# My xxh config.
default_shell = \"zsh\"   # the one I like

# plugins, in order
enabled_plugins = [
    \"prompt\", # first
    \"nvim\",
]

[hosts.web]
# web is slow
connect_timeout_s = 30

[container]
runtime = \"auto\"
";

    #[test]
    fn set_changes_one_value_and_nothing_else() {
        let out = set(FILE, "default_shell", "fish").unwrap();
        assert_eq!(
            out,
            FILE.replace("\"zsh\"   # the one I like", "\"fish\"   # the one I like")
        );
        let out = set(FILE, "hosts.web.connect_timeout_s", "5").unwrap();
        assert_eq!(out, FILE.replace("= 30", "= 5"));
        let out = set(FILE, "container.runtime", "podman").unwrap();
        assert_eq!(out, FILE.replace("\"auto\"", "\"podman\""));
        // Setting what is already there changes nothing.
        assert_eq!(set(FILE, "default_shell", "zsh").unwrap(), FILE);
    }

    #[test]
    fn set_adds_keys_and_tables_where_they_belong() {
        // A new global goes with the globals, above the first table.
        let out = set(FILE, "cleanup", "keep").unwrap();
        assert!(out.starts_with("# My xxh config.\n"), "{out}");
        let cleanup = out.find("cleanup = \"keep\"").expect("added");
        assert!(cleanup < out.find("[hosts.web]").unwrap(), "{out}");
        assert_eq!(out.replace("cleanup = \"keep\"\n", ""), FILE);

        // A new key of an existing table joins that table.
        let out = set(FILE, "hosts.web.default_shell", "fish").unwrap();
        assert!(
            out.contains("# web is slow\nconnect_timeout_s = 30\ndefault_shell = \"fish\"\n"),
            "{out}"
        );

        // A new host gets its own table; names with dots are quoted.
        let out = set(FILE, "hosts.\"db.internal\".user", "dba").unwrap();
        // It joins the other hosts; the rest is untouched.
        assert_eq!(
            out,
            FILE.replace(
                "[container]",
                "[hosts.\"db.internal\"]\nuser = \"dba\"\n\n[container]"
            )
        );

        // From nothing.
        assert_eq!(
            set("", "default_shell", "fish").unwrap(),
            "default_shell = \"fish\"\n"
        );
        assert_eq!(
            set("", "plugins.prompt.source", "/src/prompt").unwrap(),
            "[plugins.prompt]\nsource = \"/src/prompt\"\n"
        );
    }

    #[test]
    fn lists_come_comma_separated() {
        let out = set("", "enabled_plugins", "a, b,c").unwrap();
        assert_eq!(out, "enabled_plugins = [\"a\", \"b\", \"c\"]\n");
        assert_eq!(
            set("", "enabled_plugins", "").unwrap(),
            "enabled_plugins = []\n"
        );
        assert_eq!(get(&out, "enabled_plugins").unwrap(), "a\nb\nc");
        let out = set_list(FILE, "enabled_plugins", &["x".to_string()]).unwrap();
        assert!(
            out.contains("# plugins, in order\nenabled_plugins = [\"x\"]\n"),
            "{out}"
        );
        assert!(out.contains("# the one I like"), "{out}");
        assert!(set_list(FILE, "default_shell", &[]).is_err());
    }

    #[test]
    fn bad_input_is_refused() {
        let err = set(FILE, "cleanup", "sometimes").unwrap_err().to_string();
        assert!(
            err.contains("`sometimes`") && err.contains("`cleanup`"),
            "{err}"
        );
        assert!(err.contains("`ephemeral`, `keep`"), "{err}");
        let err = set(FILE, "connect_timeout_s", "soon")
            .unwrap_err()
            .to_string();
        assert!(err.contains("whole number"), "{err}");
        assert!(set(FILE, "connect_timeout_s", "-1").is_err());
        let err = set(FILE, "defualt_shell", "fish").unwrap_err().to_string();
        assert!(err.contains("did you mean `default_shell`"), "{err}");
        let err = set(FILE, "hosts.web", "x").unwrap_err().to_string();
        assert!(err.contains("hosts.web.cleanup"), "{err}");
        assert!(set(FILE, "hosts", "x").is_err());
        assert!(set(FILE, "a..b", "x").is_err());
        // A file that is not TOML is not rewritten.
        assert!(matches!(
            set("default_shell = \n", "cleanup", "keep"),
            Err(ConfigError::Unparsable(_))
        ));
    }

    #[test]
    fn get_reads_the_file_not_the_defaults() {
        assert_eq!(get(FILE, "default_shell").unwrap(), "zsh");
        assert_eq!(get(FILE, "hosts.web.connect_timeout_s").unwrap(), "30");
        assert_eq!(get(FILE, "enabled_plugins").unwrap(), "prompt\nnvim");
        let err = get(FILE, "cleanup").unwrap_err().to_string();
        assert!(
            err.contains("not set") && err.contains("default: ephemeral"),
            "{err}"
        );
        let err = get(FILE, "user").unwrap_err().to_string();
        assert!(err.contains("not set") && !err.contains("default"), "{err}");
        assert!(get(FILE, "hosts.web").is_err(), "a table has no value");
        assert!(get(FILE, "nope").is_err());
    }

    #[test]
    fn unset_removes_keys_and_emptied_tables() {
        let out = unset(FILE, "default_shell").unwrap();
        assert!(!out.contains("default_shell"), "{out}");
        assert!(out.contains("# plugins, in order"), "{out}");
        // The only key of the host: its table goes too, comment and all.
        let out = unset(FILE, "hosts.web.connect_timeout_s").unwrap();
        assert!(!out.contains("hosts"), "{out}");
        assert!(out.contains("[container]\nruntime = \"auto\"\n"), "{out}");
        // A whole table by name.
        assert_eq!(unset(FILE, "hosts.web").unwrap(), out);
        // Not there: nothing to do, not an error.
        assert_eq!(unset(FILE, "cleanup").unwrap(), FILE);
        assert_eq!(unset(FILE, "hosts.db.user").unwrap(), FILE);
        assert!(unset(FILE, "nope").is_err());
    }

    #[test]
    fn managed_files_are_refused_and_writes_are_whole() {
        let dir = std::env::temp_dir().join(format!(
            "xxh-config-edit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let path = dir.join("sub/config.toml");
        // Nothing exists yet: writable, and the directory is created on write.
        ensure_writable(&path).unwrap();
        write_atomic(&path, "a").unwrap();
        write_atomic(&path, "default_shell = \"sh\"\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "default_shell = \"sh\"\n"
        );
        assert_eq!(
            std::fs::read_dir(dir.join("sub")).unwrap().count(),
            1,
            "no temp left"
        );
        ensure_writable(&path).unwrap();

        let link = dir.join("managed.toml");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let err = ensure_writable(&link).unwrap_err().to_string();
        assert!(
            err.contains("managed elsewhere") && err.contains("symlink"),
            "{err}"
        );

        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms).unwrap();
        assert!(matches!(
            ensure_writable(&path),
            Err(ConfigError::Managed { .. })
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
