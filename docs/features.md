# MVP scope

Wayline provides a native GL.iNet page with all product settings.

| Feature | Implementation |
|---|---|
| Tunnel comparison | IPv4 ICMP through discovered devices, sources, and existing marks. |
| Manual selection | Guarded native priority request with complete order readback. |
| Tunnel controls | Native Start and Stop requests with state readback and unchanged-request checks. |
| Diagnostics | Up to four editable destinations through tunnels and optional WAN. |
| Graphs | SVG line and range band, with gaps for missing data. |
| History | Minute aggregates, 24-hour retention, five-minute write batches. |
| Updates | Signed stable releases, manual requests, optional daily installation. |
| Packaging | ARM64 IPK with native menu, RPC bridge, and startup hooks. |

The common comparison target defaults to `1.1.1.1`.
Diagnostic targets default to `1.1.1.1`, `8.8.8.8`, and `9.9.9.9`.
These addresses do not change DNS configuration.
A destination can refuse ICMP even when a tunnel works.

The UI provides 15, 30, and 60-second intervals.
A shared scheduler handles at most 45 path and destination combinations.
Duplicate combinations share one series.
Eight attempts can run concurrently.

WAN diagnostics require separate opt-in.
The monitor binds the discovered WAN device and source.
It checks the existing route before each attempt.
A VPN-routed WAN attempt appears unavailable.
Wayline does not create bypass rules.

Manual selection requires one group of equivalent native traffic rules.
All enabled group members must be connected.
Stopped members remain in the order and retain their monitoring settings.
Start requires a profile that the native dashboard assigns.
An enabled tunnel can remain offline.
The selected exit must have fresh successful measurements and no loss in its recent window.
The first MVP does not select exits automatically.

IPv6, Tailscale exit control, hostname destinations, notifications, and additional firmware contracts remain future work.
Physical Beryl 7 validation remains a release limitation.
