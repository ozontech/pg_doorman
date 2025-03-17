{
  description = "RJD command line tool";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs =
    inputs:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      imports = [ inputs.git-hooks.flakeModule ];
      perSystem =
        {
          system,
          pkgs,
          config,
          ...
        }:
        let
          rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          rust-nightly = pkgs.rust-bin.selectLatestNightlyWith (toolchain: toolchain.default);
          pg_doorman =
            rust:
            ((inputs.crane.mkLib pkgs).overrideToolchain rust).buildPackage {
              src = ./.;
              nativeBuildInputs = [ pkgs.pkg-config ];
              buildInputs = with pkgs; [
                openssl
                rust-jemalloc-sys
              ];
            };
          mkShell =
            rust: extra-packages:
            pkgs.mkShell {
              shellHook = config.pre-commit.installationScript;
              inputsFrom = [ (pg_doorman rust) ];
              packages = [ pkgs.cargo-msrv ] ++ extra-packages;
            };
        in
        {
          _module.args.pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [ (import inputs.rust-overlay) ];
          };
          formatter = pkgs.nixfmt-rfc-style;
          packages = {
            inherit pg_doorman rust;
            default = pg_doorman rust;
          };
          devShells = {
            default = mkShell rust [ ];
            nightly = mkShell rust-nightly [ pkgs.cargo-udeps ];
          };
          pre-commit.settings.hooks = {
            nixfmt-rfc-style.enable = true;
            nil.enable = true;
            statix.enable = true;
            flake-checker.enable = true;
            deadnix.enable = true;
            cargo-check.enable = true;
            rustfmt.enable = true;
          };
        };
    };
}
