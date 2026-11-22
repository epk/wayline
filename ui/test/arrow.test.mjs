import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { zstdCompressSync, zstdDecompressSync } from 'node:zlib';
import {
  CompressionType,
  float64,
  int32,
  int64,
  setCompressionCodec,
  tableFromArrays,
  tableToIPC,
  uint32,
} from '@uwdata/flechette';
import { decompress } from 'fzstd';
import { decodeFrame, decodeHistory, latencyDoc, setZstd, wifiDoc } from '../src/sources/arrow.ts';

setZstd(decompress);

const envelope = (bytes, count, meta = {}) => ({
  format: 'arrow',
  start: 1763700000000,
  step: 15,
  count,
  ...meta,
  data: Buffer.from(bytes).toString('base64'),
});

test('decodes the arrow-rs ZSTD fixture with numbers, nulls and typed arrays', async () => {
  const bytes = readFileSync(new URL('./fixtures/fixture.arrows', import.meta.url));
  const frame = await decodeFrame(envelope(bytes, 2017));
  const median = frame.column('0.median');
  const sent = frame.column('0.sent');
  const signal = frame.column('signal');
  const txBytes = frame.column('txBytes');
  assert.equal(median.length, 2017);
  assert.equal(median[0], 20000);
  assert.equal(median[3], null);
  assert.equal(median[4], 20004);
  assert.equal(sent[4], 4);
  assert.ok(sent instanceof Uint32Array);
  assert.equal(signal[0], null);
  assert.equal(signal[1], -61);
  assert.equal(txBytes[0], 5000000000);
  assert.equal(typeof txBytes[0], 'number');
});

test('decodes a zstd-compressed stream written by flechette with a Node codec', async () => {
  setCompressionCodec(CompressionType.ZSTD, {
    encode: (data) => new Uint8Array(zstdCompressSync(data)),
    decode: (data) => new Uint8Array(zstdDecompressSync(data)),
  });
  const columns = {
    '0.median': [12000, null, 15500, 9000],
    '0.min': [11000, null, 14000, 8000],
    '0.max': [14000, null, 19000, 9900],
    '0.jitter': [800, null, 1200, 400],
    '0.p50': [12100, null, 15600, 9100],
    '0.p95': [13500, null, 18000, 9800],
    '0.p99': [13900, null, 18900, 9900],
    '0.sent': [4, 4, 4, 0],
    '0.lost': [0, 4, 1, 0],
  };
  const types = {
    '0.median': int32(),
    '0.min': int32(),
    '0.max': int32(),
    '0.jitter': int32(),
    '0.p50': int32(),
    '0.p95': int32(),
    '0.p99': int32(),
    '0.sent': uint32(),
    '0.lost': uint32(),
  };
  const bytes = tableToIPC(tableFromArrays(columns, { types }), {
    format: 'stream',
    codec: CompressionType.ZSTD,
  });
  const doc = await decodeHistory(
    envelope(bytes, 4, { series: [{ id: 'home', name: 'Tunnel A' }], events: [], incidents: [] }),
    latencyDoc,
  );
  assert.equal(doc.series.length, 1);
  assert.equal(doc.series[0].name, 'Tunnel A');
  assert.deepEqual(Array.from(doc.series[0].median), columns['0.median']);
  assert.deepEqual(Array.from(doc.series[0].lost), columns['0.lost']);
  assert.deepEqual(Array.from(doc.series[0].p95), columns['0.p95']);
  assert.deepEqual(Array.from(doc.series[0].p99), columns['0.p99']);
});

test('maps wifi columns and keeps the summary from metadata', async () => {
  const bytes = tableToIPC(
    tableFromArrays(
      { signal: [-60.5, null], signalMin: [-64, null], samples: [3, 0], txBytes: [7e9, 0] },
      { types: { signal: float64(), signalMin: int32(), samples: uint32(), txBytes: int64() } },
    ),
    { format: 'stream' },
  );
  const doc = await decodeHistory(
    envelope(bytes, 2, { summary: { samples: 3 }, events: [] }),
    wifiDoc,
  );
  assert.equal(doc.summary.samples, 3);
  assert.deepEqual(Array.from(doc.columns.signal), [-60.5, null]);
  assert.equal(doc.columns.txBytes[0], 7e9);
  assert.equal(doc.columns.retries.length, 2);
  assert.equal(doc.columns.retries[0], null);
});

test('passes a plain v2 document through and rejects unknown formats', async () => {
  const plain = { start: 0, step: 60, count: 0, series: [] };
  assert.equal(await decodeHistory(plain, latencyDoc), plain);
  await assert.rejects(decodeFrame({ format: 'parquet', data: '' }), /unknown format/);
  await assert.rejects(
    decodeFrame({ ...envelope(new Uint8Array(), 1), count: 5, data: 'AA==' }),
    Error,
  );
});

test('a wayline-ctl v4 history reply decodes to the same values as its JSON form', async () => {
  const read = (name) => JSON.parse(readFileSync(new URL(`./fixtures/${name}`, import.meta.url)));
  const json = read('history-v4.json');
  const doc = await decodeHistory(read('history-v4.arrow.json'), latencyDoc);
  assert.equal(doc.count, json.count);
  const [decoded] = doc.series;
  const [expected] = json.series;
  for (const key of ['median', 'min', 'max', 'jitter', 'p50', 'p95', 'p99', 'sent', 'lost']) {
    const values = Array.from(decoded[key], (value) => value ?? null);
    assert.deepEqual(values, expected[key], key);
  }
  const row = decoded.p50.findIndex((value) => value !== null);
  assert.ok(row >= 0, 'no percentile data');
  assert.ok(decoded.min[row] <= decoded.p50[row] && decoded.p50[row] <= decoded.p95[row]);
  assert.ok(decoded.p95[row] <= decoded.p99[row] && decoded.p99[row] <= decoded.max[row]);
});

test('joins several record batches and keeps nulls across them', async () => {
  const bytes = tableToIPC(
    tableFromArrays(
      { a: [1, null, 3, 4, null], b: [1.5, 2.5, 3.5, 4.5, 5.5] },
      { types: { a: int32(), b: float64() }, maxBatchRows: 2 },
    ),
    { format: 'stream' },
  );
  const frame = await decodeFrame(envelope(bytes, 5));
  assert.deepEqual(Array.from(frame.column('a')), [1, null, 3, 4, null]);
  assert.deepEqual(Array.from(frame.column('b')), [1.5, 2.5, 3.5, 4.5, 5.5]);
  assert.deepEqual(Array.from(frame.column('missing')), [null, null, null, null, null]);
});
