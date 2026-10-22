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
  ];
  rustTools = [
    rust
    pkgs.stdenv.cc
    pkgs.llvmPackages.lld
    pkgs.git
    pkgs.coreutils
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
    ++ lib.optionals pkgs.stdenv.isDarwin [ pkgs.colima ];
  env.CC_aarch64_unknown_linux_musl = "${musl}/bin/${musl.targetPrefix}cc";
  env.AR_aarch64_unknown_linux_musl = "${musl.bintools}/bin/${musl.targetPrefix}ar";
  env.CFLAGS_aarch64_unknown_linux_musl = "-Os -mcpu=cortex-a53 -mno-outline-atomics";
  env.LIBSQLITE3_FLAGS = lib.concatStringsSep " " [
    "-USQLITE_ENABLE_COLUMN_METADATA"
    "-USQLITE_ENABLE_DBSTAT_VTAB"
    "-USQLITE_ENABLE_FTS3"
    "-USQLITE_ENABLE_FTS3_PARENTHESIS"
    "-USQLITE_ENABLE_FTS5"
    "-USQLITE_ENABLE_JSON1"
    "-USQLITE_ENABLE_LOAD_EXTENSION"
    "-USQLITE_ENABLE_MEMORY_MANAGEMENT"
    "-USQLITE_ENABLE_RTREE"
    "-USQLITE_ENABLE_STAT4"
    "-USQLITE_SOUNDEX"
    "-DSQLITE_OMIT_LOAD_EXTENSION"
    "-DSQLITE_OMIT_DEPRECATED"
    "-DSQLITE_OMIT_SHARED_CACHE"
    "-DSQLITE_OMIT_PROGRESS_CALLBACK"
    "-DSQLITE_OMIT_JSON"
    "-DSQLITE_OMIT_WAL"
    "-DSQLITE_OMIT_FOREIGN_KEY"
    "-DSQLITE_OMIT_AUTHORIZATION"
    "-DSQLITE_OMIT_EXPLAIN"
    "-DSQLITE_OMIT_TRACE"
    "-DSQLITE_OMIT_COMPLETE"
    "-DSQLITE_OMIT_DATETIME_FUNCS"
    "-DSQLITE_OMIT_LOCALTIME"
    "-DSQLITE_OMIT_UTF16"
    "-DSQLITE_OMIT_BUILTIN_TEST"
    "-DSQLITE_OMIT_INCRBLOB"
    "-DSQLITE_OMIT_TCL_VARIABLE"
    "-DSQLITE_OMIT_GET_TABLE"
    "-DSQLITE_OMIT_AUTOVACUUM"
    "-DSQLITE_OMIT_DECLTYPE"
    "-DSQLITE_OMIT_BTREECOUNT"
    "-DSQLITE_DEFAULT_MEMSTATUS=0"
  ];
  env.WAYLINE_LAB_PATH = lib.makeBinPath linuxTools;
  profiles = {
    ci-lint.module = {
      packages = lib.mkForce (rustTools ++ frontendTools ++ lintTools);
      env.WAYLINE_LAB_PATH = lib.mkForce "";
    };
    ci-build.module = {
      packages = lib.mkForce (rustTools ++ frontendTools);
      env.WAYLINE_LAB_PATH = lib.mkForce "";
    };
    ci-test.module.packages = lib.mkForce rustTools;
    ci-release.module = {
      packages = lib.mkForce (rustTools ++ [ usign ]);
      env.WAYLINE_LAB_PATH = lib.mkForce "";
    };
  };
  scripts.wayline-deps.exec = "exec cargo run --locked --package xtask -- deps";
  scripts.wayline-check.exec = "exec cargo run --locked --offline --package xtask -- check";
  scripts.wayline-lint.exec = "exec cargo run --locked --offline --package xtask -- lint";
  scripts.wayline-test.exec = "exec cargo run --locked --offline --package xtask -- test";
  scripts.wayline-build.exec = "exec cargo run --locked --offline --package xtask -- build";
  scripts.wayline-package.exec = "exec cargo run --locked --offline --package xtask -- package";
  scripts.wayline-preview.exec = "exec cargo run --locked --offline --package xtask -- preview";
  scripts.wayline-lab.exec = "exec cargo run --locked --offline --package xtask -- lab";
}
