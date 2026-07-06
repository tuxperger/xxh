# NixOS module (T057, §FR-044, C-CM1): system-wide declarative config.
# Writes the same canonical config file as the home-manager module, at the
# system-wide location `/etc/xxh/config.toml` (Принцип XI: one format, one file).

{ xxhPackage ? null }:
{ config, lib, pkgs, ... }:

let
  common = import ./common.nix { inherit lib pkgs xxhPackage; };
  cfg = config.programs.xxh;
in
{
  options.programs.xxh = common.options;

  config = lib.mkIf cfg.enable {
    environment.systemPackages = lib.optional (cfg.package != null) cfg.package;
    environment.etc."xxh/config.toml".source = common.render cfg;
  };
}
