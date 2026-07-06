{
  # Canonical dev environment + packages + checks (Принцип X).
  # cargo-without-Nix path stays first-class (anti-lock-in): see .github/workflows/cargo.yml.
  description = "xxh — portable shell environment over SSH";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # Pinned Rust toolchain overlay; version comes from rust-toolchain.toml (research R12).
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # Incremental Rust builds in Nix with dependency-layer caching (research R12).
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
      crane,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        # Toolchain pinned via rust-toolchain.toml (single source of truth).
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Keep non-cargo files the build embeds (bootstrap.sh via include_str!).
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter =
            path: type:
            (craneLib.filterCargoSources path type)
            || (builtins.match ".*bootstrap\\.sh$" path != null)
            || (builtins.match ".*config-schema\\.json$" path != null);
        };

        commonArgs = {
          inherit src;
          strictDeps = true;
          nativeBuildInputs = [ pkgs.pkg-config ];
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;
        xxh = craneLib.buildPackage (commonArgs // { inherit cargoArtifacts; });

        # Static/cross client builds (T060, Принципы I/II): one pkgsCross/pkgsStatic
        # infrastructure serves both the client and the ⭐ Nix plugin provider.
        staticFor =
          crossPkgs:
          let
            sp = crossPkgs.pkgsStatic;
            target = sp.stdenv.hostPlatform.rust.rustcTarget;
            staticToolchain = rustToolchain.override { targets = [ target ]; };
            staticCraneLib = (crane.mkLib pkgs).overrideToolchain staticToolchain;
            upperTarget = builtins.replaceStrings [ "-" ] [ "_" ] (pkgs.lib.toUpper target);
          in
          staticCraneLib.buildPackage {
            inherit src;
            strictDeps = true;
            CARGO_BUILD_TARGET = target;
            "CARGO_TARGET_${upperTarget}_LINKER" = "${sp.stdenv.cc.targetPrefix}cc";
            # C build scripts (aws-lc-sys, zstd-sys, blake3) must use the musl
            # cross cc for target code, never the glibc host cc.
            "CC_${builtins.replaceStrings [ "-" ] [ "_" ] target}" = "${sp.stdenv.cc.targetPrefix}cc";
            "AR_${builtins.replaceStrings [ "-" ] [ "_" ] target}" = "${sp.stdenv.cc.targetPrefix}ar";
            TARGET_CC = "${sp.stdenv.cc.targetPrefix}cc";
            TARGET_AR = "${sp.stdenv.cc.targetPrefix}ar";
            HOST_CC = "cc";
            nativeBuildInputs = [
              sp.stdenv.cc # cross cc (prefixed binaries)
              pkgs.stdenv.cc # host cc for build scripts themselves
              pkgs.cmake # aws-lc-sys
            ];
            # musl lacks glibc's *_chk fortify symbols.
            hardeningDisable = [ "fortify" ];
            doCheck = false; # cross tests do not run on the build host
          };
      in
      {
        devShells.default = craneLib.devShell {
          # NOTE: cross C toolchains are intentionally NOT added here — they would
          # shadow the native CC and break native crate C builds (e.g. aws-lc-sys).
          # Cross/static builds use dedicated derivations (pkgsCross/pkgsStatic) in T060.
          packages = with pkgs; [
            rustToolchain
            openssh
            docker-client
          ];
        };

        packages = {
          default = xxh;
          xxh = xxh;
          xxh-static-x86_64 = staticFor pkgs;
          xxh-static-aarch64 = staticFor pkgs.pkgsCross.aarch64-multiplatform;
          xxh-static-armv7 = staticFor pkgs.pkgsCross.armv7l-hf-multiplatform;
        };

        checks = {
          inherit xxh;
          clippy = craneLib.cargoClippy (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets -- --deny warnings";
            }
          );
          fmt = craneLib.cargoFmt { src = commonArgs.src; };
          test = craneLib.cargoTest (commonArgs // { inherit cargoArtifacts; });
          # ⭐ Declarative-module checks (T058/T059): options eval-validate and the
          # MANDATORY round-trip module → config.toml → xxh-config parser (§SC-013/015).
          nix-module-eval = import ./tests/nix-modules/eval_options.nix {
            inherit pkgs;
            inherit (pkgs) lib;
          };
          nix-module-roundtrip = import ./tests/nix-modules/roundtrip.nix {
            inherit pkgs xxh;
            inherit (pkgs) lib;
          };
        };
      }
    )
    // {
      # ⭐ Declarative modules (T056/T057, Принцип XI): generators of the canonical
      # config file; no runtime Nix dependency (§FR-042).
      homeManagerModules.default = import ./nix/modules/home-manager.nix { };
      nixosModules.default = import ./nix/modules/nixos.nix { };
    };
}
