# MVP validation

Validation date: 2026-10-20.
The release remains experimental until physical router checks pass.

## Measurement core

The closed-box lab ran the optimized ARM64 musl monitor under Linux 6.8.0-117-generic.
It used real WireGuard interfaces in isolated network namespaces.
The topology had eight exits and no Internet interface.

The following checks passed:

- All eight VPN paths returned successful probes.
- Added delay changed the measured latency.
- Total loss removed the recent median.
- Blackhole routes prevented probes.
- A route through another exit failed the interface check.
- ICMP-specific policy affected route resolution.
- A down interface stopped its measurements.
- A crash left a stale socket that the next instance recovered.
- Invalid inputs failed before monitoring began.
- Duplicate instances could not replace the active socket.
- An ordinary file at the socket path remained unchanged.
- Guard requests left routing state unchanged.
- All 45 series continued during a sustained full outage.

The timeout workload used a 15-second interval and two-second probe timeout.
Resource measurements covered approximately 80 seconds after warmup.

| Measure | Observed value |
|---|---|
| RSS, from smaps_rollup | 528 KiB |
| Threads | 1 |
| File descriptors | 99 |
| CPU, as a share of one core | 0.35% |
| Monitor binary | 458,408 bytes |

These values describe the VM workload.
They exclude the UI, control helper, SQLite process, and kernel socket memory.
They do not predict exact Cortex-A53 performance.
The [record](../artifacts/validation/monitor-arm64.json) binds these results to the binary digest.
Raw local snapshots remain under the ignored artifacts directory.
Actions retains reports for its own builds.

## Quality checks

The project passes Rust formatting and Clippy all, pedantic, and nursery groups with warnings denied.
It also passes frontend ESLint and Prettier, Lua checks, required hook checks, Nix formatting, and Actions validation.
The browser preview passes the overview, settings, and diagnostic graph interactions without console errors.
Screenshots contain sample data.

## Native package sandbox

The optional firmware sandbox runs the extracted image's opkg, ubusd, rpcd, and UCI services.
It uses an isolated Linux temporary filesystem and a minimal first-boot network configuration.
Vendor files remain outside Git.

Native package installation and removal passed.
Settings reads and writes, stale-write rejection, unchanged-write avoidance, and scheduler idempotence also passed.
Lua's empty-list input passed the Rust settings boundary.
An offline update request reported failure through the UI state.
The [native record](../artifacts/validation/native-package.json) lists these results.
The sandbox does not run the native browser authentication flow or change a real tunnel's priority.

To repeat this check with a verified extracted image:

```sh
cargo xtask package
cargo xtask lab-native /path/to/extracted-firmware-root
```

The command copies the image into the ignored local artifacts directory.
Subsequent lab runs reuse that private copy.

## Limits

Physical Beryl 7 checks remain necessary for installation, native page loading, session handling, priority changes, client egress, and updates.
The firmware contract uses static fingerprints and guarded request schemas.
It does not certify native behavior.
The lab kernel differs from the router's Linux 5.4.281 kernel.

The first release has no automatic exit selection, IPv6 probes, or Tailscale adapter.
History can lose the latest uncollected batch after a crash.
Package installation cannot guarantee recovery after power loss.
The [acceptance procedure](implementation-plan.md) lists the remaining hardware checks.
