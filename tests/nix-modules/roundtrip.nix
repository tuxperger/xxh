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
          containerRuntime = "podman";
          connectTimeoutS = 42;
          user = "deploy";
          identity = "/keys/global";
          hosts.web.default_shell = "fish";
          hosts.web.connect_timeout_s = 5;
          hosts.web.user = "www";
          hosts.web.container_runtime = "docker";
          files.".gitconfig" = "~/dotfiles/gitconfig";
          files.".config/nvim" = "~/.config/nvim";
          files.".myrc" = { source = "~/.myrc"; env = "MYTOOL_RC"; };
          files.".pgpass" = { source = "~/.pgpass"; env = "PGPASSFILE"; secret = true; };
          hosts.web.files.".gitconfig" = "~/dotfiles/gitconfig-work";
          hosts.web.files.".config/nvim" = false;
          env.EDITOR = "editor-value-9c";
          env.PAGER = "less";
          hosts.web.env.EDITOR = "vi";
          plugins.alpha.source = "https://example.org/alpha.git#v1";
          plugins.beta.source = "nixpkgs:htop";
          shells.zsh.source = "/srv/xxh-shell-zsh";
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
  grep -q 'container_runtime = Podman' global.out
  grep -q 'cleanup           = Keep' global.out
  grep -q 'connect_timeout_s = 42' global.out
  grep -q 'user              = deploy' global.out
  grep -q 'identity          = /keys/global' global.out
  grep -q 'enabled_plugins   = \["alpha", "beta"\]' global.out

  # Per-host override wins where declared, inherits everywhere else.
  xxh config show --host web > web.out
  cat web.out
  grep -q 'shell             = fish' web.out
  grep -q 'connect_timeout_s = 5' web.out
  grep -q 'transport         = Ssh' web.out
  grep -q 'container_runtime = Docker' web.out
  grep -q 'user              = www' web.out
  grep -q 'identity          = /keys/global' web.out

  # 013: declared sources reach the canonical file and parse (C-L12).
  xxh config show > declared.out
  grep -q 'plugins.alpha = https://example.org/alpha.git#v1' declared.out
  grep -q 'plugins.beta = nixpkgs:htop' declared.out
  grep -q 'shells.zsh = /srv/xxh-shell-zsh' declared.out

  # 010: personal files; the host replaces one entry and drops another (C-F13).
  xxh config validate
  grep -q 'files.".gitconfig" = ~/dotfiles/gitconfig$' declared.out
  grep -q 'files.".config/nvim" = ~/.config/nvim$' declared.out
  grep -q 'files.".myrc" = ~/.myrc (env MYTOOL_RC)$' declared.out
  grep -q 'files.".pgpass" = ~/.pgpass (env PGPASSFILE, secret)$' declared.out
  grep -q 'files.".gitconfig" = ~/dotfiles/gitconfig-work$' web.out
  grep -q 'files.".myrc" = ~/.myrc (env MYTOOL_RC)$' web.out
  if grep -q 'files.".config/nvim"' web.out; then exit 1; fi

  # 011: session variables reach the canonical file; names only in the output.
  grep -q '^env.EDITOR = <set>$' declared.out
  grep -q '^env.PAGER = <set>$' declared.out
  grep -q '^env.EDITOR = <set>$' web.out
  grep -q '^EDITOR = "vi"$' $XDG_CONFIG_HOME/xxh/config.toml
  if grep -q editor-value-9c declared.out; then exit 1; fi

  echo "round-trip: module -> config.toml -> xxh-config parser OK"
  touch $out
''
