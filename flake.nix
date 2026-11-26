{
  description = "Aurora - a native Steam client written in Rust";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { self, nixpkgs, fenix }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = fn: nixpkgs.lib.genAttrs supportedSystems (system: fn system);
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ fenix.overlays.default ];
          };
          # Fenix's minimal profile is the nightly toolchain (cargo, rustc and
          # std): newer than nixpkgs' stable rustc, and no rustup needed.
          toolchain = pkgs.fenix.minimal.toolchain;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = toolchain;
            rustc = toolchain;
          };
        in
        {
          default = self.packages.${system}.aurora;
          aurora = pkgs.callPackage ./packaging/nix { inherit rustPlatform; };
        }
      );
    };
}
