# Runtime choice

This workload sends a small number of control probes.
It does not forward user packets.
One thread and bounded state fit this workload.

OpenWrt's libubox provides `uloop`, which uses epoll on Linux.
Netifd uses this event loop.
Dnsmasq and fping also demonstrate that small C daemons can use direct readiness waits and explicit deadlines.
The [source inventory](../research/sources.json) pins these references.
The [research report](research.md) records their roles.

Sleeping until a fixed deadline can delay new status requests.
Repeated short sleeps add wakeups.
A readiness wait with the nearest timer handles both events without periodic polling.

Select has descriptor-set limits and requires set reconstruction.
Poll scans a short descriptor list and can suit this scale.
Epoll tracks registered descriptors in the kernel.
Neither epoll nor io_uring makes low-rate probes inherently faster.

The selected Rust runtime is Monoio 0.2.4 with only its legacy feature.
This backend supports the Linux 5.4 target.
Linux 5.4 contains early io_uring support, but newer runtime requirements and vendor differences make it unsuitable as the default here.
Wayline does not enable io_uring.

The implementation uses local futures, bounded concurrency, and no worker pool.
It reuses Monoio timers and readiness handling.
It does not implement a custom reactor.
The [lab](../lab/README.md) checks scheduling under full timeouts.

The scheduler permits eight attempts.
Forty-five series with two-second probe timeouts cannot sustain a 15-second interval with two attempts.
Eight attempts allow the full workload within that interval.
The route deadline adds at most 200 milliseconds per attempt.

Serde parses small bounded messages outside forwarded traffic.
A custom serializer would add maintenance cost without evidence of a useful saving.
Wayline needs no DashMap, shard-per-core layout, pinned thread pool, or io_uring queue.
