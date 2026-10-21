# Architecture

Wayline separates the resident monitor from infrequent control operations.

| Component | Responsibility |
|---|---|
| `waylined` | Route checks, ICMP, scheduling, bounded recent samples, read-only status socket. |
| `wayline-ctl` | Native discovery, UI requests, settings, service reconciliation, history, updates. |
| Lua RPC bridge | Require an administrator session and invoke seven fixed Rust operations. |
| Vue component | Show state, graphs, settings, and explicit exit controls. |
| Rust xtask | Build, check, package, preview, sign, publish, and run the isolated lab. |

The monitor uses one Monoio runtime with its legacy epoll driver.
Default Monoio features remain disabled.
It has no worker pool or io_uring dependency.
Beryl 7 firmware uses Linux 5.4.281.

The monitor owns all mutable measurement state.
It uses no shared map, thread locks, or scheduler pool.
Serde handles bounded configuration and status messages.
Serialization does not process forwarded traffic.

Socket2 provides safe socket operations.
Rustix provides process identity.
Existing netlink and pnet crates provide packet handling.
Monoio registers safe duplicate descriptors for readiness.
The monitor retains Socket2 descriptors for raw ICMP and netlink payload operations.
This avoids custom unsafe code.

Each attempt checks the route using its source, target, mark, and ICMP protocol.
The kernel must return the intended interface.
Only then can the monitor send through that interface.
GL.iNet retains ownership of routes, tunnel lifecycle, DNS, marks, and kill switches.

The monitor permits eight attempts and four status clients.
It retains 64 samples per series, with at most 45 series.
Each response has a 384 KiB limit.
A status client has a two-second write deadline.
The service descriptor limit is 128.

The control helper runs for UI requests, interface events, boot, and scheduler ticks.
It reuses native ubus, UCI, procd, SQLite, curl, usign, and opkg.
No separate HTTP server runs on the router.
The Rust settings parser accepts Lua's empty-table encoding for an empty tunnel list.
It rejects nonempty objects in that field.

Tunnel controls call native `vpn-client.set_tunnel` with the numeric tunnel identifier and a boolean `enabled` value.
Discovery checks that the native identifier, UCI section, protocol, and enabled state agree.
GL.iNet can replace a UCI section when it edits a tunnel.
Monitoring choices use the stable numeric tunnel identifier.
The helper converts known legacy section choices before it requests a tunnel change.
Start and Stop share the exit-selection lock.
They check the configuration revision before mutation and read the native result afterward.
Repeated requests for the current state avoid the native mutation.
A failed response reports an unknown outcome and causes no automatic retry.
An enabled tunnel can remain offline while GL.iNet establishes its connection.
Only connected paths contribute measurements.
The Lua bridge uses GL.iNet's asynchronous process interface with argument arrays.
Native HTTP workers can process nested RPC requests while Rust runs.
The bridge does not construct shell commands.

Cortex-A53 release settings use full LTO, one codegen unit, stripped symbols, and size optimization.
Overflow checks remain active.
The lab reports actual resource use.
These measurements do not predict exact physical router performance.

Clippy all, pedantic, and nursery groups deny warnings.
Three narrow exceptions cover local Monoio futures, independent UI switches, and the prohibition on documentation comments.
No broad unit test suite runs.
