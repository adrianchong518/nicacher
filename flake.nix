{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };

    flake-utils.url = "github:numtide/flake-utils";

    naersk = {
      url = "github:nmattia/naersk";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, naersk, rust-overlay, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = (import nixpkgs) {
          inherit system;
          overlays = [
            rust-overlay.overlays.default
          ];
        };

        rust = pkgs.rust-bin.stable.latest.default;
        rust-dev = rust.override {
          extensions = [ "rust-src" ];
        };

        naersk-lib = pkgs.callPackage naersk {
          cargo = rust;
          rustc = rust;
        };

        crateName = "nicacher";
        src = ./.;

        buildInputs = with pkgs; [
          darwin.apple_sdk.frameworks.Security
        ];
      in
      rec {
        packages.default = packages."${crateName}";

        packages."${crateName}" = naersk-lib.buildPackage {
          inherit src buildInputs;
          pname = crateName;
        };

        devShell = pkgs.mkShell {
          buildInputs = with pkgs; [ ] ++ buildInputs;

          nativeBuildInputs = with pkgs; [
            rust-dev
            rust-analyzer

            rnix-lsp
          ];
        };
      }
    );
}
