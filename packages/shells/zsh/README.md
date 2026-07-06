# zsh — first-party shell package

Shells are ordinary packages, not hardcoded into the core (Принцип IV). This
package provides `zsh` via `provides.shell = "zsh"` in `manifest.toml`.

## Layout

```
manifest.toml            # name/version/api_version + provides.shell = "zsh"
fetch.sh                 # recipe: downloads static relocatable builds
dist/<os>-<arch>/        # self-contained tree per platform (bin/zsh, share/, lib/)
```

`dist/` is not committed — run `./fetch.sh` (optionally with a target list, e.g.
`./fetch.sh linux-x86_64`) to populate it from romkatv/zsh-bin static releases
(musl-static on Linux ⇒ works on both glibc and musl hosts).

## Install for use by xxh

Copy or symlink this directory into the shell search path:

```
mkdir -p ~/.local/share/xxh/shells
ln -s "$PWD" ~/.local/share/xxh/shells/zsh
```

At session time xxh packs `dist/<detected-platform>/` as one content-addressed
Shell component, delivers it to `~/.xxh/cache/<hash>/` on the host, prepends its
`bin/` to `PATH`, and launches the shell. If the platform tree is missing and the
host has no `zsh` either, the session fails with a shell-class error (exit 20)
before anything is written to the host (§FR-011).
