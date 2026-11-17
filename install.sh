#!/bin/sh
set -eu

REPO=epk/wayline
ARCH=aarch64_cortex-a53
RELEASE_KEY=e75bc29163d35266

fail() {
    printf 'wayline install: %s\n' "$1" >&2
    exit 1
}

if [ -f /etc/apk/arch ]; then
    manager=apk
    format=apk
    manifest='manifest-apk.json'
    grep -qx "$ARCH" /etc/apk/arch || fail "this router does not support $ARCH packages"
else
    manager=opkg
    format=ipk
    manifest=manifest.json
fi

for tool in curl jsonfilter usign sha256sum "$manager" mktemp; do
    command -v "$tool" >/dev/null 2>&1 || fail "missing required tool: $tool"
done

work=$(mktemp -d) || fail "cannot create a temporary directory"
trap 'rm -rf "$work"' EXIT
cd "$work" || fail "cannot enter $work"

version=${WAYLINE_VERSION:-}
if [ -z "$version" ]; then
    printf 'Finding the newest release...\n'
    releases=$(curl -fsSL "https://api.github.com/repos/$REPO/releases?per_page=1") ||
        fail "cannot read the release list from GitHub"
    version=$(printf '%s' "$releases" | jsonfilter -e '@[0].tag_name') ||
        fail "cannot read the release version"
fi
version=${version#v}
case "$version" in
'' | *[!A-Za-z0-9.+-]*) fail "no valid release version was found" ;;
esac

base="https://github.com/$REPO/releases/download/v$version"
package="wayline_${version}_${ARCH}.${format}"
printf 'Downloading Wayline %s...\n' "$version"
curl -fsSL --fail-early --remote-name-all \
    "$base/release.pub" "$base/$manifest" "$base/$manifest.sig" "$base/$package" ||
    fail "cannot download Wayline $version from $base"

printf 'Checking the signing key...\n'
[ "$(usign -F -p release.pub)" = "$RELEASE_KEY" ] ||
    fail "the signing key does not match $RELEASE_KEY"

printf 'Checking the manifest signature...\n'
usign -V -p release.pub -m "$manifest" -x "$manifest.sig" >/dev/null ||
    fail "the manifest signature is invalid"

printf 'Checking the package hash...\n'
digest=$(sha256sum "$package") || fail "cannot hash $package"
digest=${digest%% *}
[ -n "$digest" ] || fail "cannot hash $package"
grep -F -q "$digest" "$manifest" ||
    fail "the package hash is not listed in the signed manifest"

printf 'Installing Wayline %s...\n' "$version"
if [ "$manager" = apk ]; then
    apk add --allow-untrusted "./$package" </dev/null || fail "apk add failed"
else
    printf 'Updating package lists...\n'
    opkg update </dev/null >/dev/null || fail "opkg update failed"
    opkg install "./$package" </dev/null || fail "opkg install failed"
fi

printf 'Wayline %s installed. Open VPN → Wayline in the admin UI.\n' "$version"
