{
  description = "Restate SDK for Unison";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          config.allowUnfreePredicate = pkg: builtins.elem (nixpkgs.lib.getName pkg) [ "restate" ];
        };
      in {
        devShells.default = pkgs.mkShell {
          buildInputs = [
            pkgs.unison-ucm
            pkgs.cargo
            pkgs.rustc
            pkgs.gcc
            pkgs.restate        # Restate server for integration tests
            pkgs.curl           # register endpoint + send test invocations
            pkgs.jq             # inspect JSON responses
          ];

          # After building, expose the .so so UCM can find it via openByName
          shellHook = ''
            if [ -f Cargo.toml ]; then
              echo "Building Rust FFI library..."
              cargo build --release -q
              export LD_LIBRARY_PATH="$PWD/target/release:$LD_LIBRARY_PATH"
            fi
          '';
        };
      });
}
