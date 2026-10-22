{
  stdenv,
  fetchFromGitHub,
  cmake,
}:
stdenv.mkDerivation {
  pname = "usign";
  version = "2025-10-03";
  src = fetchFromGitHub {
    owner = "openwrt";
    repo = "usign";
    rev = "c4c72b1b07945ee192361dc751291a7c98d6adcd";
    hash = "sha256-MXvv1vFwGyRZ+gxNVyUvW+gsinaCPiZSqOukCKxyV4A=";
  };
  nativeBuildInputs = [ cmake ];
}
