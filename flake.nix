{
  description = "Attinav repository tooling: workflow linters and the flake formatter";

  # The firmware and the client each have their own dev shell (software/*/flake.nix). This
  # flake only carries what applies to the repository as a whole.

  inputs = {
    # Pinned to a revision whose zizmor builds (its aws-lc-sys dependency is broken on
    # later nixos-unstable at the time of writing); bump with `nix flake update`.
    nixpkgs.url = "github:NixOS/nixpkgs/f9a75f41be3a74eab55d3a7679d34ad6cc0da977";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        devShells.default = pkgs.mkShell {
          name = "attinav-tools";
          packages = with pkgs; [
            # Syntax and semantics of the GitHub workflows.
            actionlint
            # Security audit of the GitHub workflows (pinning, permissions, injection).
            zizmor
            nixpkgs-fmt
          ];
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}
