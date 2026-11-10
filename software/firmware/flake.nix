{
  description = "Attinav Rev B firmware (STM32U575, Rust + Embassy) development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        # The toolchain is defined once, in rust-toolchain.toml, so rustup and nix agree.
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
      in
      {
        devShells.default = pkgs.mkShell {
          name = "attinav-firmware";

          packages = [
            rustToolchain

            # Flash + debug over SWD (Tag-Connect pads), streams defmt logs: `cargo run --release`.
            pkgs.probe-rs-tools
            # Stack-overflow protection: flips the memory layout so the stack grows into a fault.
            pkgs.flip-link
            # `cargo size`, `cargo objdump`, `cargo nm` (uses the llvm-tools component above).
            pkgs.cargo-binutils
            # Fallback flashing path through the ROM bootloader (BOOT0 jumper -> USB DFU).
            pkgs.dfu-util
            # Serial terminal for the CDC-ACM / LPUART link during bring-up.
            pkgs.picocom
            # Dependency hygiene (advisory database check).
            pkgs.cargo-audit
            # `make fmt` / `make fmt-check` format the flake itself.
            pkgs.nixpkgs-fmt
          ];

          env = {
            # Verbose defmt output while developing; the release build in CI trims to `info`.
            DEFMT_LOG = "debug";
          };

          shellHook = ''
            echo "attinav firmware shell: $(rustc --version)"
            echo "  cargo build --release           build the firmware (thumbv8m.main-none-eabihf)"
            echo "  cargo run --release             flash + run with defmt logs via probe-rs"
            echo "  cargo test-host                 run the host-side unit tests (core + sensors)"
            echo "  cargo lint                      clippy (pedantic) on every crate"
          '';
        };

        # `nix fmt`
        formatter = pkgs.nixpkgs-fmt;
      });
}
