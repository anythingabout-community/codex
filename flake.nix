{
  description = "Installable Codex CLI for Linux and macOS";

  nixConfig = {
    extra-substituters = [ "https://anythingabout-community.github.io/codex" ];
    extra-trusted-public-keys = [
      "anythingabout-community-codex-1:iqqNKnjHxIt69VbEDd7f2IcMtMjTe3hUs446pl60PFA="
    ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      ...
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      cargo = builtins.fromTOML (builtins.readFile ./codex-rs/Cargo.toml);
      rust = builtins.fromTOML (builtins.readFile ./codex-rs/rust-toolchain.toml);
      version =
        cargo.workspace.package.version
        + nixpkgs.lib.optionalString (
          cargo.workspace.package.version == "0.0.0"
        ) "-dev-${self.shortRev or "dirty"}";
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          toolchain = pkgs.rust-bin.stable.${rust.toolchain.channel}.minimal;
          codex = pkgs.callPackage ./codex-rs {
            inherit version;
            rustPlatform = pkgs.makeRustPlatform {
              cargo = toolchain;
              rustc = toolchain;
            };
          };
        in
        {
          inherit codex;
          default = codex;
          # Keep the existing package name usable by installed profiles.
          codex-rs = codex;
        }
      );

      apps = forAllSystems (system: {
        default = self.apps.${system}.codex;
        codex = {
          type = "app";
          program = "${self.packages.${system}.codex}/bin/codex";
          meta.description = "Codex CLI";
        };
      });

      checks = forAllSystems (system: {
        codex = self.packages.${system}.codex;
      });
    };
}
