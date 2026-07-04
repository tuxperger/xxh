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

        commonArgs = {
          src = craneLib.cleanCargoSource ./.;
          strictDeps = true;
          nativeBuildInputs = [ pkgs.pkg-config ];
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;
        xxh = craneLib.buildPackage (commonArgs // { inherit cargoArtifacts; });
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
          # Static/cross variants (pkgsStatic / pkgsCross) land in T060.
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
          # ⭐ declarative-module eval + round-trip land in T058/T059.
        };
      }
    );
}
