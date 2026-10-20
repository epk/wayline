# Monitor interface

The router control helper creates the monitor configuration.
Users configure the product through the UI.

For internal development:

```sh
cargo xtask build
```

The resulting binary targets `aarch64-unknown-linux-musl`.
It runs only on Linux.

```sh
waylined monitor --config /private/monitor.json --socket /private/status.sock
waylined status --socket /private/status.sock
```

The parent directory must belong to the process user and have mode 0700.
The status socket has mode 0600.
A file lock prevents a second instance from replacing an active endpoint.
The monitor can recover its stale socket after a crash.
It refuses to remove an ordinary file at that path.

The [example](../examples/monitor.json) describes the internal input format.
Configuration has a 32 KiB limit.
The monitor validates identities, addresses, interfaces, intervals, timeouts, and duplicate path-target combinations.

Every probe uses the configured interface index, source, and existing policy mark.
Route lookup includes ICMP and does not constrain the returned output interface.
The monitor rejects another exit, an unavailable route, or a down device.
It never retries through an unbound socket.

Each echo payload contains a random process nonce and a monotonically increasing counter.
Reply checks cover source, protocol, checksum, type, code, identity, sequence, and payload.
Each attempt processes at most 16 received packets.
A two-second timeout bounds an unsuccessful product probe.

Status schema 2 includes a run identity and timestamped samples.
Latency uses monotonic time.
Wall time locates graph samples.
The recent median covers successful samples in the last five attempts.
The loss window contains at most 64 attempts.
A timeout contributes no latency value.
