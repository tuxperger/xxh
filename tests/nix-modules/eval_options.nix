# Eval tests for the declarative module options (T058, §FR-047, §SC-015, C-CM9).
#
# Valid declarations must evaluate and render; invalid ones must fail at eval —
# errors surface at `nix build`, never at tool runtime. Wired into `flake checks`.

{ pkgs, lib }:

let
  common = import ../../nix/modules/common.nix { inherit lib pkgs; };

  evalCfg =
    declaration:
    (lib.evalModules {
      modules = [
        { options.programs.xxh = common.options; }
        { programs.xxh = declaration; }
      ];
    }).config.programs.xxh;

  # A representative valid declaration exercising every option.
  valid = evalCfg {
    enable = true;
    defaultShell = "zsh";
    enabledPlugins = [ "syntax-highlight" ];
    cleanup = "keep";
    transport = "ssh";
    containerRuntime = "podman";
    connectTimeoutS = 30;
    user = "deploy";
    identity = "/keys/id_ed25519";
    hosts.web = {
      default_shell = "fish";
      cleanup = "ephemeral";
      user = "www";
      identity = "/keys/web";
      container_runtime = "docker";
      files.".config/nvim" = false;
      files.".gitconfig" = "~/dotfiles/gitconfig-work";
    };
    files.".gitconfig" = "~/dotfiles/gitconfig";
    files.".myrc" = { source = "~/.myrc"; env = "MYTOOL_RC"; };
    env.EDITOR = "nvim";
  };
  validToml = common.render valid;

  # Invalid declarations must be rejected by the type system at eval.
  mustFail =
    name: declaration:
    let
      result = builtins.tryEval (builtins.deepSeq (evalCfg declaration) "evaluated");
    in
    if result.success then
      throw "eval test `${name}`: invalid declaration was accepted"
    else
      "ok";

  badCleanup = mustFail "bad-cleanup" {
    enable = true;
    cleanup = "sometimes"; # not in enum [ "ephemeral" "keep" ]
  };
  badTimeout = mustFail "bad-timeout" {
    enable = true;
    connectTimeoutS = "soon"; # not an unsigned int
  };
  badHostField = mustFail "bad-host-transport" {
    enable = true;
    hosts.web.transport = "carrier-pigeon"; # not in enum [ "russh" "ssh" ]
  };
  badUser = mustFail "bad-user" {
    enable = true;
    user = 42; # not a string
  };
  badRuntime = mustFail "bad-runtime" {
    enable = true;
    containerRuntime = "containerd"; # not in enum [ "auto" "docker" "podman" ]
  };
  badFileEnv = mustFail "bad-file-env" {
    enable = true;
    files.".myrc" = { source = "~/.myrc"; env = "MY-RC"; }; # not a variable name
  };
  badFileGlobalFalse = mustFail "bad-file-global-false" {
    enable = true;
    files.".gitconfig" = false; # only a host may drop an entry
  };
  badEnvName = mustFail "bad-env-name" {
    enable = true;
    env."MY-VAR" = "x"; # not a variable name
  };
  badEnvReserved = mustFail "bad-env-reserved" {
    enable = true;
    hosts.web.env.XXH_ROOT = "/x"; # xxh's own
  };
in
pkgs.runCommand "xxh-nix-module-eval-options"
  {
    inherit badCleanup badTimeout badHostField badUser badRuntime badFileEnv badFileGlobalFalse
      badEnvName badEnvReserved;
  }
  ''
    # The valid declaration rendered a canonical config file.
    test -s ${validToml}
    grep -q 'default_shell = "zsh"' ${validToml}
    grep -q 'cleanup = "keep"' ${validToml}
    grep -q 'transport = "ssh"' ${validToml}
    grep -q 'runtime = "podman"' ${validToml}
    grep -q 'user = "deploy"' ${validToml}
    grep -q 'identity = "/keys/id_ed25519"' ${validToml}
    grep -q '".gitconfig" = "~/dotfiles/gitconfig"' ${validToml}
    grep -q 'env = "MYTOOL_RC"' ${validToml}
    grep -q '".config/nvim" = false' ${validToml}
    grep -q 'EDITOR = "nvim"' ${validToml}
    echo "eval options: valid accepted, invalid rejected ($badCleanup/$badTimeout/$badHostField/$badUser/$badRuntime/$badFileEnv/$badFileGlobalFalse/$badEnvName/$badEnvReserved)"
    touch $out
  ''
