# Shared option schema for the declarative xxh modules (T055, Принцип XI).
#
# Options mirror the canonical `Config` of crates/xxh-config 1:1 (see
# nix/config-schema.json, generated from the Rust types — the single source of
# truth). The module system only *generates* the canonical config.toml; the tool
# never depends on Nix at runtime (contracts/nix-config-module.md C-CM3/C-CM5).
#
# Invalid declarations fail at eval/`nix build`, not at tool runtime (§FR-047).

{ lib, pkgs, xxhPackage ? null }:

let
  inherit (lib) mkOption mkEnableOption types;

  cleanupType = types.enum [ "ephemeral" "keep" ];
  transportType = types.enum [ "russh" "ssh" ];
  runtimeType = types.enum [ "auto" "docker" "podman" ];

  # A personal file (010 C-F1): the client path, or the path with the variable
  # that gets the delivered copy and the permission to send a secret.
  fileSpec = types.submodule {
    options = {
      source = mkOption {
        type = types.str;
        description = "Path on the client; `~/` is the client's home directory.";
      };
      env = mkOption {
        type = types.nullOr (types.strMatching "[A-Za-z_][A-Za-z0-9_]*");
        default = null;
        description = "Variable that gets the delivered path, for a program without a known one.";
      };
      secret = mkOption {
        type = types.bool;
        default = false;
        description = "Deliver it even though it looks like a secret.";
      };
    };
  };
  fileEntry = types.either types.str fileSpec;
  # On a host, `false` drops a global entry (010 C-F3).
  hostFileEntry = types.oneOf [ (types.enum [ false ]) types.str fileSpec ];
  # Session variables (011 C-E1/C-E2): a variable name, never one of xxh's own.
  envVars = types.addCheck (types.attrsOf types.str) (
    vars:
    lib.all (
      name: builtins.match "[A-Za-z_][A-Za-z0-9_]*" name != null && !lib.hasPrefix "XXH_" name
    ) (builtins.attrNames vars)
  );

  # Per-host overrides: every field optional; null means "inherit global"
  # (mirrors HostOverride; list-valued fields replace, not merge).
  hostOverride = types.submodule {
    options = {
      default_shell = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Shell for this host (overrides the global default).";
      };
      enabled_plugins = mkOption {
        type = types.nullOr (types.listOf types.str);
        default = null;
        description = "Plugin list for this host (replaces the global list).";
      };
      cleanup = mkOption {
        type = types.nullOr cleanupType;
        default = null;
        description = "Cleanup behaviour for this host.";
      };
      transport = mkOption {
        type = types.nullOr transportType;
        default = null;
        description = "Transport backend for this host.";
      };
      connect_timeout_s = mkOption {
        type = types.nullOr types.ints.unsigned;
        default = null;
        description = "Connect timeout (seconds) for this host.";
      };
      user = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Login user for this host (null: ssh-config decides).";
      };
      identity = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Private key (identity file) path for this host.";
      };
      container_runtime = mkOption {
        type = types.nullOr runtimeType;
        default = null;
        description = "Container runtime for this target (container: targets only).";
      };
      files = mkOption {
        type = types.nullOr (types.attrsOf hostFileEntry);
        default = null;
        description = "Personal files for this host, merged over the global set by name; `false` drops an entry.";
      };
      env = mkOption {
        type = types.nullOr envVars;
        default = null;
        description = "Session variables for this host, merged over the global set by name.";
      };
    };
  };
  # A plugin or shell declared with its source (013 C-L1).
  declared = types.submodule {
    options.source = mkOption {
      type = types.str;
      description = "Where to get it — what `xxh plugin add` accepts (git URL, path, nixpkgs:<attr>, flake:…).";
    };
  };
in
rec {
  options = {
    enable = mkEnableOption "xxh — portable shell environment over SSH";

    package = mkOption {
      type = types.nullOr types.package;
      default = xxhPackage;
      description = "The xxh package to install (defaults to this flake's build).";
    };

    defaultShell = mkOption {
      type = types.str;
      default = "zsh";
      description = "Shell delivered to hosts unless overridden (config: default_shell).";
    };

    enabledPlugins = mkOption {
      type = types.listOf types.str;
      default = [ ];
      description = "Globally enabled plugins (config: enabled_plugins).";
    };

    cleanup = mkOption {
      type = cleanupType;
      default = "ephemeral";
      description = "Host cleanup behaviour after a session.";
    };

    transport = mkOption {
      type = transportType;
      default = "russh";
      description = "SSH transport backend.";
    };

    containerRuntime = mkOption {
      type = runtimeType;
      default = "auto";
      description = "Container runtime for container: targets (config: container.runtime).";
    };

    connectTimeoutS = mkOption {
      type = types.ints.unsigned;
      default = 10;
      description = "Connect timeout in seconds (config: connect_timeout_s).";
    };

    user = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "Login user for all hosts (config: user; null: ssh-config decides).";
    };

    identity = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "Private key (identity file) path for all hosts (config: identity).";
    };

    # 010: personal files, by the name a program looks for in the home directory.
    files = mkOption {
      type = types.attrsOf fileEntry;
      default = { };
      example = {
        ".gitconfig" = "~/.gitconfig";
        ".config/nvim" = "~/.config/nvim";
        ".myrc" = { source = "~/.myrc"; env = "MYTOOL_RC"; };
      };
      description = "Personal files made visible in the session (config: [files]).";
    };

    # 011: variables set in every session; `-e` beats them. Values end up in the
    # generated config (and the Nix store) as written — keep secrets out.
    env = mkOption {
      type = envVars;
      default = { };
      example = { EDITOR = "nvim"; };
      description = "Session variables (config: [env]); names starting with XXH_ are reserved.";
    };

    hosts = mkOption {
      type = types.attrsOf hostOverride;
      default = { };
      description = "Per-host overrides applied on top of the global settings.";
    };

    # 013: declared sources; `xxh sync` installs them at the versions the lock
    # file pins. Enabling stays with enabledPlugins.
    plugins = mkOption {
      type = types.attrsOf declared;
      default = { };
      example = { neovim.source = "git@github.com:me/xxh-plugin-neovim.git"; };
      description = "Plugins to install, by name, with their sources (config: [plugins.<name>]).";
    };

    shells = mkOption {
      type = types.attrsOf declared;
      default = { };
      example = { zsh.source = "git@github.com:me/xxh-shell-zsh.git"; };
      description = "Shell packages to install, by shell name, with their sources (config: [shells.<name>]).";
    };

    lockFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      description = ''
        The xxh.lock to install from (keep it next to this configuration). When
        null, `xxh sync` writes ~/.config/xxh/xxh.lock itself; copy it here to
        pin the versions.
      '';
    };

    syncOnActivation = mkOption {
      type = types.bool;
      default = true;
      description = "Run `xxh sync` when the configuration is activated (home-manager).";
    };
  };

  # Render the canonical config.toml from an evaluated option set.
  # Field names match crates/xxh-config exactly (round-trip-tested, C-CM10).
  render =
    cfg:
    let
      dropNulls = attrs: lib.filterAttrs (_: v: v != null) attrs;
      # A file entry as TOML has it: a bare path, or a table without defaults.
      renderFile =
        v:
        if builtins.isAttrs v then
          { inherit (v) source; }
          // lib.optionalAttrs (v.env != null) { inherit (v) env; }
          // lib.optionalAttrs v.secret { secret = true; }
        else
          v;
      renderHost =
        ho:
        dropNulls (
          ho // lib.optionalAttrs (ho.files != null) { files = lib.mapAttrs (_: renderFile) ho.files; }
        );
      settings = {
        default_shell = cfg.defaultShell;
        enabled_plugins = cfg.enabledPlugins;
        cleanup = cfg.cleanup;
        transport = cfg.transport;
        connect_timeout_s = cfg.connectTimeoutS;
        container = { runtime = cfg.containerRuntime; };
      } // lib.optionalAttrs (cfg.user != null) {
        user = cfg.user;
      } // lib.optionalAttrs (cfg.identity != null) {
        identity = cfg.identity;
      } // lib.optionalAttrs (cfg.hosts != { }) {
        hosts = lib.mapAttrs (_: renderHost) cfg.hosts;
      } // lib.optionalAttrs (cfg.files != { }) {
        files = lib.mapAttrs (_: renderFile) cfg.files;
      } // lib.optionalAttrs (cfg.env != { }) {
        inherit (cfg) env;
      } // lib.optionalAttrs (cfg.plugins != { }) {
        plugins = lib.mapAttrs (_: p: { inherit (p) source; }) cfg.plugins;
      } // lib.optionalAttrs (cfg.shells != { }) {
        shells = lib.mapAttrs (_: s: { inherit (s) source; }) cfg.shells;
      };
    in
    (pkgs.formats.toml { }).generate "xxh-config.toml" settings;
}
