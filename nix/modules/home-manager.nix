# Home-manager module (T056, §FR-044, C-CM1): per-user declarative config.
# Writes the canonical `~/.config/xxh/config.toml` via xdg.configFile; the tool
# reads only that file — no runtime Nix dependency (Принцип XI).

{ xxhPackage ? null }:
{ config, lib, pkgs, ... }:

let
  common = import ./common.nix { inherit lib pkgs xxhPackage; };
  cfg = config.programs.xxh;
in
{
  options.programs.xxh = common.options;

  config = lib.mkIf cfg.enable {
    home.packages = lib.optional (cfg.package != null) cfg.package;
    xdg.configFile."xxh/config.toml".source = common.render cfg;
  };
}
