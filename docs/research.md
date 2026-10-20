# GL.iNet extension research

Research date: 2026-10-20.
The research used official documentation, pinned upstream code, and an official firmware image.
It did not connect to a physical router.
The [source inventory](../research/sources.json) records revisions and licenses.

## Target

Beryl 7 is GL-MT3600BE.
It differs from Beryl AX and Slate 7.
GL.iNet lists a quad-core 2 GHz CPU, 512 MB RAM, and 512 MB NAND.
Vendor throughput figures do not predict Wayline performance.
[Official product](https://www.gl-inet.com/products/gl-mt3600be)

The inspected 4.9.0 release1 image uses OpenWrt 21.02-SNAPSHOT, Linux 5.4.281, musl 1.2.3, opkg, and iptables-generation firewall packages.
Its package architecture is `aarch64_cortex-a53`.
The official baseline also identifies Cortex-A53 and musl.
[Baseline configuration](https://github.com/gl-inet/gl-image/blob/617e1f1d8f3cce672a63a3c59bb15003094652f3/versions/4.9.0/configs/gl-mt3600be-baseline.config)

These properties apply to this image.
A model or GL version alone cannot establish another device's kernel, firewall, or package format.
The [firmware report](firmware-evidence.md) records hashes and exact paths.

## Native extensions

The factory plug-in manager installs OpenWrt packages.
A package needs separate menu and view files to supply a native page.
[Official plug-in guide](https://docs.gl-inet.com/router/en/4/interface_guide/plugins/)

| Surface | Observed contract |
|---|---|
| Menu | JSON under `/usr/share/oui/menu.d`. |
| View | `/www/views/gl-sdk4-ui-<view>.common.js.gz`. |
| Frontend | Existing Vue runtime and `$rpcRequest`. |
| RPC | `/usr/lib/oui-httpd/rpc/<object>`. |
| Validation | `/usr/share/gl-validator.d/<object>.lua`. |
| Lifecycle | OpenWrt procd and required package hooks. |

The community SDK4 toolkit documents these interfaces.
Its pinned firmware catalog does not include Beryl 7.
The inspected image independently confirms the loader and file layout.
Native behavior still needs hardware checks.
[SDK4 toolkit](https://github.com/go-wombat/gl-sdk4-plugin-kit), [authentication guide](https://github.com/go-wombat/gl-sdk4-plugin-kit/blob/570ac6596c12a507b41969002acf455e2206c271/docs/backend-auth.md)

The official clean-image build excludes proprietary GL applications and the GL UI.
A replacement firmware build would not reproduce this native product interface.
Wayline therefore ships an add-on IPK.
[Official build guide](https://github.com/gl-inet/gl-image/blob/617e1f1d8f3cce672a63a3c59bb15003094652f3/versions/4.9.0/GL-MT3600BE_FIRMWARE_GUIDE.md)

## Tunnel priority

Tunnel order controls which matching traffic rule applies first.
It does not necessarily change existing connections, guest networks, split rules, or every router process.

Profile order within one tunnel controls which profile connects.
Lower-priority profiles can remain inactive.
Their public endpoint latency does not establish through-tunnel performance.

GL 4.8 documents multiple tunnel rules and failover behavior.
GL 4.9 adds ordered profiles within independent groups.
Wayline promotes an already active group only when all relevant traffic rules match.
[4.8 dashboard](https://docs.gl-inet.com/router/en/4/interface_guide/vpn_dashboard_v4.8/), [4.9 dashboard](https://docs.gl-inet.com/router/en/4/interface_guide/vpn_dashboard/)

The inspected dashboard calls `vpn-client.get_tunnel`.
It handles `tunnels` separately from `default_tunnels`.
For ordering, it sends `{id_list:[...]}` with each tunnel's section `id`.
Editing uses numeric `tunnel_id`.
These identifiers are not interchangeable.

Vendor helpers call `gl-session.call` locally with module, function, and parameters.
Their result appears under `result`.
Wayline reuses this fixed native API.
It does not expose a general RPC proxy.

Vendor setup helpers create per-instance marks, tables, and blackhole routes.
Wayline discovers the existing mapping.
It does not assign new table numbers.
[Native method reference](https://github.com/go-wombat/gl-sdk4-plugin-kit/blob/570ac6596c12a507b41969002acf455e2206c271/docs/write-methods-vpn.md)

## Useful measurements

| Measurement | Meaning | Limit |
|---|---|---|
| Public endpoint through WAN | Route to the server's public address. | Does not prove encrypted forwarding. |
| Private responder through VPN | Route through the tunnel to a known responder. | Requires an available responder. |
| Shared Internet target through VPN | Comparable route through each exit. | Depends on that destination and ICMP policy. |

The MVP uses a common Internet target through each active VPN.
It also offers optional destination diagnostics.
It does not label a public endpoint check as through-tunnel latency.

WireGuard does not answer arbitrary unauthenticated UDP probes.
UDP connect success and handshake age are not RTT measurements.
An idle tunnel's old handshake does not prove failure.
[WireGuard paper](https://www.wireguard.com/papers/wireguard.pdf), [quick start](https://www.wireguard.com/quickstart/)

Binding only a source address does not prove egress.
The monitor checks route resolution with ICMP, the existing mark, and the source.
It then binds the numeric interface index.
Unknown paths fail closed.
[Linux socket API](https://man7.org/linux/man-pages/man7/socket.7.html)

## Upstream references

| Project | Reused knowledge | Boundary |
|---|---|---|
| [SDK4 toolkit](https://github.com/go-wombat/gl-sdk4-plugin-kit) | Menu, view, RPC, and firmware contracts. | MIT. No runtime dependency. |
| [Tailscale extension](https://github.com/RemoteToHome-io/gl-tailscale-fix) | Lifecycle and authenticated integration. | GPL-3.0 reference only. No copied injection code. |
| [Tailscale updater](https://github.com/admonstrator/glinet-tailscale-updater) | Release assets and router footprint. | README review only. |
| [Reality VPN extension](https://github.com/mxt96/reality-vpn-glinet-universal) | Compact native page and validators. | MIT reference. Its data plane remains separate. |
| [Home Assistant GL integration](https://github.com/metril/ha-glinet) | VPN status and control requests. | MIT reference. Different selection model. |
| [GL profiler](https://github.com/glinet4/glinet4-profiler) | Read-only API discovery. | GPL-3.0 reference only. |
| [GL registry](https://github.com/glinet4/glinet4-registry) | Response schemas and evidence labels. | GPL-3.0 reference. No Beryl profile at the pinned revision. |
| [mwan3](https://github.com/openwrt/packages/tree/master/net/mwan3) | Multiple targets and recovery thresholds. | Do not install another routing manager for this feature. |
| [GL image sources](https://github.com/gl-inet/gl-image) | Platform and package configuration. | Clean images omit proprietary applications. |

The runtime study also inspected OpenWrt libubox, netifd, dnsmasq, fping, and Monoio.
The [runtime decision](constrained-runtime.md) explains readiness waits and bounded concurrency.
Wayline reuses existing Rust packet, socket, process, archive, and signing tools.
It avoids a custom reactor, database codec, signature scheme, or routing manager.

## Tailscale

Tailscale needs a distinct adapter.
Its exit recommendations and automatic selection already cover related use cases.
Support depends on the installed client and account plan.
[Exit recommendations](https://tailscale.com/docs/features/exit-nodes/auto-exit-nodes)

Tailscale ping can measure peer connectivity.
It does not prove Internet forwarding through every candidate exit.
Binding `tailscale0` cannot independently select several exits within one daemon.
[Ping types](https://tailscale.com/kb/1465/ping-types)

GL warns that Tailscale and other VPN clients can conflict.
The MVP therefore excludes Tailscale exit control.
[GL Tailscale guide](https://docs.gl-inet.com/router/en/4/interface_guide/tailscale/)

## Remaining evidence

Static inspection establishes file layout, identifiers, and request construction.
The isolated WireGuard lab checks measurement routing and failure handling.
Neither proves native behavior on a physical Beryl 7.

Hardware checks must cover authentication, priority persistence, client egress, DNS, existing connections, IPv6 policy, and updates.
The [validation report](validation.md) separates measured results from these limits.
