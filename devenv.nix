{
  pkgs,
  inputs,
  lib,
  ...
}:
let
  rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
  usign = pkgs.callPackage ./nix/usign.nix { };
  musl = pkgs.pkgsCross.aarch64-multiplatform-musl.stdenv.cc;
  linux = import inputs.nixpkgs { system = "aarch64-linux"; };
  linuxTools = with linux; [
    iproute2
    wireguard-tools
    tcpdump
    util-linux
    coreutils
    glibc.bin
    gnutar
    gzip
    apk-tools
  ];
  rustTools = [
    rust
    pkgs.stdenv.cc
    pkgs.llvmPackages.lld
    pkgs.git
    pkgs.coreutils
  ];
  packageTools = [
    pkgs.apk-tools
    pkgs.fakeroot
  ];
  frontendTools = [
    pkgs.nodejs_24
    pkgs.pnpm
  ];
  lintTools = with pkgs; [
    lua5_1
    lua51Packages.luacheck
    stylua
    shellcheck
    shfmt
    nixfmt
    actionlint
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
      curl
      usign
      binutils
      gnutar
      gzip
      coreutils
    ]
    ++ packageTools
    ++ lib.optionals pkgs.stdenv.isDarwin [ pkgs.colima ];
  env.CC_aarch64_unknown_linux_musl = "${musl}/bin/${musl.targetPrefix}cc";
  env.AR_aarch64_unknown_linux_musl = "${musl.bintools}/bin/${musl.targetPrefix}ar";
  env.CFLAGS_aarch64_unknown_linux_musl = "-Os -mcpu=cortex-a53 -mno-outline-atomics";
  env.WAYLINE_LAB_PATH = lib.makeBinPath linuxTools;
  profiles = {
    ci-lint.module = {
      packages = lib.mkForce (rustTools ++ frontendTools ++ lintTools);
      env.WAYLINE_LAB_PATH = lib.mkForce "";
    };
    ci-build.module = {
      packages = lib.mkForce (rustTools ++ frontendTools ++ packageTools);
      env.WAYLINE_LAB_PATH = lib.mkForce "";
    };
    ci-test.module.packages = lib.mkForce rustTools;
    ci-release.module = {
      packages = lib.mkForce (rustTools ++ [ usign ]);
      env.WAYLINE_LAB_PATH = lib.mkForce "";
    };
  };
  # Builds run offline. Say so on entry when a lockfile needs packages that
  # are not downloaded yet, rather than letting cargo fail with "no matching package".
  enterShell = ''
    if ! cargo metadata --manifest-path "$DEVENV_ROOT/Cargo.toml" --locked --offline --format-version 1 >/dev/null 2>&1; then
      echo "wayline: Cargo.lock needs crates that are not downloaded yet. Run wayline-deps." >&2
    fi
    if [ ! -e "$DEVENV_ROOT/ui/node_modules/.modules.yaml" ] || [ "$DEVENV_ROOT/ui/pnpm-lock.yaml" -nt "$DEVENV_ROOT/ui/node_modules/.modules.yaml" ]; then
      echo "wayline: ui/pnpm-lock.yaml changed since the last install. Run wayline-deps." >&2
    fi
  '';
  scripts.wayline-deps.exec = "exec cargo run --locked --package xtask -- deps";
  scripts.wayline-check.exec = "exec cargo run --locked --offline --package xtask -- check";
  scripts.wayline-lint.exec = "exec cargo run --locked --offline --package xtask -- lint";
  scripts.wayline-test.exec = "exec cargo run --locked --offline --package xtask -- test";
  scripts.wayline-build.exec = "exec cargo run --locked --offline --package xtask -- build";
  scripts.wayline-package.exec = "exec cargo run --locked --offline --package xtask -- package";
  scripts.wayline-preview.exec = "exec cargo run --locked --offline --package xtask -- preview";
  scripts.wayline-lab.exec = "exec cargo run --locked --offline --package xtask -- lab";
}
