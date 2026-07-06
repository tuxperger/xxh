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
    connectTimeoutS = 30;
    hosts.web = {
      default_shell = "fish";
      cleanup = "ephemeral";
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
in
pkgs.runCommand "xxh-nix-module-eval-options"
  {
    inherit badCleanup badTimeout badHostField;
  }
  ''
    # The valid declaration rendered a canonical config file.
    test -s ${validToml}
    grep -q 'default_shell = "zsh"' ${validToml}
    grep -q 'cleanup = "keep"' ${validToml}
    grep -q 'transport = "ssh"' ${validToml}
    echo "eval options: valid accepted, invalid rejected ($badCleanup/$badTimeout/$badHostField)"
    touch $out
  ''
