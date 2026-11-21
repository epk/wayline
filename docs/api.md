# Wayline page API (contract v4)

This API is private to the in-router page and changes with it. Tools and
off-router consumers should use the stable contracts instead: the Parquet files
and their metadata ([metrics.md](metrics.md)), `dimensions.parquet`, and the
relay payload ([relay.md](relay.md)).

The page talks to `wayline-ctl` through the GL.iNet RPC bridge in
`package/files/usr/lib/oui-httpd/rpc/wayline`. Reads use the RPC object
`wayline`. Writes use the versioned object `wayline_<version with dots as underscores>`.
Every call is `$rpcRequest('call', ['sid', object, method, params])`.

Reads: `get_state`, `get_history`, `get_wifi_history`, `get_telemetry_history`.
Writes: `set_config`, `select_tunnel`, `set_tunnel_enabled`, `purge_history`,
`check_update`, `install_update` (unchanged from v1).

Times are Unix milliseconds unless a field name ends in `Seconds` or the field
is documented as seconds. Latency values on the wire are integer microseconds.

## Measurement data is Arrow

Measurement data is columnar from disk to chart. History reads accept
`"format": "arrow"`, and the page always sends it. The reply is JSON:

```json
{ "format": "arrow", "start": 1763700000000, "step": 60, "count": 1441, "...": "metadata", "data": "<base64>" }
```

`data` is base64 of an Arrow IPC stream (one record batch) whose buffers use
Arrow body compression ZSTD. History is stored as Parquet on the router and
converted to Arrow per request. Any Arrow implementation reads it: arrow-rs,
pyarrow, Polars, or flechette in the page. The table has exactly `count` rows,
and row `i` covers `[start + i * step * 1000, start + (i + 1) * step * 1000)`.
A null cell means no data.

Decode in the browser with flechette and fzstd. Pass a preallocated output
buffer, because fzstd otherwise returns an unaligned view:

```js
setCompressionCodec(CompressionType.ZSTD, {
  decode: (bytes, length) => decompress(bytes, new Uint8Array(length)),
});
const table = tableFromIPC(bytes, { useBigInt: false });
```

Without `format`, the reply is the same data as a plain JSON document (shapes
below), for tools. Control-plane state (`get_state`) and commands stay JSON:
they are small nested objects, not tables.

## get_state

Unchanged v1 fields, plus:

- `device`: the router's random id (32 hex characters), as written into
  every Parquet file.
- `clock: { synced, source, pending }`: whether the wall clock is trusted for
  this boot, why (`ntp`, `step`, `connectivity` or `null`), and whether
  buckets recorded before sync are staged and waiting to be stored. Show the
  page's times as provisional while `synced` is false.
- `tunnels[].samples`, `tunnels[].lost`, `uplinks[].samples`,
  `uplinks[].lost`: probes in the recent window and how many were lost.

- `config.upstreamWifi: boolean`, `config.telemetry: boolean`.
- `probe: { interval: seconds, timeout: ms }`.
- `tunnels[].jitter`, `uplinks[].jitter`: `number | null`, milliseconds, mean
  absolute difference between consecutive successful RTTs in the recent window.
- `wifi`: the v1 Wi-Fi view (see `control/src/wifi.rs` `View`).
- `telemetry`:

```json
{
  "enabled": true,
  "sampleAgeMs": 1200,
  "interval": 5,
  "system": {
    "cpu": 12.5, "load": [0.21, 0.18, 0.12],
    "memTotal": 1032192000, "memAvailable": 640000000,
    "temperature": 58.2, "conntrack": 412, "conntrackMax": 16384,
    "uptime": 123456
  },
  "interfaces": [
    {
      "id": "uplink:wan", "name": "wan", "device": "eth1", "kind": "uplink", "present": true,
      "rxBps": 1520000, "txBps": 220000, "rxBytes": 1234567890, "txBytes": 34567890,
      "errors": 0, "drops": 3, "handshakeAge": null, "endpoint": null
    }
  ]
}
```

`kind` is `uplink`, `tunnel`, or `wifi`. `present` is false when the network
device does not exist right now (rates are then `null`). `handshakeAge` (seconds) and
`endpoint` are set for WireGuard tunnels. `system` is `null` until the first
sample. Rates are bits per second over the last sampling interval.

- `incidents`: most recent first, at most 50, from the last 6 hours.

```json
{ "subject": "wan", "name": "Direct WAN", "kind": "loss", "start": 1763700000000, "end": 1763700045000, "detail": "All probes to 1.1.1.1 lost" }
```

`kind` is `loss` (every probe in consecutive 15-second periods lost) or `wifi`
(the repeater was disconnected). `end` is `null` while ongoing.

## get_history

Request: v1 fields (`start`, `end`, `step`, `tunnel`, `target`, `all`,
`include_wan`, `endpoints`, `uplinks`) plus `format`.

Arrow metadata: `series: [{id, name}]`, `events`, `incidents`. Columns for
series index `k` (0-based, in `series` order): `k.median`, `k.min`, `k.max`,
`k.jitter`, `k.p50`, `k.p95`, `k.p99` (Int32 microseconds, nullable),
`k.sent`, `k.lost` (UInt32). Percentiles come from the RTT histogram; see
[metrics.md](metrics.md) for their error bound.

JSON document:

```json
{
  "start": 0, "step": 60, "count": 0,
  "series": [
    { "id": "home", "name": "Tunnel A",
      "median": [], "min": [], "max": [], "jitter": [],
      "p50": [], "p95": [], "p99": [], "sent": [], "lost": [] }
  ],
  "events": [ { "at": 0, "seq": 0, "subject": "", "kind": "", "text": "" } ],
  "incidents": []
}
```

`median`, `min`, `max`, `jitter`, `p50`, `p95`, `p99` are microseconds. Events carry `seq`, a per-router counter that increases with every
recorded event and survives purge (0 for events written before v4), so a
consumer can tell which events it has already seen. `sent` and `lost` are probe counts.

## get_wifi_history

Request: `start`, `end`, `step`, `format`.

Arrow metadata: `summary`, `events`. Columns use the JSON `columns` names.
Float64 nullable: `uptime`, `signal`, `noise`, `snr`, `txRate`, `txRateMin`,
`rxRate`, `expected`, `retries`, `failures`, `busy`, `rxAir`, `txAir`,
`upload`, `download`. Int32 nullable: `signalMin`, `signalMax`. UInt32:
`samples`, `disconnects`, `roams`. Int64 nullable: `txBytes`, `rxBytes`.

JSON document:

```json
{
  "start": 0, "step": 60, "count": 0,
  "summary": { "samples": 0, "uptime": null, "signal": null, "signalMin": null,
    "signalMax": null, "noise": null, "snr": null, "txRate": null, "txRateMin": null,
    "rxRate": null, "expected": null, "retries": null, "failures": null, "busy": null,
    "rxAir": null, "txAir": null, "upload": null, "download": null, "txBytes": 0,
    "rxBytes": 0, "disconnects": 0, "roams": 0 },
  "columns": { "samples": [], "uptime": [], "signal": [], "signalMin": [],
    "signalMax": [], "noise": [], "snr": [], "txRate": [], "txRateMin": [],
    "rxRate": [], "expected": [], "retries": [], "failures": [], "busy": [],
    "rxAir": [], "txAir": [], "upload": [], "download": [], "txBytes": [],
    "rxBytes": [], "disconnects": [], "roams": [] },
  "events": []
}
```

Units: dBm, dB, Mbit/s for rates and `upload`/`download`, percent for
`uptime`, `retries`, `failures`, `busy`, `rxAir`, `txAir`, bytes for byte counts.

## get_telemetry_history

Request: `start`, `end`, `step`, `format`.

Arrow metadata: `interfaces: [{id, name, device, kind}]`, `conntrackMax`.
Float64 nullable: `system.cpu`, `system.cpuMax`, `system.load`,
`system.memUsed`, `system.temperature`, `system.temperatureMax`. Int64
nullable: `system.memAvailableMin`, `system.conntrack`, and for interface index
`k`: `k.rxBps`, `k.txBps`, `k.rxPeakBps`, `k.txPeakBps`, `k.rxBytes`,
`k.txBytes`, `k.errors`, `k.drops`, `k.handshakeAgeMax`.

JSON document:

```json
{
  "start": 0, "step": 60, "count": 0,
  "system": {
    "cpu": [], "cpuMax": [], "load": [], "memUsed": [], "memAvailableMin": [],
    "temperature": [], "temperatureMax": [], "conntrack": [], "conntrackMax": null
  },
  "interfaces": [
    { "id": "uplink:wan", "name": "wan", "device": "eth1", "kind": "uplink",
      "rxBps": [], "txBps": [], "rxPeakBps": [], "txPeakBps": [],
      "rxBytes": [], "txBytes": [], "errors": [], "drops": [], "handshakeAgeMax": [] }
  ]
}
```

`cpu`, `cpuMax`, `memUsed` are percent. `load` is the 1-minute load average.
`temperature` is degrees Celsius. Rates are average and peak bits per second.
