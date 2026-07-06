# MANDATORY round-trip test (T059, §SC-013, C-CM10): declarative module →
# generated config.toml → the real xxh-config parser (via the built `xxh` binary).
# The effective configuration reported by the tool must match the declaration,
# proving the module and the parser cannot drift apart. Wired into `flake checks`.

{ pkgs, lib, xxh }:

let
  common = import ../../nix/modules/common.nix { inherit lib pkgs; };

  declared = (lib.evalModules {
    modules = [
      { options.programs.xxh = common.options; }
      {
        programs.xxh = {
          enable = true;
          defaultShell = "bash";
          enabledPlugins = [ "alpha" "beta" ];
          cleanup = "keep";
          transport = "ssh";
          connectTimeoutS = 42;
          hosts.web.default_shell = "fish";
          hosts.web.connect_timeout_s = 5;
        };
      }
    ];
  }).config.programs.xxh;

  configToml = common.render declared;
in
pkgs.runCommand "xxh-nix-module-roundtrip" { nativeBuildInputs = [ xxh ]; } ''
  export HOME=$TMPDIR
  export XDG_CONFIG_HOME=$TMPDIR/.config
  mkdir -p $XDG_CONFIG_HOME/xxh
  cp ${configToml} $XDG_CONFIG_HOME/xxh/config.toml

  # Global resolution reflects the declaration.
  xxh config show > global.out
  cat global.out
  grep -q 'shell             = bash' global.out
  grep -q 'transport         = Ssh' global.out
  grep -q 'cleanup           = Keep' global.out
  grep -q 'connect_timeout_s = 42' global.out
  grep -q 'enabled_plugins   = \["alpha", "beta"\]' global.out

  # Per-host override wins where declared, inherits everywhere else.
  xxh config show --host web > web.out
  cat web.out
  grep -q 'shell             = fish' web.out
  grep -q 'connect_timeout_s = 5' web.out
  grep -q 'transport         = Ssh' web.out

  echo "round-trip: module -> config.toml -> xxh-config parser OK"
  touch $out
''
