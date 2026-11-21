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
Coarser tiers are rollups of finer ones. Sums (counts, histogram bins, jitter
sums), minimums and maximums combine without loss. The `median_us` column of a
coarse period is the lower median of its children's medians, which is an
approximation; for percentiles of a long period use the histogram below.
Live data not yet saved is rolled up through the same tiers at query time, so a
period reads the same before and after it is saved.

## Storage and transport

Columnar end to end, with each format used for what it was designed for:
Parquet at rest, Arrow in motion.

- **Disk: Parquet.** Self-describing, Hive-partitioned files that make sense
  off the router:

  ```
  /etc/wayline/history/            (memory only: /var/run/wayline/history/)
    kind=latency/step=15/20261121T180000Z.parquet
    kind=latency/step=60/20261121T180000Z.parquet
    kind=wifi/...  kind=system/...  kind=interface/...
    dimensions.parquet
  ```

  Each chunk is named by its UTC start. Columns: `series` (UTF-8, dictionary
  encoded), `ts` (TIMESTAMP, milliseconds, UTC: the bucket start) and the
  kind's measurements (INT64, nullable, delta encoded). Pages are
  zstd-compressed with min/max statistics. Key-value metadata records
  `wayline.schema`, `wayline.kind`, `wayline.description`,
  `wayline.device` (the router's id, below), `wayline.step_seconds`,
  `wayline.span_seconds` and `wayline.columns`, which lists every column's
  unit, description and rollup rule (`sum`, `min`, `max` or
  `median_of_medians`). The rollup rule tells a consumer how to aggregate
  further: sums add, minimums and maximums take the extreme, and
  `median_of_medians` must not be averaged.
- **Schema evolution.** The column registry lives in
  `protocol/src/schema.rs`. Columns are only ever added. A change of unit or
  rollup rule gets a new column name. Readers match columns by name: a column
  missing from an older file reads as null, and a column a reader does not know
  is ignored, so old and new files coexist in one directory.
- **Device id.** A random 128-bit id is created once in
  `/etc/wayline/identity.json` and written into every file. It survives purge
  and upgrades, and carries nothing about the router or its owner.
- **Dimensions.** Series keys are digests that keep tunnel identities private.
  `dimensions.parquet` maps `(table, series)` to `kind` (tunnel, wan, uplink,
  endpoint, wifi, system), `subject` (the id the page and API use), `name`,
  `target` and `device`. It is versioned (a type 2 slowly changing dimension):
  each row is valid from `valid_from` to `valid_to` (UTC, null while current).
  A rename or a new destination closes the old row and opens a new one; a
  removed path keeps its last row with `valid_to` set, so old history still
  joins to the name it had at the time.
- **Wire and page: Arrow IPC.** History replies are one Arrow IPC stream with
  zstd buffers, decoded in the page straight into typed arrays for the charts.
- **Control plane: JSON.** `get_state` and commands are small nested objects.

| Tier | Chunk |
| --- | --- |
| 15 s | 1 hour |
| 1 min | 6 hours |
| 5 min | 1 day |
| 1 hour | 7 days |

Each chunk's `wayline.span_seconds` records the period it covers.

Every 5 minutes wayline-ctl merges new 15-second rows into the open chunk,
recomputes the affected coarser periods, and rewrites each changed file
atomically (write, fsync, rename). Only the chunks of the current period
change; older files are immutable, which keeps incremental copies cheap.
Retention deletes whole files, so data can outlive the nominal period by up to
one chunk. A file that cannot be read is renamed to `.broken` and logged in
Recent changes; only that chunk is lost. Storage is capped at 96 MB on flash
and 8 MB in memory, oldest chunks first.

Measured for one week of eleven latency series, all tiers:

| Store | Size |
| --- | --- |
| SQLite (earlier versions) | 2.9 MB |
| Arrow IPC, zstd buffers | 360 KB |
| Parquet, delta + dictionary, zstd | 91 KB |

Parquet adds about 57 KB to wayline-ctl on top of the Arrow writer the wire
needs, and writes and reads a week of chunks faster than Arrow IPC files.

Query the files with DuckDB, Polars or pyarrow, on the router or a copy:

```sql
SELECT d.name, date_trunc('hour', h.ts) AS hour,
       sum(h.lost) * 100.0 / sum(h.sent) AS loss_pct,
       sum(h.jitter_sum) / sum(h.jitter_n) / 1000 AS jitter_ms,
       sum(h.rtt_100_150ms + h.rtt_150_250ms + h.rtt_250_500ms + h.rtt_ge_500ms)
         * 100.0 / (sum(h.sent) - sum(h.lost)) AS over_100ms_pct
FROM read_parquet('history/kind=latency/step=300/*.parquet') h
JOIN 'history/dimensions.parquet' d
  ON d.table = 'latency' AND d.series = h.series
 AND h.ts >= d.valid_from AND (d.valid_to IS NULL OR h.ts < d.valid_to)
GROUP BY ALL ORDER BY hour;
```

The time-ranged join gives each row the name the path had when it was
measured. Join on `(table, series)` with `valid_to IS NULL` for today's names.

`latency.sqlite` from earlier versions is left in place and no longer read.
Purge history deletes it.

### Planned: export and off-router views

The design for relaying data off the router, with retries and backfill, is in
[relay.md](relay.md). The layout is meant to be copied as is. A scheduled export can upload changed
files to any S3-compatible bucket (Cloudflare R2, AWS S3, or Google Cloud
Storage through its S3 interoperability API) under `router=<id>/`, keeping
`kind=`/`step=` partitions. A Cloudflare-hosted MCP server or the hosted page
can then query the same Parquet with DuckDB or R2 SQL, using `dimensions.parquet`
for names and the column metadata for units and rollup rules. The page
reads through one data-source interface, so a hosted build only needs a new
source.

## Clock without a real-time clock

GL.iNet travel routers have no battery-backed clock. They boot at the firmware
build time and jump to the real time when NTP succeeds, which can be minutes
later, or never on a captive network.

- Every bucket records `uptime_ms`, the time since boot at its start, next to
  its wall-clock start. Each status snapshot carries the kernel boot id, the
  uptime and the wall clock at the same moment.
- The clock counts as synced for a boot when any of these happens first: the
  NTP hotplug hook (`/etc/hotplug.d/ntp/95-wayline`) reports a stratum other
  than 16; the wall clock steps more than 60 s against uptime; or the router has
  been up 15 minutes and a probe has a reply.
- Until then, every 5-minute collection appends the closed buckets to
  `history/pending/<boot id>.ndjson` instead of the Parquet store. On sync the
  staged buckets are moved to `uptime_ms + (wall − uptime)`, floored to
  15 s, and stored. Buckets already within 30 s of that time are left as they are.
- Staged files from earlier boots that never synced are stored with the
  timestamps they have, and logged in Recent changes. Staging is capped at
  16 MB.
- `get_state.clock` reports `synced`, the `source` (`ntp`, `step` or
  `connectivity`) and whether staged data is waiting.

## Latency, loss, jitter

- **Latency**: round-trip time of a 32-byte ICMP echo through the path, bound
  to the path's interface, routing mark and source address. Per bucket: min,
  lower median, max, and a histogram of every reply.
- **Histogram**: 16 bins with edges at 1, 2, 3, 5, 7.5, 10, 15, 20, 30, 50, 75,
  100, 150, 250 and 500 ms (columns `rtt_lt_1ms` … `rtt_ge_500ms`). Bins are
  counts, so any period sums them exactly. **p50, p95, p99** are read from the
  summed bins by linear interpolation inside the bin that holds the rank, with
  the bin clamped to the period's min and max. The error is at most the width
  of that bin: under 1 ms below 3 ms, under 5 ms below 20 ms, 50 ms between 100
  and 150 ms. The open last bin is bounded by the period's max.
- **Loss**: probes without a matching reply within the timeout (2 s) divided by
  probes sent. The page shows the destination and the probe count beside it.
- **Jitter**: mean absolute difference between consecutive successful RTTs on
  the same path, `J = Σ|RTTᵢ − RTTᵢ₋₁| / n`. A lost probe breaks the chain.
  Buckets keep the sum and count, so rollups stay exact. This is the
  interarrival form of RFC 3550 without its 1/16 smoothing.
- **Collection cadence**: every 5 minutes, or every minute while any path's
  latest bucket lost every probe or upstream Wi-Fi is disconnected, so a
  trouble window reaches disk before a reboot can take it.
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
