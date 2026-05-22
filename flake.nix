{
  description = "MoQT development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      rust-overlay,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        rustToolchain = pkgs.rust-bin.stable."1.92.0".minimal.override {
          extensions = [
            "clippy"
            "rustfmt"
            "rust-src"
          ];
          targets = [ "wasm32-unknown-unknown" ];
        };

        darwinPackages = pkgs.lib.optionals pkgs.stdenv.isDarwin (
          with pkgs;
          [ libiconv ]
        );

        linuxPackages = pkgs.lib.optionals pkgs.stdenv.isLinux (
          with pkgs;
          [
            libGL
            libxkbcommon
            wayland
            xorg.libX11
            xorg.libXcursor
            xorg.libXi
            xorg.libXrandr
          ]
        );
      in
      {
        devShells.default = pkgs.mkShell {
          packages =
            with pkgs;
            [
              rustToolchain

              clang
              ffmpeg
              gnumake
              libclang
              nodejs_24
              pkg-config
              wasm-pack
            ]
            ++ darwinPackages
            ++ linuxPackages;

          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";

          shellHook = ''
            export RUSTFLAGS="--cfg tokio_unstable --cfg web_sys_unstable_apis --remap-path-prefix=$(pwd)=."
          '';
        };
      }
    );
}
