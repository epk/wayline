let
  source = builtins.fetchTarball {
    url = "https://github.com/NixOS/nixpkgs/archive/e513db7b0a65def6f6e17bb55195dfe510871944.tar.gz";
    sha256 = "sha256-RkAXQ3+a5rHMiP97Er7AawR1kvEXX9tK1mhiWedOPTc=";
  };
  pkgs = import source { };
in
{
  inherit (pkgs) devenv;
}
