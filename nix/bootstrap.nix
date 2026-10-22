let
  source = builtins.fetchTarball {
    url = "https://github.com/NixOS/nixpkgs/archive/f324bcfb496deb48d1cb1e679b87f82037e0b72f.tar.gz";
    sha256 = "sha256-MbpS2LDxuBhUu4IdzKlFbeikesP6Nt5Dgr5yVa6c0KY=";
  };
  pkgs = import source { };
in
{
  inherit (pkgs) devenv;
}
