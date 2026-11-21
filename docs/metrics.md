# How Wayline measures

Every number on the page comes from one of the measurements below. Scores are
derived from these raw values with the formulas on this page, and the page
shows the inputs next to each score.

## Sampling and retention

| Source | Interval | Bucket | Kept (history setting 7 days) |
| --- | --- | --- | --- |
| ICMP probes per path and destination | 1–60 s (setting) | 15 s | 15 s for 6 h, 1 min for 3 days, 5 min for 7 days |
| Upstream Wi-Fi (`iwinfo`) | 5 s | 15 s | same tiers |
| Router and interface telemetry | 5 s | 15 s | same tiers |

With 30 days, 5-minute and 1-hour tiers keep 30 days. With 1 day, 1-minute
data keeps 1 day. In memory only, 15 s data keeps 3 hours and 1-minute data 1 day.
Coarser tiers are exact rollups of finer ones: sums, minimums and maximums
combine without loss, and latency medians are lower medians of child medians.
Live data not yet saved is rolled up through the same tiers at query time, so a
period reads the same before and after it is saved.

## Storage

History is columnar on disk, in the same Arrow format the page receives.
Each kind (`latency`, `wifi`, `system`, `interface`) has a directory per tier,
holding Arrow IPC files with ZSTD-compressed buffers, one file per time chunk:

| Tier | Chunk |
| --- | --- |
| 15 s | 1 hour |
| 1 min | 6 hours |
| 5 min | 1 day |
| 1 hour | 7 days |

Each file has the columns `series` (UTF-8), `period` (bucket start / step) and
the kind's measurement columns (Int64, nullable). Every 5 minutes wayline-ctl
merges new 15-second rows into the open chunk, recomputes the affected coarser
periods, and rewrites each changed file atomically (write, fsync, rename).
Retention deletes whole files, so it can keep up to one chunk longer than the
nominal period. A file that cannot be read is renamed to `.broken`, logged in
Recent changes, and only that chunk is lost. Storage is capped at 96 MB on
flash and 8 MB in memory; the oldest chunks go first.

A week of eleven latency series takes about 360 KB; the SQLite store it
replaced took 2.9 MB for the same data. Read the files with any Arrow tool,
for example `pyarrow.ipc.open_file("…/latency/s1m/1795291800.arrow")`.

`latency.sqlite` from earlier versions is left in place and no longer read.
Purge history deletes it.

## Latency, loss, jitter

- **Latency**: round-trip time of a 32-byte ICMP echo through the path, bound
  to the path's interface, routing mark and source address. Per bucket: min,
  lower median, max.
- **Loss**: probes without a matching reply within the timeout (2 s) divided by
  probes sent. The page shows the destination and the probe count beside it.
- **Jitter**: mean absolute difference between consecutive successful RTTs on
  the same path, `J = Σ|RTTᵢ − RTTᵢ₋₁| / n`. A lost probe breaks the chain.
  Buckets keep the sum and count, so rollups stay exact. This is the
  interarrival form of RFC 3550 without its 1/16 smoothing.
- **Incident**: a run of consecutive 15-second buckets in which every probe was
  lost. It ends with the first bucket that has a reply; a gap without probes
  also ends it. The incident log (`incidents.json`, last 200) is written at
  15-second resolution every 5 minutes, so it outlives the 15-second tier.
  Upstream Wi-Fi incidents are runs of buckets in which the repeater was never
  associated. History views derive incidents at the resolution they show.
  Uptime for a range is `1 − (fully lost buckets / buckets with probes)`.

## Voice quality estimate (MOS)

A simplified ITU-T G.107 E-model, as used by common network tools. It models
voice calls, not general browsing.

```
EL = latency_ms + 2 × jitter_ms + 10
R  = 93.2 − EL / 40            if EL < 160
R  = 93.2 − (EL − 120) / 10    otherwise
R  = R − 2.5 × loss_percent, clamped to 0..100
MOS = 1 + 0.035 R + 7e-6 × R × (R − 60) × (100 − R)
```

Bands: 4.3 and above excellent, 4.0–4.3 good, 3.6–4.0 fair, below 3.6 poor.

## Upstream Wi-Fi

Read from `ubus call iwinfo info|assoclist|survey` for the client-mode
interface, plus `/sys/class/net/<if>/statistics`.

- **Signal, noise**: dBm. Values outside −110..−1 (signal) or −120..−50 (noise)
  are driver placeholders and are dropped. **SNR** = signal − noise.
- **Rates**: last transmit and receive PHY rate, MCS, streams, width and guard
  interval. **Expected throughput** is the driver's estimate when it reports one.
- **Retries** = retried frames / (sent frames + retried frames). **Failures** =
  failed frames / sent frames. Counters reset on reassociation and are not
  carried across it.
- **Channel busy** = Δbusy time / Δactive time from the channel survey of the
  serving frequency. **Own airtime** = Δ(rx + tx time) / Δactive.
  **Others** = busy − own.
- **Drop**: connected → disconnected, a silent reassociation (connected time
  went backwards), or a change of network. **Roam**: new access point, same
  network.

Default colour bands (heuristics, as used by common Wi-Fi survey guidance):

| Metric | Good | Fair | Poor |
| --- | --- | --- | --- |
| Signal | ≥ −67 dBm | −68 to −75 | < −75 |
| SNR | ≥ 25 dB | 15–25 | < 15 |
| Retries | ≤ 10 % | 10–25 % | > 25 % |
| Channel busy | ≤ 50 % | 50–70 % | > 70 % |

## Router telemetry

- **CPU** = Δ(non-idle jiffies) / Δ(total jiffies) from `/proc/stat`.
- **Load**: `/proc/loadavg`. **Memory used** = 1 − MemAvailable / MemTotal.
- **Temperature**: hottest `/sys/class/thermal/thermal_zone*/temp`.
- **Conntrack**: `nf_conntrack_count` and `nf_conntrack_max`.
- **Interface rates**: Δbytes × 8 / Δt from `/sys/class/net/<if>/statistics`.
  Errors and drops add receive and transmit counters.
- **WireGuard handshake age**: now − newest `latest-handshake` across the
  interface's peers, from `wg show all dump`. Above 180 s with traffic means
  the tunnel is not rekeying.
