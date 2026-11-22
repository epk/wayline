# Relay design (planned, not built)

This page describes how a router will send its measurements off the router
for remote diagnosis. Nothing here is built yet. It is written down so that
the storage and clock work already shipped lines up with it.

## Principles

- **Local first.** The router records, stores and serves its own data with no
  network beyond its probe targets. A laptop plugged into the LAN port always
  sees the full page and full history, whether the relay is configured,
  failing or off.
- **Passive path.** The relay only reads what the collector has already
  written. It never blocks collection, never holds the history lock longer
  than one file read, and failing or slow uploads change nothing on the
  router except the relay's own cursor and status.
- **Portable router.** Uplinks come and go: hotel Wi-Fi, captive portals,
  tethering, nothing for days. Every upload retries, and anything missed is
  backfilled from local Parquet when connectivity returns.
- **Primitive pieces.** One process on the router, one Durable Object per
  device in Cloudflare, plain HTTPS. No data platform, no broker.

## On the router: `wayline-ctl uplink`

A long-running procd service, separate from the per-minute collector. It is
enabled only when credentials exist.

1. **Live.** After each collection it reads the newly closed 15-second buckets
   and the events with `seq` above its cursor, and posts them.
2. **Backfill.** If the last acknowledged time lags, it walks the local
   Parquet chunks newest first: recent trouble matters most when diagnosing.
   It uploads the finest tier still kept for each period, so a week offline
   sends 15-second data for the last 6 hours and 5-minute data before that.
3. **Retry.** Failed posts back off exponentially with full jitter, from 5 s
   up to 15 minutes, and reset when a post succeeds. A 4xx other than 408 or
   429 stops the relay and records the reason in `get_state` and Recent
   changes; it does not retry a request the server rejected.
4. **Clock.** It sends nothing stamped before the clock is synced for this
   boot (see [metrics.md](metrics.md#clock-without-a-real-time-clock)).
   Staged buckets are relayed after they are restamped and stored.

State lives in `/etc/wayline/relay.json`: per `(kind, step)` the
`ack_through` timestamp, plus the event `seq` acknowledged. It is written
only after the server acknowledges, so a crash re-sends rather than skips.

### Modes

| Mode | Sends |
| --- | --- |
| `live` | 15-second buckets as they close, events, backfill |
| `reduced` | 1-minute rollups every 5 minutes, events, backfill (metered links) |
| `archive-only` | Parquet chunks to object storage, nothing live |
| `off` | Nothing; the default |

The router chooses `reduced` automatically on a tethered or metered uplink
when the mode is `live`.

### Credentials

`/etc/wayline/credentials.json`, mode 0600, separate from `settings.json` so
that settings can be exported and shown without secrets. It holds the relay
URL and a device token that can only write this device's data. It cannot read
anything back or touch other devices. Archive uploads use presigned URLs
handed out by the server, so no storage keys live on the router.

## Payload

`POST /v1/devices/<device id>/ingest`, body NDJSON, zstd-compressed
(`Content-Encoding: zstd`), at most 256 KB per request:

```
{"type":"bucket","kind":"latency","step":15,"series":"<digest>","ts":1763700000000,"median_us":21000,"...":"one field per schema column"}
{"type":"event","seq":118,"at":1763700012000,"subject":"wan","kind":"wifi","text":"..."}
{"type":"dimension","table":"latency","series":"<digest>","name":"Direct WAN","valid_from":1763600000000,"valid_to":null}
```

Field names are the column names from `protocol/src/schema.rs`, so the same
registry describes disk, wire and relay. The first line of each request is a
header with `device`, `schema` and the router's boot id.

The reply is `{"ack": {"latency/15": 1763700000000, "events": 118}}`. Each
row's key is `(device, kind, step, series, ts)` and each event's is
`(device, seq)`; the server upserts, so re-sending is always safe.

## In Cloudflare

- **One Durable Object per device**, with SQLite storage. It keeps a short
  hot window (15-second data for 48 hours, events for 30 days) for
  semi-realtime views and for an MCP server that diagnoses one router at a
  time. Its alarm compacts older rows away.
- **Long history stays in Parquet.** `archive-only` and a daily job copy
  closed chunks unchanged into R2 under `device=<id>/kind=…/step=…/`, so a
  query engine reads the same files the router wrote.
- A hosted page or MCP server aggregates across devices by fanning out to
  the Durable Objects for recent data and reading R2 for older data.

## Not covered yet

Event log export, captive portal detection, DNS checks and remote diagnosis
context are deliberately left for later passes.
