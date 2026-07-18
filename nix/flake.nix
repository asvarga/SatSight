{
  description = "SatSight — a bidirectional SAT-reduction library with an egui/eframe demo.";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
      in
      {
        # A dev shell for the pure-Rust workspace. `.envrc` loads it via direnv.
        # rustup honours ./rust-toolchain.toml, so everyone gets the same pinned
        # toolchain; the repo's `bin/` is put on PATH by `.envrc` (PATH_add bin).
        devShells.default = pkgs.mkShellNoCC {
          packages = [
            # Toolchain proxy; the actual toolchain (and the wasm32 target) is
            # pinned by rust-toolchain.toml at the repo root.
            pkgs.rustup
            # `bin/main` runs the GUI under `cargo watch` for hot reloading.
            pkgs.cargo-watch
            # `bin/web` serves the wasm build (plan §9); wasm-bindgen + wasm-opt
            # come bundled with trunk's tooling.
            pkgs.trunk
          ];

          RUST_BACKTRACE = "1";
        };
      }
    );
}
