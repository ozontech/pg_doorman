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
      perSystem =
        { system, pkgs, ... }:
        let
          rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          craneLib = (inputs.crane.mkLib pkgs).overrideToolchain rust;
          pg_doorman = craneLib.buildPackage {
            src = ./.;
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = with pkgs; [
              openssl
              rust-jemalloc-sys
            ];
          };
        in
        {
          _module.args.pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [ (import inputs.rust-overlay) ];
          };
          formatter = pkgs.nixfmt-rfc-style;
          packages = {
            inherit pg_doorman;
            default = pg_doorman;
          };
          devShells.default = pkgs.mkShell { inputsFrom = [ pg_doorman ]; };
        };
    };
}
