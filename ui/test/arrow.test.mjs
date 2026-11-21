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
import { decodeFrame, decodeHistory, latencyDoc, wifiDoc } from '../src/sources/arrow.ts';

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
    '0.sent': [4, 4, 4, 0],
    '0.lost': [0, 4, 1, 0],
  };
  const types = {
    '0.median': int32(),
    '0.min': int32(),
    '0.max': int32(),
    '0.jitter': int32(),
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
