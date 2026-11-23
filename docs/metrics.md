# How Wayline measures

Every number on the page comes from one of the measurements below. Scores are
derived from these raw values with the formulas on this page, and the page
shows the inputs next to each score.

## Sampling and retention

| Source | Interval | Bucket | Kept (history setting 7 days) |
| --- | --- | --- | --- |
| ICMP probes per path and destination | 1–60 s (setting) | 15 s | 15 s for 6 h, 1 min for 3 days, 5 min for 7 days |
| Upstream Wi-Fi (nl80211) | 5 s, and at each connect or disconnect | 15 s | same tiers |
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
    kind=event/20261121.parquet
    kind=incident/20261121.parquet
    dimensions.parquet
  ```

  `kind=event` and `kind=incident` hold individual records, one file per UTC
  day, and are not rolled up. Events have `ts`, `seq`, `subject`, `kind` and
  `text`; incidents have `ts` (start), `end` (null while ongoing), `series`,
  `kind` and `seen` (end of the last lost bucket seen so far). An ongoing
  incident is closed at `seen` once nothing has extended it for two buckets,
  for example after a restart emptied the live ring, and it only absorbs a
  later run that starts within one bucket of `seen`. Both are kept as long as the longest metric tier, so every view
  covers the same range.

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

Every 5 minutes wayline-ctl merges new 15-second rows into the open chunk and
rewrites it atomically (write, fsync, rename). The coarser tiers are written
once an hour, after the hour has closed, and again only if late rows change
them; everything newer is rolled up from 15-second rows at query time. This
keeps flash writes to the open 15-second chunk plus one coarser rewrite an
hour, instead of rewriting the open 1-minute and 5-minute chunks at every
collection. Older
files are immutable, which keeps incremental copies cheap. Retention deletes
whole files, so data can outlive the nominal period by up to one chunk. A file
that does not decode is renamed to `.broken`, logged in Recent changes, and
deleted after 3 days; only that chunk is lost. A file that cannot be read
(an I/O error or one above the size limit) is reported as an error and left in
place. Storage is capped at 96 MB on flash and 8 MB in memory. The cap counts
every history file (chunks, event and incident tables, dimensions, staged and
damaged files); when it is exceeded, damaged files go first, then the oldest
chunks, and the removal is logged in Recent changes.

Switching retention between memory only and flash deletes the store the old
setting used and logs that history starts fresh: flash history does not fit
the 8 MB memory cap, and memory history covers at most a day. Doctor and the
Health tab report the unused store if it is still there.

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

The SQLite store of 0.1.0-alpha.24 and earlier (`latency.sqlite` with its
journal files, `telemetry.sqlite` and `series.json`) is renamed once to
`<name>.legacy`, logged in Recent changes, and not shown or counted toward the
cap. Purge history deletes it.

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
  firmware marks time valid (`/var/state/ntp-valid`, written on tmpfs by GL.iNet
  firmware after chrony reports a normal leap status within 5 s, by busybox
  ntpd's stratum hook on older 4.x releases, or by a manual time sync); the
  wall clock steps more than 60 s against uptime; or the router has been up
  15 minutes and a probe has a reply.
- Until then, every 5-minute collection appends the closed buckets to
  `history/pending/<boot id>.ndjson` instead of the Parquet store. On sync the
  staged buckets are moved to `uptime_ms + (wall − uptime)`, floored to
  15 s, and stored. Buckets already within 30 s of that time are left as they are.
- Staged files from earlier boots that never synced are stored with the
  timestamps they have, and logged in Recent changes. Staging is capped at
  16 MB per boot; data dropped at the cap, and staged lines that do not parse,
  are logged in Recent changes when the file is stored. A file that fails does
  not stop the others.
- Trust from connectivity alone is provisional. If the wall clock later steps
  more than 60 s against the offset it was trusted at, the source becomes
  `step`, buckets still in the live ring are re-timed, and Recent changes notes
  that history saved earlier in that boot may be at the wrong time.
- Change events (Recent changes) are not recorded until the clock is synced.
  Changes that happen before that are noticed on the first tick after sync.
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
  A probe slot that cannot be sent because the interface is missing or down,
  the route is missing or points elsewhere, or the socket or send fails counts
  as sent and lost, so an absent tunnel shows as loss rather than a gap.
- **Jitter**: mean absolute difference between consecutive successful RTTs on
  the same path, `J = Σ|RTTᵢ − RTTᵢ₋₁| / n`. A lost probe breaks the chain.
  Buckets keep the sum and count, so rollups stay exact. This is the
  interarrival form of RFC 3550 without its 1/16 smoothing.
- **Collection cadence**: every 5 minutes, or every minute while any path's
  latest bucket lost every probe or upstream Wi-Fi is disconnected, so a
  trouble window reaches disk before a reboot can take it.
- **Incident**: a run of consecutive 15-second buckets in which every probe was
  lost. It ends with the first bucket that has a reply; a gap without probes
  also ends it. The incident log (`kind=incident`) is written at 15-second
  resolution on every collection and kept as long as the rest of history, so
  it outlives the 15-second tier. History views add logged incidents that
  their coarser buckets no longer show.
  Upstream Wi-Fi incidents are runs of buckets in which the repeater was never
  associated (`wifi`), or in which the radio sat on another channel than the
  access point for most connected samples (`off_channel`). History views derive
  incidents at the resolution they show.
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

Read straight from the kernel over generic netlink (nl80211), without ubus or
`iwinfo`: `GET_INTERFACE` for every station-mode interface (SSID, frequency,
width, transmit power), `GET_STATION` for the access point it is associated
with (signal, rates, frame counters, connected time), `GET_SURVEY` for the
serving channel (noise, busy, receive and transmit time) and `GET_SCAN` for the
associated BSS (its security, from the RSN or WPA element). Interface byte
counters come from `/sys/class/net/<if>/statistics` and the driver name from
`/sys/class/net/<if>/device/driver`. The monitor also listens to nl80211
connect, roam and disconnect events and to rtnetlink link changes, and samples
at once when one arrives.

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
- **Radio on another channel**: the survey marks the channel the radio is tuned
  to. When it differs from the access point's channel (from `GET_SCAN`) for 60
  seconds in a row, the link reports `radio_frequency`, the samples count as
  off channel, and channel busy, airtime and noise come from the channel the
  radio is on. The 60-second hold skips background scans. On a GL-MT3600BE the
  MediaTek driver once left the radio on channel 36 for 30 minutes after a
  reconnect to an access point on channel 40; the link stayed associated while
  almost no traffic got through.
- **Routers without Wi-Fi**: when `/sys/class/ieee80211` lists no radio, upstream
  Wi-Fi monitoring cannot be turned on, the controller saves it as off, and the
  page hides the Wi-Fi view.
- **Access point**: the station the client interface is associated with, from
  `GET_STATION`. An interface counts as connected only while it has one, so an
  idle radio that keeps an old SSID or channel is not mistaken for a link.
- **Drop**: the connection is lost (counted once per gap), the same access
  point reassociates (connected time went backwards), or the repeater moves to
  another network without a visible gap. **Roam**: another access point of the
  same network, with or without a gap. The last network is remembered across a
  gap, so a reconnect is told apart from a roam or a change of network.
- **Per access point**: each 15-second bucket is filed under the access point
  that had most of its connected samples (ties keep the first). A bucket with
  no connection is filed under the access point that was lost. Coarser tiers
  keep the access points apart, so per-network summaries stay exact to 15
  seconds. History saved before 0.1.0-alpha.27 has no access point.
- **Changes**: the monitor logs every change of network, access point or
  channel when it happens (at the event, or at the next 5-second sample), and
  the collector turns them into change events at those times, so stays shorter
  than a minute are not lost. Counts from event samples are added to the next
  5-second sample's bucket, so every bucket still holds evenly spaced samples.
- **Reload**: when the uplink changes, the router's hotplug makes the
  controller rewrite the monitor configuration. The monitor checks the file
  every second and applies it in place, keeping every bucket and its Wi-Fi
  state; a probe whose interface or source address changed starts a new path
  and its pending probes count as lost.
- **Throughput** (`upload`, `download`) is bytes over the whole bucket. A speed
  test shorter than the bucket reads lower than its own result.

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
