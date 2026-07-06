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

    connectTimeoutS = mkOption {
      type = types.ints.unsigned;
      default = 10;
      description = "Connect timeout in seconds (config: connect_timeout_s).";
    };

    hosts = mkOption {
      type = types.attrsOf hostOverride;
      default = { };
      description = "Per-host overrides applied on top of the global settings.";
    };
  };

  # Render the canonical config.toml from an evaluated option set.
  # Field names match crates/xxh-config exactly (round-trip-tested, C-CM10).
  render =
    cfg:
    let
      dropNulls = attrs: lib.filterAttrs (_: v: v != null) attrs;
      settings = {
        default_shell = cfg.defaultShell;
        enabled_plugins = cfg.enabledPlugins;
        cleanup = cfg.cleanup;
        transport = cfg.transport;
        connect_timeout_s = cfg.connectTimeoutS;
      } // lib.optionalAttrs (cfg.hosts != { }) {
        hosts = lib.mapAttrs (_: ho: dropNulls ho) cfg.hosts;
      };
    in
    (pkgs.formats.toml { }).generate "xxh-config.toml" settings;
}
