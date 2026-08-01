{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    flake-utils = {
      url = "github:numtide/flake-utils";
    };
  };

  outputs = { nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
        };
        name = "lyrtui";
        package = pkgs.rustPlatform.buildRustPackage {
          inherit name;
          pname = name;
          meta.mainProgram = name;

          src = pkgs.lib.cleanSource ./.;
          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          nativeBuildInputs = [
            pkgs.pkg-config
          ];

          buildInputs = [
            pkgs.openssl
          ];
        };
      in
      {
        apps.default = {
          type = "app";
          program = "${pkgs.lib.getExe package}";
        };
      }
    );
}





