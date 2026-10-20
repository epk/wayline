{
  pkgs,
  inputs,
  lib,
  ...
}:
let
  rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
  usign = pkgs.callPackage ./nix/usign.nix { };
  linux = import inputs.nixpkgs { system = "aarch64-linux"; };
  linuxTools = with linux; [
    iproute2
    wireguard-tools
    tcpdump
    util-linux
    coreutils
    glibc.bin
  ];
in
{
  overlays = [ inputs.rust-overlay.overlays.default ];
  packages =
    with pkgs;
    [
      rust
      llvmPackages.lld
      nodejs_24
      pnpm
      lua5_1
      lua51Packages.luacheck
      stylua
      shellcheck
      shfmt
      nixfmt
      actionlint
      git
      gh
      curl
      usign
      binutils
      gnutar
      gzip
      coreutils
    ]
    ++ lib.optionals pkgs.stdenv.isDarwin [ pkgs.colima ];
  env.WAYLINE_LAB_PATH = lib.makeBinPath linuxTools;
  scripts.wayline-deps.exec = "exec cargo run --locked --package xtask -- deps";
  scripts.wayline-check.exec = "exec cargo run --locked --offline --package xtask -- check";
  scripts.wayline-build.exec = "exec cargo run --locked --offline --package xtask -- build";
  scripts.wayline-package.exec = "exec cargo run --locked --offline --package xtask -- package";
  scripts.wayline-preview.exec = "exec cargo run --locked --offline --package xtask -- preview";
  scripts.wayline-lab.exec = "exec cargo run --locked --offline --package xtask -- lab";
}
