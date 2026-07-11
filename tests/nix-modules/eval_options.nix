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
    };
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
in
pkgs.runCommand "xxh-nix-module-eval-options"
  {
    inherit badCleanup badTimeout badHostField badUser badRuntime;
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
    echo "eval options: valid accepted, invalid rejected ($badCleanup/$badTimeout/$badHostField/$badUser/$badRuntime)"
    touch $out
  ''
