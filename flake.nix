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
        shellHook = ''
          if [ -f Cargo.toml ]; then
            echo "Building Rust FFI library..."
            cargo build --release -q
            export LD_LIBRARY_PATH="$PWD/target/release:$LD_LIBRARY_PATH"
          fi
        '';
      in {
        # Default shell: UCM + Rust toolchain only. Builds fast (no restate compile).
        # Use for development, unit tests, and FFI smoke tests.
        devShells.default = pkgs.mkShell {
          buildInputs = [
            pkgs.unison-ucm
            pkgs.cargo
            pkgs.rustc
            pkgs.gcc
          ];
          inherit shellHook;
        };

        # Integration shell: adds restate-server, curl, jq.
        # Use for Stage 4 integration tests. Requires compiling restate from source
        # (no nixpkgs binary cache), so first entry takes ~20 min.
        devShells.integration = pkgs.mkShell {
          buildInputs = [
            pkgs.unison-ucm
            pkgs.cargo
            pkgs.rustc
            pkgs.gcc
            pkgs.restate
            pkgs.curl
            pkgs.jq
          ];
          inherit shellHook;
        };
      });
}
