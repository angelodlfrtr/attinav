{
  description = "Attinav desktop client (Tauri v2 + React) development environment";

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
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

        # Native libraries the Tauri (wry / WebKitGTK) backend and the serial port crate link to.
        nativeLibs = with pkgs; [
          webkitgtk_4_1
          gtk3
          libsoup_3
          glib
          cairo
          pango
          gdk-pixbuf
          librsvg
          openssl
          udev
          dbus
          glib-networking
        ];
      in
      {
        devShells.default = pkgs.mkShell {
          name = "attinav-client";

          nativeBuildInputs = with pkgs; [
            rustToolchain
            cargo-tauri
            nodejs_22
            pkg-config
            wrapGAppsHook3
            nixpkgs-fmt
          ];
          buildInputs = nativeLibs;

          env = {
            # WebKitGTK needs its GIO modules (TLS) and GSettings schemas at run time.
            GIO_EXTRA_MODULES = "${pkgs.glib-networking}/lib/gio/modules";
            XDG_DATA_DIRS = "${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}";
            # Software rendering fallback keeps the webview usable inside containers / VMs.
            WEBKIT_DISABLE_DMABUF_RENDERER = "1";
          };

          shellHook = ''
            echo "attinav client shell: $(rustc --version), node $(node --version)"
            echo "  npm install                     fetch the frontend dependencies (once)"
            echo "  cargo tauri dev                 run the app with hot reload"
            echo "  cargo tauri build               produce a release bundle"
            echo "  npm run check                   tsc + eslint"
            echo "  (cd src-tauri && cargo lint)    clippy (pedantic) on the Rust side"
          '';
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}
