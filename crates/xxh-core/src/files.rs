//! Personal files in the session (010 T002/T003, contracts/files.md C-F*).
//!
//! The config's `[files]` names files and directories of this machine by the
//! name a program looks for (`.gitconfig`, `.config/nvim`). Each becomes a
//! content-addressed component like a plugin: it lives under the environment
//! root's cache, and its `env.sh` points the program at the delivered copy —
//! `XDG_CONFIG_HOME`, the program's own variable, or one the user named. The
//! target's home directory is neither read nor written (Принцип I).
//!
//! What goes to someone else's machine is decided here, before connecting
//! (Принцип V): a file that looks like a secret stays home unless its entry says
//! otherwise, a symlink never leads out of the declared directory, and no
//! message ever carries file contents.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use xxh_config::FileSpec;

use crate::ShellError;
use crate::deploy::{Component, ComponentKind};

/// Above this an entry is reported before it is sent (C-F10).
const LARGE_BYTES: u64 = 10 * 1024 * 1024;

/// Symlinked directories are followed this deep at most; a loop ends here.
const MAX_DEPTH: usize = 32;

/// How the variable of a known file refers to the delivered copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Value {
    /// The file or directory itself.
    Path,
    /// `source <file>` — vim runs `VIMINIT` as a command.
    VimSource,
    /// The directory holding the file — curl looks for `.curlrc` in `CURL_HOME`.
    ParentDir,
}

/// Files whose programs can be pointed elsewhere by a variable (C-F6).
const KNOWN: [(&str, &str, Value); 10] = [
    (".gitconfig", "GIT_CONFIG_GLOBAL", Value::Path),
    (".inputrc", "INPUTRC", Value::Path),
    (".vimrc", "VIMINIT", Value::VimSource),
    (".screenrc", "SCREENRC", Value::Path),
    (".wgetrc", "WGETRC", Value::Path),
    (".curlrc", "CURL_HOME", Value::ParentDir),
    (".npmrc", "NPM_CONFIG_USERCONFIG", Value::Path),
    (".psqlrc", "PSQLRC", Value::Path),
    (".ripgreprc", "RIPGREP_CONFIG_PATH", Value::Path),
    (".editrc", "EDITRC", Value::Path),
];

/// How a declared entry becomes visible to programs.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Exposure {
    /// Part of the delivered `.config` tree behind `XDG_CONFIG_HOME`.
    Xdg,
    Var(String, Value),
    /// Nothing reads it from anywhere but the home directory (C-F7).
    None,
}

fn exposure(name: &str, spec: &FileSpec) -> Exposure {
    if let Some(var) = &spec.env {
        return Exposure::Var(var.clone(), Value::Path);
    }
    if name == ".config" || name.starts_with(".config/") {
        return Exposure::Xdg;
    }
    match KNOWN.iter().find(|(known, _, _)| *known == name) {
        Some((_, var, value)) => Exposure::Var((*var).to_string(), *value),
        None => Exposure::None,
    }
}

/// Whether one declaration is well-formed (C-F2). Names and variables end up in
/// shell text run on the target, so anything a double-quoted string would
/// interpret is refused rather than escaped.
pub fn check_entry(name: &str, spec: &FileSpec) -> Result<(), String> {
    let bad = |why: &str| Err(format!("files: `{name}`: {why}"));
    if name.is_empty() || name.starts_with('/') {
        return bad("the name must be a path relative to the home directory");
    }
    if name
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return bad("the name must not contain empty, `.` or `..` segments");
    }
    if name.contains(['"', '`', '$', '\\', '\n', '\r']) {
        return bad("the name must not contain quotes, `$`, backslashes or line breaks");
    }
    if spec.source.is_empty() {
        return bad("the source path is empty");
    }
    if let Some(var) = &spec.env {
        let mut chars = var.chars();
        let head = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
        if !head || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return bad(&format!("`{var}` is not a variable name"));
        }
    }
    Ok(())
}

/// [`check_entry`] for a whole set; the first problem is reported.
pub fn check(files: &BTreeMap<String, FileSpec>) -> Result<(), String> {
    files
        .iter()
        .try_for_each(|(name, spec)| check_entry(name, spec))
}

/// The client path a declaration names: `~/` and relative paths are taken from
/// the home directory.
fn source_path(source: &str, home: &Path) -> PathBuf {
    match source.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if source == "~" => home.to_path_buf(),
        None => home.join(source),
    }
}

/// Whether a file's name alone marks it as a secret (research R4).
fn secret_name(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    const EXACT: [&str; 11] = [
        "id_rsa",
        "id_dsa",
        "id_ecdsa",
        "id_ed25519",
        ".netrc",
        "_netrc",
        ".pgpass",
        ".git-credentials",
        "credentials",
        ".env",
        ".htpasswd",
    ];
    const EXTENSIONS: [&str; 5] = [".pem", ".key", ".kdbx", ".p12", ".pfx"];
    EXACT.contains(&name.as_str())
        || EXTENSIONS.iter().any(|ext| name.ends_with(ext))
        || name.contains("token")
        || name.contains("secret")
}

/// Whether a file starts like a PEM private key. Only the head is read, and
/// nothing of it leaves this function.
fn private_key(path: &Path) -> bool {
    use std::io::Read as _;
    let mut head = [0u8; 80];
    let n = std::fs::File::open(path)
        .and_then(|mut f| f.read(&mut head))
        .unwrap_or(0);
    let head = String::from_utf8_lossy(&head[..n]);
    head.starts_with("-----BEGIN ") && head.contains("PRIVATE KEY")
}

/// The first thing under `path` that looks like a secret, if any (C-F9).
fn secret_in(path: &Path, depth: usize) -> Option<PathBuf> {
    const DIRS: [&str; 3] = [".ssh", ".gnupg", ".aws"];
    let meta = std::fs::metadata(path).ok()?;
    if meta.is_dir() {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if DIRS.contains(&name.as_str()) {
            return Some(path.to_path_buf());
        }
        if depth >= MAX_DEPTH {
            return None;
        }
        let mut children: Vec<PathBuf> = std::fs::read_dir(path)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .collect();
        children.sort();
        return children.iter().find_map(|c| secret_in(c, depth + 1));
    }
    (secret_name(path) || private_key(path)).then(|| path.to_path_buf())
}

fn mode_of(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    meta.permissions().mode() & 0o777
}

fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// What copying an entry found on the way.
#[derive(Default)]
struct Copied {
    bytes: u64,
    /// Symlinks that lead out of the declared directory, left behind (C-F11).
    outside: Vec<PathBuf>,
}

/// Copy `src` to `dst` with its permission bits (§FR-007). `root` is the
/// declared directory: a symlink is followed only while it stays inside it.
fn copy_tree(
    src: &Path,
    dst: &Path,
    root: &Path,
    depth: usize,
    out: &mut Copied,
) -> std::io::Result<()> {
    let meta = std::fs::metadata(src)?;
    if meta.is_file() {
        // `fs::copy` carries the permission bits along.
        out.bytes += std::fs::copy(src, dst)?;
        return Ok(());
    }
    if !meta.is_dir() || depth >= MAX_DEPTH {
        // Sockets, fifos, devices — and a symlink loop — are not configuration.
        return Ok(());
    }
    std::fs::create_dir_all(dst)?;
    let mut children: Vec<PathBuf> = std::fs::read_dir(src)?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|e| e.path())
        .collect();
    children.sort();
    for child in children {
        let Some(name) = child.file_name() else {
            continue;
        };
        if std::fs::symlink_metadata(&child)?.file_type().is_symlink() {
            let inside = std::fs::canonicalize(&child).is_ok_and(|t| t.starts_with(root));
            if !inside {
                out.outside.push(child);
                continue;
            }
        }
        copy_tree(&child, &dst.join(name), root, depth + 1, out)?;
    }
    // The owner must be able to remove what was delivered: a read-only
    // directory would survive the cleanup on the target (Принцип I).
    set_mode(dst, mode_of(&meta) | 0o700)
}

/// The staging directory of one `build`; removed with the value.
#[derive(Debug)]
struct Stage(PathBuf);

impl Drop for Stage {
    fn drop(&mut self) {
        // Read-only files copied in are still removable: directories are u+rwx.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The components of a file set, with what the user should know about them.
#[derive(Debug, Default)]
pub struct Built {
    /// One per entry outside `.config/`, one for everything under it (C-F4).
    pub components: Vec<Component>,
    /// One line each; never any file contents (C-F12).
    pub warnings: Vec<String>,
    /// The components are packed from here on demand; it lives as long as they do.
    _stage: Option<Stage>,
}

fn io_error(what: &str, path: &Path, e: std::io::Error) -> ShellError {
    ShellError::Other(format!("files: {what} {}: {e}", path.display()))
}

/// Build the components for `files` (the effective set of a target), reading
/// the sources relative to `home`. Entries that cannot or must not be delivered
/// are left out with a warning; the session goes on without them (§FR-009).
pub fn build(
    files: &BTreeMap<String, FileSpec>,
    home: &Path,
    fmt: &str,
) -> Result<Built, ShellError> {
    check(files).map_err(ShellError::Other)?;
    let mut built = Built::default();
    if files.is_empty() {
        return Ok(built);
    }
    let stage = std::env::temp_dir().join(format!(
        "xxh-files-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    std::fs::create_dir_all(&stage).map_err(|e| io_error("creating", &stage, e))?;
    set_mode(&stage, 0o700).map_err(|e| io_error("protecting", &stage, e))?;
    let stage = Stage(stage);

    let xdg = stage.0.join("xdg");
    let mut xdg_used = false;
    for (index, (name, spec)) in files.iter().enumerate() {
        let source = source_path(&spec.source, home);
        let how = exposure(name, spec);
        if how == Exposure::None {
            built.warnings.push(format!(
                "files: `{name}` is not delivered: no known way to point its program at a \
                 copy — give the entry `env = \"VAR\"` or put the file under `.config/`"
            ));
            continue;
        }
        if std::fs::metadata(&source).is_err() {
            built.warnings.push(format!(
                "files: `{name}`: {} does not exist on this machine — skipped",
                source.display()
            ));
            continue;
        }
        if !spec.secret {
            if let Some(found) = secret_in(&source, 0) {
                built.warnings.push(format!(
                    "files: `{name}` is not delivered: {} looks like a secret — add \
                     `secret = true` to this entry to send it anyway",
                    found.display()
                ));
                continue;
            }
        }
        // A directory's own canonical path bounds its symlinks; a single file
        // has nothing to bound.
        let root = std::fs::canonicalize(&source).map_err(|e| io_error("reading", &source, e))?;
        let (dir, dest) = match &how {
            Exposure::Xdg => (xdg.clone(), xdg.join(name)),
            _ => {
                let dir = stage.0.join(index.to_string());
                (dir.clone(), dir.join("f").join(name))
            }
        };
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_error("creating", parent, e))?;
        }
        let mut copied = Copied::default();
        copy_tree(&source, &dest, &root, 0, &mut copied)
            .map_err(|e| io_error("copying", &source, e))?;
        for link in &copied.outside {
            built.warnings.push(format!(
                "files: `{name}`: the symlink {} leads outside {} — skipped",
                link.display(),
                source.display()
            ));
        }
        if copied.bytes > LARGE_BYTES {
            built.warnings.push(format!(
                "files: `{name}` is large ({} MiB)",
                copied.bytes / (1024 * 1024)
            ));
        }
        match how {
            Exposure::Xdg => xdg_used = true,
            Exposure::Var(var, value) => {
                let target = match value {
                    Value::Path => format!("\"$XXH_COMPONENT_DIR/f/{name}\""),
                    Value::VimSource => format!("\"source $XXH_COMPONENT_DIR/f/{name}\""),
                    Value::ParentDir => "\"$XXH_COMPONENT_DIR/f\"".to_string(),
                };
                let env = dir.join("env.sh");
                std::fs::write(&env, format!("export {var}={target}\n"))
                    .map_err(|e| io_error("writing", &env, e))?;
                built.components.push(
                    Component::pack_dir(ComponentKind::Config, &dir, fmt)?
                        .with_label(format!("files {name}")),
                );
            }
            Exposure::None => {}
        }
    }
    if xdg_used {
        let env = xdg.join("env.sh");
        std::fs::write(
            &env,
            "export XDG_CONFIG_HOME=\"$XXH_COMPONENT_DIR/.config\"\n",
        )
        .map_err(|e| io_error("writing", &env, e))?;
        built.components.push(
            Component::pack_dir(ComponentKind::Config, &xdg, fmt)?.with_label("files .config"),
        );
    }
    built._stage = Some(stage);
    Ok(built)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A client home with a few files; removed on drop.
    struct Home(PathBuf);

    impl Home {
        fn new(tag: &str) -> Home {
            let dir = std::env::temp_dir().join(format!(
                "xxh-files-test-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos())
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Home(dir)
        }

        fn write(&self, rel: &str, text: &str) -> PathBuf {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn spec(source: &str) -> FileSpec {
        FileSpec {
            source: source.into(),
            ..FileSpec::default()
        }
    }

    fn set(entries: &[(&str, FileSpec)]) -> BTreeMap<String, FileSpec> {
        entries
            .iter()
            .map(|(name, spec)| ((*name).to_string(), spec.clone()))
            .collect()
    }

    /// Unpack a component and return its directory (under `home`).
    fn unpacked(home: &Home, comp: &Component) -> PathBuf {
        let dir = home.0.join(format!("unpacked-{}", comp.hash));
        std::fs::create_dir_all(&dir).unwrap();
        let tar = flate2::read::GzDecoder::new(std::io::Cursor::new(comp.payload().unwrap()));
        tar::Archive::new(tar).unpack(&dir).unwrap();
        dir
    }

    fn label_hashes(built: &Built) -> BTreeMap<String, String> {
        built
            .components
            .iter()
            .map(|c| (c.label.clone(), c.hash.clone()))
            .collect()
    }

    #[test]
    fn declarations_are_checked() {
        let ok = spec("~/x");
        for name in [".gitconfig", ".config/nvim", "a b/c", ".config"] {
            assert_eq!(check_entry(name, &ok), Ok(()), "{name}");
        }
        for name in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a//b",
            "./a",
            "a\"b",
            "a$b",
            "a`b",
            "a\\b",
            "a\nb",
        ] {
            assert!(check_entry(name, &ok).is_err(), "{name:?}");
        }
        assert!(check_entry(".x", &spec("")).is_err());
        let with_env = |var: &str| FileSpec {
            env: Some(var.into()),
            ..spec("~/x")
        };
        assert_eq!(check_entry(".x", &with_env("MY_RC2")), Ok(()));
        assert_eq!(check_entry(".x", &with_env("_x")), Ok(()));
        for var in ["", "2X", "A-B", "A B", "A=B", "A;rm"] {
            let err = check_entry(".x", &with_env(var)).unwrap_err();
            assert!(err.contains("`.x`"), "{err}");
        }
    }

    #[test]
    fn exposure_follows_the_table() {
        let plain = spec("~/x");
        assert_eq!(
            exposure(".gitconfig", &plain),
            Exposure::Var("GIT_CONFIG_GLOBAL".into(), Value::Path)
        );
        assert_eq!(exposure(".config/nvim", &plain), Exposure::Xdg);
        assert_eq!(exposure(".config", &plain), Exposure::Xdg);
        assert_eq!(exposure(".configx", &plain), Exposure::None);
        assert_eq!(exposure(".tmux.conf", &plain), Exposure::None);
        // A named variable wins over both.
        let named = FileSpec {
            env: Some("X".into()),
            ..spec("~/x")
        };
        for name in [".gitconfig", ".config/nvim", ".tmux.conf"] {
            assert_eq!(
                exposure(name, &named),
                Exposure::Var("X".into(), Value::Path)
            );
        }
    }

    #[test]
    fn secrets_are_recognised_by_name_and_by_header() {
        let home = Home::new("secrets");
        for name in [
            "id_ed25519",
            ".netrc",
            "server.pem",
            "api-token.txt",
            ".env",
            "db.KEY",
        ] {
            let path = home.write(&format!("s/{name}"), "x");
            assert!(secret_in(&path, 0).is_some(), "{name}");
        }
        for name in [".gitconfig", "init.lua", "keymap.vim", "environment"] {
            let path = home.write(&format!("p/{name}"), "x");
            assert_eq!(secret_in(&path, 0), None, "{name}");
        }
        let key = home.write("p/deploy", "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n");
        assert_eq!(secret_in(&key, 0), Some(key.clone()));
        let public = home.write("p/deploy.pub", "-----BEGIN PUBLIC KEY-----\n");
        assert_eq!(secret_in(&public, 0), None);
        // Anywhere inside a directory, and whole directories known for secrets.
        assert_eq!(secret_in(&home.0.join("p"), 0), Some(key));
        home.write(".ssh/config", "Host x\n");
        assert_eq!(
            secret_in(&home.0.join(".ssh"), 0),
            Some(home.0.join(".ssh"))
        );
    }

    #[test]
    fn components_carry_the_files_and_point_programs_at_them() {
        let home = Home::new("build");
        let git = home.write("dot/gitconfig", "[user]\n\tname = Me\n");
        set_mode(&git, 0o600).unwrap();
        home.write(".config/tool/conf", "a = 1\n");
        home.write(".config/tool/sub/deep", "b\n");
        home.write(".config/other/x", "c\n");
        home.write(".myrc", "rc\n");
        home.write(".curlrc", "silent\n");
        let files = set(&[
            (".gitconfig", spec("~/dot/gitconfig")),
            (".config/tool", spec("~/.config/tool")),
            (".config/other/x", spec(".config/other/x")),
            (
                ".myrc",
                FileSpec {
                    env: Some("MY_RC".into()),
                    ..spec(home.0.join(".myrc").to_str().unwrap())
                },
            ),
            (".curlrc", spec("~/.curlrc")),
        ]);
        let built = build(&files, &home.0, "gz").unwrap();
        assert_eq!(built.warnings, [""; 0]);
        let labels: Vec<&str> = built.components.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "files .curlrc",
                "files .gitconfig",
                "files .myrc",
                "files .config"
            ]
        );
        let by_label = |label: &str| {
            let comp = built.components.iter().find(|c| c.label == label).unwrap();
            assert_eq!(comp.kind, ComponentKind::Config);
            unpacked(&home, comp)
        };
        let read = |path: PathBuf| std::fs::read_to_string(path).unwrap();

        let dir = by_label("files .gitconfig");
        assert_eq!(
            read(dir.join("env.sh")),
            "export GIT_CONFIG_GLOBAL=\"$XXH_COMPONENT_DIR/f/.gitconfig\"\n"
        );
        assert_eq!(read(dir.join("f/.gitconfig")), "[user]\n\tname = Me\n");
        let mode = mode_of(&std::fs::metadata(dir.join("f/.gitconfig")).unwrap());
        assert_eq!(mode, 0o600, "owner-only stays owner-only (§FR-007)");

        let dir = by_label("files .myrc");
        assert_eq!(
            read(dir.join("env.sh")),
            "export MY_RC=\"$XXH_COMPONENT_DIR/f/.myrc\"\n"
        );
        let dir = by_label("files .curlrc");
        assert_eq!(
            read(dir.join("env.sh")),
            "export CURL_HOME=\"$XXH_COMPONENT_DIR/f\"\n"
        );

        // Everything under `.config/` is one tree behind XDG_CONFIG_HOME.
        let dir = by_label("files .config");
        assert_eq!(
            read(dir.join("env.sh")),
            "export XDG_CONFIG_HOME=\"$XXH_COMPONENT_DIR/.config\"\n"
        );
        assert_eq!(read(dir.join(".config/tool/conf")), "a = 1\n");
        assert_eq!(read(dir.join(".config/tool/sub/deep")), "b\n");
        assert_eq!(read(dir.join(".config/other/x")), "c\n");
    }

    #[test]
    fn addresses_change_only_with_the_entry() {
        let home = Home::new("address");
        home.write(".gitconfig", "one\n");
        home.write(".inputrc", "set bell-style none\n");
        home.write(".config/tool/conf", "a\n");
        let files = set(&[
            (".gitconfig", spec("~/.gitconfig")),
            (".inputrc", spec("~/.inputrc")),
            (".config/tool", spec("~/.config/tool")),
        ]);
        let first = label_hashes(&build(&files, &home.0, "gz").unwrap());
        // Nothing changed: the same addresses, so nothing is sent again (§FR-006).
        assert_eq!(label_hashes(&build(&files, &home.0, "gz").unwrap()), first);

        home.write(".gitconfig", "two\n");
        let second = label_hashes(&build(&files, &home.0, "gz").unwrap());
        assert_ne!(second["files .gitconfig"], first["files .gitconfig"]);
        assert_eq!(second["files .inputrc"], first["files .inputrc"]);
        assert_eq!(second["files .config"], first["files .config"]);
    }

    #[test]
    fn what_must_not_go_stays_home_with_a_warning() {
        let home = Home::new("warn");
        home.write(".gitconfig", "ok\n");
        home.write(".netrc", "machine example login me password hunter2\n");
        home.write(".config/tool/conf", "a\n");
        home.write(
            ".config/tool/deploy",
            "-----BEGIN RSA PRIVATE KEY-----\nSECRETBODY\n",
        );
        home.write(".tmux.conf", "set -g mouse on\n");
        let files = set(&[
            (".gitconfig", spec("~/.gitconfig")),
            (".inputrc", spec("~/.inputrc")),
            (
                ".netrc",
                FileSpec {
                    env: Some("NETRC".into()),
                    ..spec("~/.netrc")
                },
            ),
            (".config/tool", spec("~/.config/tool")),
            (".tmux.conf", spec("~/.tmux.conf")),
        ]);
        let built = build(&files, &home.0, "gz").unwrap();
        let labels: Vec<&str> = built.components.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["files .gitconfig"], "{:?}", built.warnings);
        let all = built.warnings.join("\n");
        assert_eq!(built.warnings.len(), 4, "{all}");
        assert!(
            all.contains("`.inputrc`") && all.contains("does not exist"),
            "{all}"
        );
        assert!(
            all.contains("`.netrc` is not delivered") && all.contains("secret = true"),
            "{all}"
        );
        assert!(
            all.contains("`.config/tool` is not delivered") && all.contains("deploy"),
            "{all}"
        );
        assert!(
            all.contains("`.tmux.conf` is not delivered") && all.contains("env ="),
            "{all}"
        );
        // Not a byte of any file is in the messages (C-F12).
        for content in ["hunter2", "SECRETBODY", "mouse on"] {
            assert!(!all.contains(content), "{all}");
        }

        // With the entry's say-so the secret goes.
        let files = set(&[(
            ".netrc",
            FileSpec {
                env: Some("NETRC".into()),
                secret: true,
                ..spec("~/.netrc")
            },
        )]);
        let built = build(&files, &home.0, "gz").unwrap();
        assert_eq!((built.components.len(), built.warnings.len()), (1, 0));
        // A malformed declaration is an error, not a warning.
        assert!(build(&set(&[("../x", spec("~/.gitconfig"))]), &home.0, "gz").is_err());
        assert!(
            build(&BTreeMap::new(), &home.0, "gz")
                .unwrap()
                .components
                .is_empty()
        );
    }

    #[test]
    fn symlinks_stay_inside_the_declared_directory() {
        let home = Home::new("links");
        home.write("outside/passwords", "TOPSECRET\n");
        home.write(".config/tool/real", "inner\n");
        home.write(".config/tool/dir/x", "x\n");
        let tool = home.0.join(".config/tool");
        std::os::unix::fs::symlink("real", tool.join("alias")).unwrap();
        std::os::unix::fs::symlink(home.0.join("outside/passwords"), tool.join("leak")).unwrap();
        std::os::unix::fs::symlink(home.0.join("outside"), tool.join("leakdir")).unwrap();
        std::os::unix::fs::symlink("..", tool.join("dir/loop")).unwrap();
        std::os::unix::fs::symlink("gone", tool.join("dangling")).unwrap();
        let ro = home.0.join(".config/tool/ro");
        std::fs::create_dir(&ro).unwrap();
        std::fs::write(ro.join("f"), "r\n").unwrap();
        set_mode(&ro, 0o500).unwrap();

        let files = set(&[(".config/tool", spec("~/.config/tool"))]);
        let built = build(&files, &home.0, "gz").unwrap();
        set_mode(&ro, 0o700).unwrap();
        let dir = unpacked(&home, &built.components[0]).join(".config/tool");
        assert_eq!(
            std::fs::read_to_string(dir.join("alias")).unwrap(),
            "inner\n"
        );
        assert!(
            dir.join("alias").symlink_metadata().unwrap().is_file(),
            "a copy"
        );
        assert!(!dir.join("leak").exists() && !dir.join("leakdir").exists());
        assert!(!dir.join("dangling").exists());
        // A loop inside the directory ends; the tree is still delivered.
        assert_eq!(std::fs::read_to_string(dir.join("dir/x")).unwrap(), "x\n");
        // A read-only directory arrives removable (cleanup on the target).
        let mode = mode_of(&std::fs::metadata(dir.join("ro")).unwrap());
        assert_eq!(mode & 0o700, 0o700);

        let all = built.warnings.join("\n");
        assert!(all.contains("leak ") && all.contains("leakdir"), "{all}");
        assert!(all.contains("dangling"), "{all}");
        assert!(!all.contains("TOPSECRET"));
    }

    #[test]
    fn large_entries_are_announced() {
        let home = Home::new("large");
        let big = home.0.join(".config/tool/blob");
        std::fs::create_dir_all(big.parent().unwrap()).unwrap();
        std::fs::write(&big, vec![b'a'; (LARGE_BYTES + 1024 * 1024) as usize]).unwrap();
        let files = set(&[(".config/tool", spec("~/.config/tool"))]);
        let built = build(&files, &home.0, "gz").unwrap();
        assert_eq!(built.components.len(), 1, "still delivered");
        assert_eq!(built.warnings, ["files: `.config/tool` is large (11 MiB)"]);
    }
}
