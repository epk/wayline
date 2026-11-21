import assert from 'node:assert/strict';
import test from 'node:test';
import {
  latencyLine,
  lossClass,
  offered,
  runs,
  stepFor,
  summarize,
  times,
  uptime,
} from '../src/series.ts';

const tiers = [
  { step: 15, keptSeconds: 21600 },
  { step: 60, keptSeconds: 259200 },
  { step: 300, keptSeconds: 604800 },
];

test('point i is start + i * step', () => {
  assert.deepEqual(Array.from(times({ start: 1000000, step: 60, count: 3 })), [1000, 1060, 1120]);
});

test('picks the finest tier that keeps the range within 2400 points', () => {
  assert.equal(stepFor(tiers, 60), 15);
  assert.equal(stepFor(tiers, 360), 15);
  assert.equal(stepFor(tiers, 1440), 60);
  assert.equal(stepFor(tiers, 10080), 300);
  assert.equal(stepFor(tiers, 60, 1440), 60);
  assert.equal(stepFor(tiers, 43200), 0);
  assert.ok(offered(tiers, 10080));
  assert.ok(!offered(tiers, 43200));
});

test('converts microseconds and summarizes a series', () => {
  const doc = { start: 0, step: 15, count: 4 };
  const line = latencyLine(doc, {
    id: 'a',
    name: 'A',
    median: Int32Array.from([10000, 20000, 30000, 40000]),
    min: [9000, null, 29000, 39000],
    max: [11000, null, 31000, 45000],
    jitter: [1000, 1000, 2000, null],
    sent: Uint32Array.from([4, 4, 4, 4]),
    lost: Uint32Array.from([0, 0, 1, 4]),
  });
  assert.equal(line.median[0], 10);
  assert.equal(line.loss[2], 25);
  const stats = summarize(line);
  assert.equal(stats.last, 40);
  assert.equal(stats.mean, 25);
  assert.equal(stats.min, 9);
  assert.equal(stats.max, 45);
  assert.ok(Math.abs(stats.jitter - 4 / 3) < 1e-9);
  assert.equal(stats.loss, (5 / 16) * 100);
  assert.ok(stats.score);
});

test('uptime counts fully lost buckets among buckets with probes', () => {
  const u = uptime([4, 4, 0, 4], [0, 4, 0, 1]);
  assert.equal(u.full, 1);
  assert.equal(u.probed, 3);
  assert.ok(Math.abs(u.uptime - 200 / 3) < 1e-9);
  assert.equal(uptime([0], [0]).uptime, null);
});

test('loss classes and strip runs', () => {
  assert.deepEqual(
    [
      lossClass(0, 0),
      lossClass(200, 0),
      lossClass(200, 1),
      lossClass(100, 2),
      lossClass(10, 1),
      lossClass(4, 4),
    ],
    [0, 1, 2, 3, 4, 5],
  );
  assert.deepEqual(runs([4, 4, 4, 0, 4], [0, 0, 4, 0, 0]), [
    { start: 0, length: 2, level: 1 },
    { start: 2, length: 1, level: 5 },
    { start: 3, length: 1, level: 0 },
    { start: 4, length: 1, level: 1 },
  ]);
});
