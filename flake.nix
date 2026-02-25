{
  description = "Nix flake for bentobox development and bentoctl packaging";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    systems.url = "github:nix-systems/default";
  };

  outputs =
    {
      self,
      nixpkgs,
      systems,
    }:
    let
      forEachSystem = nixpkgs.lib.genAttrs (import systems);
    in
    {
      packages = forEachSystem (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
          };
          bentoctlToml = fromTOML (builtins.readFile ./crates/bentoctl/Cargo.toml);
          pname = bentoctlToml.package.name;
          version = bentoctlToml.package.version or "0.1.0";
        in
        {
          bentoctl = pkgs.rustPlatform.buildRustPackage {
            inherit pname version;
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [
              "-p"
              "bentoctl"
            ];

            postFixup = pkgs.lib.optionalString pkgs.stdenv.isDarwin ''
              /usr/bin/codesign -f --entitlements ${./app.entitlements} -s - "$out/bin/bentoctl"
              /usr/bin/codesign --verify --verbose=4 "$out/bin/bentoctl"
            '';
          };

          default = self.packages.${system}.bentoctl;
        }
      );

      apps = forEachSystem (system: {
        bentoctl = {
          type = "app";
          program = "${self.packages.${system}.bentoctl}/bin/bentoctl";
        };

        default = self.apps.${system}.bentoctl;
      });

      devShells = forEachSystem (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
          };
        in
        {
          default = pkgs.mkShell {
            packages = [
              pkgs.cargo
              pkgs.rustc
              pkgs.rust-analyzer
              pkgs.docker
              pkgs.vfkit
            ];

            shellHook = ''
              echo "Entering bentobox dev shell (Rust + docker + vfkit)."
            '';
          };
        }
      );

      defaultPackage = forEachSystem (system: self.packages.${system}.default);
      defaultApp = forEachSystem (system: self.apps.${system}.default);
    };
}
