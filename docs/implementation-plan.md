# MVP completion and next checks

The MVP implements the Rust monitor, control helper, native page, package, signed updates, and locked development workflow.
The repository records the remaining validation limits explicitly.

Before stable release, use a physical Beryl 7 with firmware 4.9.0 release1:

1. Install the signed package.
2. Check the native menu and page loader.
3. Check requests with active, expired, and missing administrator sessions.
4. Connect two VPN tunnels with equivalent traffic rules.
5. Enable monitoring through the UI.
6. Check tunnel and direct WAN traffic with packet capture.
7. Choose another healthy exit.
8. Check native order and client egress.
9. Change factory settings during a Wayline request.
10. Check stale-state rejection and unknown outcomes.
11. Disconnect and reconnect tunnels.
12. Check history after a reboot.
13. Install a signed update.
14. Remove the package.
15. Check the factory VPN page and existing policies.

Do not enable another firmware contract through a version string alone.
Record native module fingerprints and observed request schemas first.
Repeat the routing, authentication, and selection checks on each supported contract.

Future work can add IPv6, Tailscale, and broader native rule groups.
The first release does not require those features.
