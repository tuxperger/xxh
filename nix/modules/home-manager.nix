# Home-manager module (T056, §FR-044, C-CM1): per-user declarative config.
# Writes the canonical `~/.config/xxh/config.toml` via xdg.configFile; the tool
# reads only that file — no runtime Nix dependency (Принцип XI).
#
# 013: declared plugins/shells are installed by `xxh sync` on activation, at the
# versions of `lockFile` when one is given (C-L12).

{ xxhPackage ? null }:
{ config, lib, pkgs, ... }:

let
  common = import ./common.nix { inherit lib pkgs xxhPackage; };
  cfg = config.programs.xxh;
  declaresSomething = cfg.plugins != { } || cfg.shells != { };
in
{
  options.programs.xxh = common.options;

  config = lib.mkIf cfg.enable {
    home.packages = lib.optional (cfg.package != null) cfg.package;
    xdg.configFile."xxh/config.toml".source = common.render cfg;
    xdg.configFile."xxh/xxh.lock" = lib.mkIf (cfg.lockFile != null) {
      source = cfg.lockFile;
    };

    # Installing needs the network and the sources' tools; a failure must not
    # fail the activation — it is reported, and `xxh sync` can be rerun.
    home.activation.xxhSync =
      lib.mkIf (cfg.syncOnActivation && declaresSomething && cfg.package != null)
        (lib.hm.dag.entryAfter [ "writeBoundary" ] ''
          export PATH="${lib.makeBinPath [ pkgs.git pkgs.curl pkgs.openssh pkgs.gnutar pkgs.gzip ]}:/run/current-system/sw/bin:$PATH"
          run ${cfg.package}/bin/xxh sync \
            || echo "xxh: \`xxh sync\` failed during activation; run it by hand to see why" >&2
        '');
  };
}
