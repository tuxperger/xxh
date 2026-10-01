# xxh — portable shell environment over SSH

Bring **your** shell — with your prompt, aliases and plugins — to any host you can
SSH into, without installing anything permanent there. One command, your zsh on a
foreign box; log out and the host is exactly as it was (*zero footprint*).

```console
$ xxh prod-web-3
xxh: ▸ connect prod-web-3
xxh: ▸ detect platform
xxh: ▸ deliver components: sending 2, reused 0
xxh: ▸ shell zsh
prod-web-3 ~ ❯ …            # your own shell, on their machine
prod-web-3 ~ ❯ exit
$ # ~/.xxh on the host is gone
```

## How it works

- The client is a single Rust binary (musl-static builds available). It connects
  over SSH (pure-Rust `russh` by default, or the system `ssh` with `--transport ssh`),
  detects the host platform with a streamed POSIX-`sh` bootstrap (nothing is written
  until the host is known to be supported), then delivers your shell, configs and
  plugins as content-addressed components into `~/.xxh/cache/<blake3>`.
- The same environment works inside **running containers** without an sshd: address
  a container with `docker:<ref>`, `podman:<ref>` or `container:<ref>` and xxh
  drives the runtime's `exec` as the transport (same bootstrap, same cache, same
  guaranteed cleanup — the image and its layers are never modified).
- Cleanup is guaranteed: a remote `trap` removes everything on exit (normal or not),
  and a reconcile sweep on the next connect clears leftovers from crashed sessions.
- A component's address is the hash of its content — file names, permission bits
  and bytes, never timestamps — so the client knows it without packing anything.
  Only components missing on the host are packed (reproducibly) and sent; packed
  archives are kept in `~/.cache/xxh/packed` (override: `XXH_PACK_CACHE_DIR`), so
  a new host does not cost a second compression either.
- With `--keep`, the cache survives between sessions and re-entry transfers and
  packs only what changed — typically nothing.

## Install / build

```sh
# plain cargo (no Nix required)
cargo build --release

# via Nix (canonical, reproducible)
nix build .#xxh                 # native binary
nix build .#xxh-static-x86_64   # static musl binary (also: -aarch64, -armv7)
```

Development: `nix develop` (or direnv `use flake`) gives the pinned toolchain;
plain `cargo build`/`cargo test` also works.

## Usage

```sh
# SSH host (default family)
xxh [user@]<host> [-l user] [-i ~/.ssh/key] [--shell zsh] [--keep] [--transport russh|ssh] [--connect-timeout 10] [-v|-vv|--debug]

# Running container (same flags, minus the SSH-only ones; plus --runtime)
xxh docker:app1                 # a running docker container by name or id
xxh podman:6f0a12               # a running podman container
xxh container:app1 [--runtime docker|podman]   # runtime from flag/config, else auto (docker → podman)

# One command instead of a session (same environment, same cleanup)
xxh <target> -- <command> [args…]   # exact arguments, no shell re-interpretation
xxh <target> -c '<command line>'    # through your shell: pipes, redirections
xxh <target> -t -- <command>        # with a terminal (full-screen programs)

# Will a login work? Checks this machine and, read-only, the target
xxh doctor [<target>] [--json]

# What xxh left on a target, and removing it
xxh status <target> [--json]           # kept environment, last use, size, sessions, components
xxh clean <target> [--stale] [--force] # remove it all, or only what you no longer use

xxh config                     # canonical config file location
xxh config show [--host web]   # effective settings (flag > per-host > global > default)

xxh plugin add <git-url | path | nixpkgs:attr | flake:ref#attr> [--name NAME]
xxh plugin enable|disable|update|remove <name>
xxh plugin list [--enabled]
```

### Container targets

A single positional target addresses either family: a bare `[user@]host` (or
`ssh:host`) is SSH; a `docker:`/`podman:`/`container:` prefix is a running
container reached through that runtime's `exec`. The shared session options
(`--shell`, plugins, `--keep`, `--connect-timeout`, `-l/--user`) apply to both —
for a container, `--user` is the exec user (runtime `-u`). SSH-only options
(`-i/--identity`, `--transport`) are rejected for container targets, and
`--runtime` is rejected for SSH targets, each before any connection.

`container:` resolves its runtime deterministically: `--runtime` flag >
per-target config > `container.runtime` global > auto-order (docker, then podman;
first available). An explicit `docker:`/`podman:` scheme is never silently
substituted — a conflicting `--runtime` is an error. The chosen runtime is shown
with `-v`. A container must have a POSIX `sh` (as any SSH host must); scratch
images without a shell fail with a clear delivery error and no changes.

```console
$ xxh docker:app1
xxh: ▸ runtime docker
xxh: ▸ connect app1
xxh: ▸ detect platform
xxh: ▸ shell zsh
app1 ~ ❯ …                   # your shell, inside their container; the image is untouched
```

Configuration lives in `~/.config/xxh/config.toml` (system-wide fallback:
`/etc/xxh/config.toml`):

```toml
default_shell = "zsh"
enabled_plugins = ["syntax-highlight"]
cleanup = "ephemeral"        # or "keep"
transport = "russh"          # or "ssh" — the SSH client backend
connect_timeout_s = 10
# user / identity are optional; unset means ~/.ssh/config decides
# user = "deploy"
# identity = "/home/me/.ssh/id_ed25519"

[container]
runtime = "auto"             # or "docker" / "podman" — for container: targets

[hosts.web]
default_shell = "fish"       # per-host overrides beat globals; flags beat both
user = "www"                 # login user for this host (like ssh -l)
identity = "~/.ssh/web_key"  # private key for this host (like ssh -i, used exclusively)

[hosts.app1]
container_runtime = "podman" # per-target runtime for the container reference `app1`
```

### Running one command

```console
$ xxh prod-web-3 -- rg -c ERROR /var/log/app.log     # your tools, their machine
$ tar c . | xxh docker:app1 -- tar x -C /tmp/drop      # stdin is the command's stdin
$ xxh prod-web-3 -c 'ps aux | sort -rk4 | head -5' > top.txt
```

The command runs in the same delivered environment as an interactive session
(your plugins on `PATH`) and, without `--keep`, the target is clean again when it
returns. It is built for scripts:

- stdout is the command's stdout, byte for byte; stderr stays separate. xxh's own
  stage lines appear only with `-v`.
- The exit code is the command's (`128 + N` if signal *N* killed it, `255` if
  unknown). An xxh failure is always announced as `xxh: <class>: …` on stderr.
- After `--` the arguments reach the target exactly as given — unlike `ssh`,
  nothing is re-split or re-interpreted. Use `-c` when you *want* a shell.
- Interrupting xxh (Ctrl-C, `SIGTERM`) stops the command on the target, waits for
  the cleanup and exits `130`/`143`.
- With stdin redirected xxh never prompts: needing a password or key passphrase is
  a transport error, not a silent read of your input.

### Before the first login: `xxh doctor`

```console
$ xxh doctor docker:app1
client
  ok    configuration is valid
  ok    zsh package builds: linux-aarch64, linux-x86_64
  …
target app1 (linux/x86_64/musl)
  ok    required tools present: sh, cat, mkdir, chmod, tar, gzip
  warn  `zstd` is missing on the target: deliveries fall back to gzip: larger and slower
        → optional — install `zstd` on the target if that matters
  ok    environment will be created in /root/.xxh
  ok    22411 KiB needed, 119100160 KiB free in /root/.xxh
10 passed, 1 warnings, 0 failed
```

Without a target it checks only this machine (config, enabled plugins, the shell
package, `git`/`nix`, container runtimes) — no network. With one it only *reads*
the target, so the target is unchanged afterwards. Exit code: `0` (warnings
allowed), `1` if any check failed, `10` if the target is unreachable; `--json`
for scripts.

A shell package that has no build for the target's platform is reported at login
too: `xxh: note: …` when the target has that shell anyway, a shell error naming
the available builds when it does not.

### Seeing and removing what xxh left

```console
$ xxh status prod-web-3
/home/me/.xxh — kept, last used 2 h ago, 33.2 MiB
  components:
    current  9b6fa61a2a0a  shell zsh            23.6 MiB
    stale    5c01d2e3f4a5  -                    1.3 MiB
next login: reuses 5, delivers 1 (plugin nix-htop)
$ xxh clean prod-web-3 --stale     # only what your current setup no longer uses
$ xxh clean prod-web-3             # everything; the host is as before your first visit
```

`status` writes nothing to the target — not even on a host xxh has never seen —
and `--json` prints one object for scripts. It looks in every place a login may
have used (`$HOME/.xxh`, `$TMPDIR/.xxh`, `/tmp/.xxh`), only in directories owned by
you. `clean` will not pull an environment from under a running session: it lists
the sessions and exits `50` unless you pass `--force`; it also exits `50` if
something could not be removed, and says what.


Exit codes are distinguishable by error class: `10` transport, `20` shell,
`30` plugin, `40` config, `50` target (`clean` refused or incomplete); `xxh doctor`
exits `1` when a check fails.

## Platform matrix

| | x86_64 | aarch64 | armv7 |
|---|---|---|---|
| Linux (glibc: Debian/Ubuntu, …) | ✅ | ✅ | ✅ |
| Linux (musl/BusyBox: Alpine, …) | ✅ | ✅ | ✅ |
| macOS / BSD hosts | shell delivery planned; unsupported platforms fail cleanly before any write | | |

Host requirements are minimal: POSIX `sh`, `cat`, `mkdir`, `chmod`, `tar`, `gzip`
(zstd is used opportunistically). No root, no package manager, no internet on the host.

## Plugins

A plugin is a directory with a `plugin.toml` manifest ([contract]) and its payload;
`env.sh` is sourced in the remote shell init (`$XXH_COMPONENT_DIR` points at the
plugin's delivered directory):

```toml
name = "syntax-highlight"
version = "1.4.0"
api_version = "1.0.0"                 # semver-checked against the client
targets = ["linux", "darwin"]        # empty = any platform
priority = 5                          # higher loads earlier (ties: by name)

[dependencies]
base-theme = "^2.0"                  # resolved & cycle-checked before deploy

[hooks.post_deploy]                   # pre_connect | post_deploy | pre_exit
run = "hooks/install.sh"             # isolated subprocess, restricted env,
timeout_s = 20                        # failure never kills the session
```

Sources: a git URL (`…#ref` optional), a local path, or — with the ⭐ `nix-source`
feature and Nix on the **client only** — `nixpkgs:<attr>`, built via `pkgsStatic`
into a fully static tool delivered to hosts without Nix. With the same feature,
`flake:<ref>[#<attr>]` takes a program or a ready-made plugin from **any flake** —
see below. Shells themselves are
plugins too (`provides.shell = "zsh"`). First-party packages live in their own
repositories: `xxh-shell-zsh`, `xxh-plugin-zsh-prompt`, `xxh-plugin-neovim`.

### Plugins from a flake ⭐

```sh
xxh plugin add 'flake:github:owner/tool#tool'          # a program some flake exports
xxh plugin add 'flake:github:NixOS/nixpkgs/nixos-25.05#pkgsStatic.ripgrep'
xxh plugin add 'flake:.#my-plugin'                     # a local flake, e.g. your own plugin
xxh plugin add 'flake:github:owner/repo' --name mytool # default output, explicit name
```

The flake is built **on the client**; the host still needs neither Nix nor root.

- **Two output shapes.** An output with a `plugin.toml` at its root is installed as
  that plugin (its own name, version, hooks, dependencies). An output with a `bin/`
  directory is wrapped automatically: its programs land on `PATH`, with terminfo and
  a CA bundle shipped alongside.
- **The output must be self-sufficient.** Dynamically linked binaries and scripts
  whose interpreter lives in `/nix/store` are rejected at `plugin add` — on the
  client, not as a `not found` on the host. Point at a static output
  (`pkgsStatic.<package>`, or a `*-static` package the flake provides).
- **Pinned.** The exact flake revision is recorded at install time and shown by
  `xxh plugin list`; your environment changes only when you run
  `xxh plugin update <name>` (which reports `old -> new`). A local or dirty tree has
  no revision to pin — xxh says so.
- **Per-architecture.** The plugin is tagged with the architecture of its binaries
  and skipped, with a message, on hosts of any other one.
- The flake's own `nixConfig` (extra substituters, keys) is never accepted. Keep
  access tokens out of the reference — use Nix's `access-tokens`/netrc instead;
  credentials that do appear in a reference are redacted from all output.

[contract]: specs/001-portable-shell-over-ssh/contracts/plugin-manifest.md

## Declarative configuration (Nix modules) ⭐

Home-manager and NixOS modules generate the same canonical `config.toml`
(no runtime Nix dependency; invalid declarations fail at `nix build`):

```nix
# flake input `xxh`
programs.xxh = {
  enable = true;
  defaultShell = "zsh";
  enabledPlugins = [ "syntax-highlight" ];
  hosts.web.default_shell = "fish";
};
# HM: imports = [ xxh.homeManagerModules.default ];
# NixOS: imports = [ xxh.nixosModules.default ];
```

A mandatory round-trip flake check (`nix flake check`) proves the module and the
config parser cannot drift apart.

## Testing

Unit tests: `cargo test --workspace --lib`. Integration tests run against **real
sshd containers** (Debian/Ubuntu/Alpine; `XXH_TEST_IMAGE` selects the distro) and
every scenario asserts the host is left clean; they skip gracefully without docker.
CI: `nix flake check` + no-Nix cargo builds + the container matrix (see
`.github/workflows/`).
