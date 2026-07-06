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
- Cleanup is guaranteed: a remote `trap` removes everything on exit (normal or not),
  and a reconcile sweep on the next connect clears leftovers from crashed sessions.
- With `--keep`, the cache survives between sessions and re-entry transfers only
  what changed — typically nothing.

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
xxh <host> [--shell zsh] [--keep] [--transport russh|ssh] [--connect-timeout 10] [-v|-vv|--debug]

xxh config path                 # canonical config file location
xxh config show [--host web]   # effective settings (flag > per-host > global > default)

xxh plugin add <git-url | path | nixpkgs:attr>
xxh plugin enable|disable|update|remove <name>
xxh plugin list [--enabled]
```

Configuration lives in `~/.config/xxh/config.toml` (system-wide fallback:
`/etc/xxh/config.toml`):

```toml
default_shell = "zsh"
enabled_plugins = ["syntax-highlight"]
cleanup = "ephemeral"        # or "keep"
transport = "russh"          # or "ssh"
connect_timeout_s = 10

[hosts.web]
default_shell = "fish"       # per-host overrides beat globals; flags beat both
```

Exit codes are distinguishable by error class: `10` transport, `20` shell,
`30` plugin, `40` config.

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
into a fully static tool delivered to hosts without Nix. Shells themselves are
plugins too (`provides.shell = "zsh"`, see `packages/shells/zsh/`).

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
