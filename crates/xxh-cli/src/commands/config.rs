//! `xxh config …` — where the config is, what it resolves to, and checking,
//! creating and changing it (T047; 009 T005/T008/T011,
//! contracts/config-commands.md C-G*).
//!
//! `show` prints the *effective* configuration after precedence
//! (flag > per-host > global > default). The config carries no secrets by
//! design, so nothing needs masking here (Принцип V). Nothing here connects
//! anywhere, and every failure is config-class (exit 40, 009 §FR-010).

use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use clap_complete::engine::ArgValueCompleter;
use xxh_config::validate::Warning;
use xxh_config::{CliOverrides, Config, ConfigError, edit, template, validate};
use xxh_plugins::source::SourceSpec;

use crate::complete;

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Print the canonical config file path.
    Path,
    /// Show the effective configuration (with precedence applied).
    Show {
        /// Resolve as for this host alias.
        #[arg(long)]
        host: Option<String>,
    },
    /// Check the config for errors and unknown keys, without connecting anywhere.
    Validate {
        /// Check this file instead of the config a login would read.
        #[arg(value_name = "FILE", value_hint = clap::ValueHint::FilePath)]
        file: Option<PathBuf>,
        /// Treat unknown keys as errors (for CI).
        #[arg(long)]
        strict: bool,
    },
    /// Create a commented config with the default values.
    Init {
        /// Overwrite an existing config.
        #[arg(long)]
        force: bool,
    },
    /// Print one value from the config file.
    Get {
        /// Dotted key, e.g. `default_shell` or `hosts.web.cleanup`.
        #[arg(add = ArgValueCompleter::new(complete::config_keys))]
        key: String,
    },
    /// Set one value in the config file, keeping comments and the rest as is.
    Set {
        /// Dotted key, e.g. `default_shell` or `hosts.web.cleanup`.
        #[arg(add = ArgValueCompleter::new(complete::config_keys))]
        key: String,
        /// The new value; a list is comma-separated.
        value: String,
    },
    /// Remove one value (or a whole table, e.g. `hosts.web`) from the config file.
    Unset {
        /// Dotted key, e.g. `hosts.web.cleanup` or `hosts.web`.
        #[arg(add = ArgValueCompleter::new(complete::config_keys))]
        key: String,
    },
    /// Open the config in $VISUAL / $EDITOR and save it only if it is valid.
    Edit,
}

/// Load the canonical config; a missing file is the built-in default (§FR-022).
/// The per-user file wins over a NixOS-managed system-wide one (§FR-044).
pub fn load() -> Result<Config, ConfigError> {
    Config::load_default()
}

fn user_path() -> Result<PathBuf, ConfigError> {
    Config::default_path()
        .ok_or_else(|| ConfigError::Other("cannot determine the config directory".into()))
}

/// The file a login reads: the user's, else the system-wide one, else none.
fn effective_file() -> Option<PathBuf> {
    Config::default_path()
        .filter(|p| p.is_file())
        .or_else(|| Some(Config::system_path()).filter(|p| p.is_file()))
}

fn read(path: &Path) -> Result<String, ConfigError> {
    std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// The text of `path`; a file that is not there yet is an empty config.
fn read_or_empty(path: &Path) -> Result<String, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(ConfigError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Everything that would stop a login because of the config (009 research R2):
/// the parser's verdict, then the declared sources, read as `plugin add` would.
fn check(text: &str) -> (Option<String>, Vec<Warning>) {
    let report = validate::check(text);
    if report.error.is_some() {
        return (report.error, report.warnings);
    }
    let mut problems = Vec::new();
    if let Some(cfg) = validate::parse(text) {
        let declared = cfg
            .plugins
            .iter()
            .map(|d| ("plugins", d))
            .chain(cfg.shells.iter().map(|d| ("shells", d)));
        for (table, (name, d)) in declared {
            if let Err(e) = SourceSpec::parse(&d.source) {
                problems.push(format!("{table}.{name}.source = \"{}\": {e}", d.source));
            }
        }
    }
    // Declared files, for every host that changes the set (010 C-F2).
    if let Some(cfg) = validate::parse(text) {
        let hosts = std::iter::once("").chain(cfg.hosts.keys().map(String::as_str));
        for host in hosts {
            let files = cfg.resolve(host, &CliOverrides::default()).files;
            if let Err(e) = xxh_core::files::check(&files) {
                if !problems.contains(&e) {
                    problems.push(e);
                }
            }
        }
    }
    let error = (!problems.is_empty()).then(|| problems.join("\n"));
    (error, report.warnings)
}

fn print_warnings(path: &Path, warnings: &[Warning]) {
    for w in warnings {
        let line = w.line.map(|l| format!(":{l}")).unwrap_or_default();
        let hint = w
            .suggestion
            .as_ref()
            .map(|s| format!(" (did you mean `{s}`?)"))
            .unwrap_or_default();
        eprintln!(
            "warning: {}{line}: unknown key `{}`{hint}",
            path.display(),
            w.key
        );
    }
}

fn run_validate(file: Option<&Path>, strict: bool) -> Result<(), ConfigError> {
    let path = match file {
        Some(f) => f.to_path_buf(),
        None => match effective_file() {
            Some(p) => p,
            None => {
                println!("no config file: the built-in defaults apply");
                return Ok(());
            }
        },
    };
    let (error, warnings) = check(&read(&path)?);
    print_warnings(&path, &warnings);
    if let Some(error) = error {
        return Err(ConfigError::Invalid {
            path,
            details: format!(":\n{error}"),
        });
    }
    match warnings.len() {
        0 => println!("{}: ok", path.display()),
        n if strict => {
            return Err(ConfigError::Invalid {
                path,
                details: format!(": {n} unknown key(s), refused by --strict"),
            });
        }
        n => println!("{}: ok ({n} warning(s))", path.display()),
    }
    Ok(())
}

fn run_init(force: bool) -> Result<(), ConfigError> {
    let path = user_path()?;
    if std::fs::symlink_metadata(&path).is_ok() && !force {
        return Err(ConfigError::Exists { path });
    }
    edit::ensure_writable(&path)?;
    edit::write_atomic(&path, template::TEMPLATE)?;
    println!("{}", path.display());
    Ok(())
}

/// Apply `change` to the user's config text and write the result (C-G10..C-G15).
fn change(change: impl FnOnce(&str) -> Result<String, ConfigError>) -> Result<(), ConfigError> {
    let path = user_path()?;
    edit::ensure_writable(&path)?;
    let existed = path.is_file();
    let text = read_or_empty(&path)?;
    let new = change(&text)?;
    if new != text {
        edit::write_atomic(&path, &new)?;
        // A user file replaces the system-wide one as a whole, not key by key.
        let system = Config::system_path();
        if !existed && system.is_file() {
            eprintln!(
                "note: {} now takes the place of {}",
                path.display(),
                system.display()
            );
        }
    }
    Ok(())
}

/// Whether the answer to "edit again? [Y/n]" means yes.
fn wants_retry(answer: &str) -> bool {
    matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    )
}

fn ask_retry() -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    eprint!("edit again? [Y/n] ");
    let mut answer = String::new();
    match std::io::stdin().read_line(&mut answer) {
        Ok(n) if n > 0 => wants_retry(&answer),
        _ => false,
    }
}

/// Write the working copy readable by the user only: it sits next to the config.
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(text.as_bytes())
}

fn run_edit() -> Result<(), ConfigError> {
    let editor = ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.trim().is_empty())
        .ok_or_else(|| {
            ConfigError::Other(
                "no editor is set: set $VISUAL or $EDITOR, e.g. `EDITOR=vi xxh config edit`".into(),
            )
        })?;
    let path = user_path()?;
    edit::ensure_writable(&path)?;
    let original = path.is_file().then(|| read(&path)).transpose()?;
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| ConfigError::Io { path, source }
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(io(dir))?;
    // `.toml` so the editor highlights it; edited aside so a broken file never
    // becomes the config (research R6).
    let copy = dir.join(format!("config.edit-{}.toml", std::process::id()));
    write_private(&copy, original.as_deref().unwrap_or(template::TEMPLATE)).map_err(io(&copy))?;

    let result = edit_copy(&editor, &copy, &path, original.as_deref());
    if result.is_err() {
        let _ = std::fs::remove_file(&copy);
    }
    result
}

/// Run the editor on `copy` until it holds a valid config or the user gives up
/// (C-G17/C-G18); on success `copy` becomes `path`.
fn edit_copy(
    editor: &str,
    copy: &Path,
    path: &Path,
    original: Option<&str>,
) -> Result<(), ConfigError> {
    // `code -w` and the like: a program and its arguments, no shell involved.
    let mut words = editor.split_whitespace();
    let program = words.next().unwrap_or_default();
    loop {
        let status = std::process::Command::new(program)
            .args(words.clone())
            .arg(copy)
            .status()
            .map_err(|e| ConfigError::Other(format!("cannot run the editor `{editor}`: {e}")))?;
        if !status.success() {
            return Err(ConfigError::Other(format!(
                "the editor `{editor}` failed ({status}); the config is unchanged"
            )));
        }
        let text = read(copy)?;
        let (error, warnings) = check(&text);
        print_warnings(path, &warnings);
        match error {
            None if original == Some(text.as_str()) => {
                let _ = std::fs::remove_file(copy);
                println!("unchanged");
                return Ok(());
            }
            None => {
                std::fs::rename(copy, path).map_err(|source| ConfigError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
                println!("saved {}", path.display());
                return Ok(());
            }
            Some(error) => {
                eprintln!("{error}");
                if !ask_retry() {
                    return Err(ConfigError::Invalid {
                        path: path.to_path_buf(),
                        details: ": the edited text was not saved, the config is unchanged".into(),
                    });
                }
            }
        }
    }
}

pub fn run(action: &ConfigAction, cli: &CliOverrides) -> Result<(), ConfigError> {
    match action {
        ConfigAction::Path => {
            match Config::default_path() {
                Some(p) => println!("{}", p.display()),
                None => println!("<cannot determine config directory>"),
            }
            Ok(())
        }
        ConfigAction::Show { host } => {
            let cfg = load()?;
            let alias = host.as_deref().unwrap_or("<global>");
            let eff = cfg.resolve(alias, cli);
            println!("host              = {alias}");
            println!("shell             = {}", eff.shell);
            println!("transport         = {:?}", eff.transport);
            println!("container_runtime = {:?}", eff.container_runtime);
            println!("cleanup           = {:?}", eff.cleanup);
            println!("connect_timeout_s = {}", eff.connect_timeout_s);
            println!(
                "user              = {}",
                eff.user.as_deref().unwrap_or("<ssh-config>")
            );
            println!(
                "identity          = {}",
                eff.identity
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<ssh-config>".into())
            );
            println!("enabled_plugins   = {:?}", eff.enabled_plugins);
            // Personal files for this target (010 C-F3).
            for (name, f) in &eff.files {
                let mut extra = Vec::new();
                if let Some(var) = &f.env {
                    extra.push(format!("env {var}"));
                }
                if f.secret {
                    extra.push("secret".to_string());
                }
                if extra.is_empty() {
                    println!("files.\"{name}\" = {}", f.source);
                } else {
                    println!("files.\"{name}\" = {} ({})", f.source, extra.join(", "));
                }
            }
            // Declared sources (013): what `xxh sync` installs.
            for (name, d) in &cfg.plugins {
                println!("plugins.{name} = {}", d.source);
            }
            for (name, d) in &cfg.shells {
                println!("shells.{name} = {}", d.source);
            }
            Ok(())
        }
        ConfigAction::Validate { file, strict } => run_validate(file.as_deref(), *strict),
        ConfigAction::Init { force } => run_init(*force),
        ConfigAction::Get { key } => {
            let text = match effective_file() {
                Some(path) => read(&path)?,
                None => String::new(),
            };
            println!("{}", edit::get(&text, key)?);
            Ok(())
        }
        ConfigAction::Set { key, value } => {
            change(|text| edit::set(text, key, value))?;
            println!("{key} = {value}");
            Ok(())
        }
        ConfigAction::Unset { key } => {
            change(|text| edit::unset(text, key))?;
            println!("{key} unset");
            Ok(())
        }
        ConfigAction::Edit => run_edit(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_answers() {
        for yes in ["", "\n", "y\n", "Y", " yes \n"] {
            assert!(wants_retry(yes), "{yes:?}");
        }
        for no in ["n\n", "no", "q", "nope"] {
            assert!(!wants_retry(no), "{no:?}");
        }
    }

    /// Declared sources are read the way `plugin add` reads them (research R2).
    #[test]
    fn check_covers_declared_sources() {
        let (error, warnings) = check("[plugins.a]\nsource = \"/src/a\"\n");
        assert_eq!((error, warnings.len()), (None, 0));
        let (error, _) = check("[shells.z]\nsource = \"nixpkgs:\"\n");
        let error = error.expect("a source `plugin add` would refuse");
        assert!(error.contains("shells.z.source"), "{error}");
        // The parser's own error comes first and alone.
        let (error, _) = check("cleanup = \"x\"\n[plugins.a]\nsource = \"nixpkgs:\"\n");
        assert!(error.unwrap().contains("cleanup"));
    }
}
