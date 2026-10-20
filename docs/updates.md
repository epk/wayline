# Releases and updates

The repository publishes ARM64 IPKs through GitHub Actions.
Both CI and releases use the locked devenv environment.
Actions use pinned commit identifiers and ARM64 runners.

The release build checks formatting and lint rules, builds the binaries and UI, runs the isolated WireGuard lab, and creates the package.
The publish job compiles its Rust tool before it receives the signing secret.
Only the signing step receives `RELEASE_SIGNING_KEY`.
Only the publish job receives repository write permission.

The signing tool creates a bounded JSON manifest.
The manifest binds the repository, version, package, firmware contract, architecture, size, and SHA-256.
OpenWrt usign signs the exact manifest bytes.
The tool verifies its signature against the committed public key before publication.

The trusted public fingerprint is `e75bc29163d35266`.
Keep the private key outside this repository.
Store the release secret in GitHub Actions.
Do not rotate the public key without an explicit trust migration.

Release procedure:

1. Change the root package version.
2. Update the release notes.
3. Run `cargo xtask check`.
4. Run `cargo xtask build`.
5. Run `cargo xtask lab`.
6. Push the reviewed commit.
7. Create and push a matching `v<version>` tag.
8. Check the Release workflow result.

The Rust tool reads the version from `GITHUB_REF_NAME` in Actions.
A local package command can receive an explicit version.
Prerelease tags create GitHub prereleases.
The updater accepts stable releases only.

The UI queues update requests for the next scheduler tick.
Automatic updates require explicit opt-in.
The automatic check runs at most once each day.
The updater downloads only through HTTPS and applies bounded time and size limits.
It verifies the manifest with the installed key before parsing its contents.
It checks opkg architecture, package control fields, size, and digest before a dry run and installation.

Settings remain a package conffile.
The updater reports installation state before it invokes opkg.
A failed or uncertain installation stops automatic retries during that boot.
It does not change routing to reach GitHub.
It does not provide transactional power-loss recovery or automatic firmware rollback.
