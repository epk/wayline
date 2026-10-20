# Beryl 7 firmware evidence

Inspection date: 2026-10-20.
The research downloaded the official firmware and checked its SHA-256.
It did not flash an image or contact a router.

## Provenance

| Field | Value |
|---|---|
| Model | GL-MT3600BE |
| Firmware | 4.9.0 release1 |
| Published release time | 2026-07-07 17:58:20 |
| Bytes | 80,993,891 |
| SHA-256 | `4c32509876176a04ca32ed7288567be1d31603a7473f592552380c586b66df65` |
| Root member | `sysupgrade-glinet_gl-mt3600be/root` |

[Official metadata](https://firmware-api.gl-inet.com/cloud-api/model/info?model=MT3600BE), [official image](https://fw.gl-inet.com/firmware/mt3600be/release/mt3600be-4.9.0_release1-1046-0702-1783003143.bin)

The image is a tar archive with a SquashFS root member.
Inspection used tar and squashfs-tools 4.7.5.
The [machine-readable evidence](../research/beryl7-4.9.0.json) contains all recorded file and package hashes.
This repository does not redistribute vendor firmware, scripts, or bundles.

## Integration observations

The native loader requests `/views/gl-sdk4-ui-<view>.common.js`.
Menu JSON appears under `/usr/share/oui/menu.d`.
The runtime exposes `$rpcRequest` and an existing Vue instance.

The dashboard calls `vpn-client.order_tunnel` with section `id` values.
Its edit methods use numeric `tunnel_id`.
The response separates `tunnels` from `default_tunnels`.

The 4.9 profile picker uses a nested `via` object.
Group entries contain `group_id` and ordered `id_list` values.
These observations do not establish every protocol response schema.

The RPC dispatcher checks ordinary method access.
Its session helper contains a local-request exception.
Wayline separately requires a root administrator session.
That explicit check must still pass active, expired, and missing-session hardware checks.

Vendor setup helpers generate interface configurations, marks, tables, and blackhole routes.
They recognize WireGuard and OpenVPN instances.
Wayline reads this mapping and leaves policy ownership with GL.

The platform reports OpenWrt 21.02-SNAPSHOT, mediatek/mt7987, and aarch64_cortex-a53.
Its packages identify Linux 5.4.281, musl 1.2.3, opkg, and the iptables-generation firewall.
Nftables files alone do not establish fw4 use.

## Contract fingerprints

| File | SHA-256 |
|---|---|
| Native VPN RPC | `9c8a9102c046ff12ba95e4c9a03b4e5392c461584c5d4d8ab9c5591ab6ec9237` |
| Stored app gzip | `14932d7856b8893e2185a33da80304f02de51925626cd15fff04e7f30879bf82` |
| Decompressed app | `d85b8cf6573572bbe4ba096a8c6f7043c7c2cd1df5541933c6b83192f05240c7` |
| Stored dashboard gzip | `5cb95aa16263a9a087ab2e1a2f2156b92faff1a1cf264091ce4181a603f6e81a` |
| Decompressed dashboard | `7dbed4b17c3a5174e8c50b9ce2bd42815b8ffc33341226c7311eecdd3a26ffad` |

Stored and decompressed hashes describe different byte sequences.
Compare the same representation.
A matching fingerprint does not replace native runtime checks.
