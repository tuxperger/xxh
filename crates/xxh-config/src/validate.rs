//! Checking a config file without connecting anywhere (009 T004, research R2,
//! contracts/config-commands.md C-G4/C-G5).
//!
//! Errors are whatever would stop a login: the file goes through the very parser
//! a login uses. Unknown keys are not errors — the parser ignores them — but they
//! are almost always typos, so each is reported with its line and the nearest
//! known key.

use toml_edit::{ImDocument, Item, TableLike};

use crate::Config;
use crate::keys::{self, Kind};

/// A key the config types do not know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// 1-based line of the key, when the parser kept it.
    pub line: Option<usize>,
    pub key: String,
    pub suggestion: Option<String>,
}

/// The outcome of checking one config text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Why the file cannot be used: place, offending line and what was expected.
    pub error: Option<String>,
    pub warnings: Vec<Warning>,
}

/// Check `text` as a config file.
pub fn check(text: &str) -> Report {
    let mut report = Report::default();
    if let Err(e) = toml::from_str::<Config>(text) {
        report.error = Some(e.to_string().trim_end().to_string());
    }
    // A file that is not TOML at all has no keys to look at.
    if let Ok(doc) = ImDocument::parse(text) {
        unknown_keys(text, doc.as_table(), &mut Vec::new(), &mut report.warnings);
    }
    report
}

/// `text` as the config a login would get, when it is one.
pub fn parse(text: &str) -> Option<Config> {
    toml::from_str(text).ok()
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

fn unknown_keys(text: &str, table: &dyn TableLike, path: &mut Vec<String>, out: &mut Vec<Warning>) {
    for (name, item) in table.iter() {
        path.push(name.to_string());
        match keys::lookup(path) {
            Ok(Kind::Table(_) | Kind::Map) => {
                if let Some(inner) = item.as_table_like() {
                    unknown_keys(text, inner, path, out);
                }
            }
            Ok(_) => {}
            Err(unknown) => {
                let span = table
                    .get_key_value(name)
                    .and_then(|(key, _)| key.span())
                    .or_else(|| item_span(item));
                out.push(Warning {
                    line: span.map(|s| line_of(text, s.start)),
                    key: unknown.key,
                    suggestion: unknown.suggestion,
                });
            }
        }
        path.pop();
    }
}

fn item_span(item: &Item) -> Option<std::ops::Range<usize>> {
    item.span()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sound_config_is_clean() {
        let report = check(
            "default_shell = \"fish\"\nenabled_plugins = [\"a\"]\n\n[container]\nruntime = \"podman\"\n\n\
             [hosts.web]\ncleanup = \"keep\"\nconnect_timeout_s = 3\n\n\
             [hosts.\"db.internal\"]\nuser = \"dba\"\n\n[plugins.a]\nsource = \"/src/a\"\n",
        );
        assert_eq!(report, Report::default());
        assert_eq!(check(""), Report::default(), "no settings is a config");
    }

    #[test]
    fn a_bad_value_names_place_value_and_choices() {
        let report = check("default_shell = \"zsh\"\n\n[hosts.web]\ncleanup = \"sometimes\"\n");
        let error = report.error.expect("an error");
        assert!(error.contains("line 4"), "{error}");
        assert!(error.contains("cleanup = \"sometimes\""), "{error}");
        assert!(
            error.contains("`ephemeral`") && error.contains("`keep`"),
            "{error}"
        );
        assert!(report.warnings.is_empty());

        // A wrong type and broken syntax are located the same way.
        let error = check("connect_timeout_s = \"soon\"\n").error.unwrap();
        assert!(
            error.contains("line 1") && error.contains("connect_timeout_s"),
            "{error}"
        );
        let error = check("default_shell = \n").error.unwrap();
        assert!(error.contains("line 1"), "{error}");
    }

    #[test]
    fn unknown_keys_are_warnings_with_a_hint() {
        let report = check(
            "# my config\ndefualt_shell = \"fish\"\n\n[hosts.web]\nclenup = \"keep\"\nuser = \"me\"\n\n\
             [contaner]\nruntime = \"podman\"\n\n[hosts.db]\nwhatever = { a = 1 }\n",
        );
        assert_eq!(report.error, None, "the parser ignores unknown keys");
        let got: Vec<(Option<usize>, &str, Option<&str>)> = report
            .warnings
            .iter()
            .map(|w| (w.line, w.key.as_str(), w.suggestion.as_deref()))
            .collect();
        assert_eq!(
            got,
            [
                (Some(2), "defualt_shell", Some("default_shell")),
                (Some(5), "hosts.web.clenup", Some("hosts.web.cleanup")),
                (Some(12), "hosts.db.whatever", None),
                (Some(8), "contaner", Some("container")),
            ]
        );
    }
}
