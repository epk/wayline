Wayline's first experimental MVP provides a native GL.iNet page for VPN latency comparison and guarded manual exit selection.

Features include destination diagnostics, optional direct WAN measurements, 24-hour graphs, and signed stable updates.

This package targets Beryl 7 (GL-MT3600BE), firmware 4.9.0 release1, and aarch64_cortex-a53.
Other firmware contracts remain disabled.
Monitoring and automatic updates start disabled.

The isolated ARM64 lab uses real WireGuard tunnels and injected failures.
Physical router installation, authentication, and native priority changes still require hardware validation.
Read the README and validation limits before installation.
