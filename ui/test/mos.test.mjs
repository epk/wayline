import assert from 'node:assert/strict';
import test from 'node:test';
import { band, score, scoreColumn } from '../src/mos.ts';

const near = (actual, expected, epsilon = 1e-9) =>
  assert.ok(Math.abs(actual - expected) < epsilon, `${actual} is not ${expected}`);

const reference = (latency, jitter, loss) => {
  const el = latency + 2 * jitter + 10;
  let r = el < 160 ? 93.2 - el / 40 : 93.2 - (el - 120) / 10;
  r = Math.min(100, Math.max(0, r - 2.5 * loss));
  return 1 + 0.035 * r + 7e-6 * r * (r - 60) * (100 - r);
};

test('follows the documented E-model below 160 ms effective latency', () => {
  const s = score(20, 2, 0);
  near(s.el, 34);
  near(s.r, 93.2 - 34 / 40);
  near(s.mos, reference(20, 2, 0));
  assert.equal(s.band, 'excellent');
});

test('switches to the steep branch at 160 ms and subtracts loss', () => {
  const s = score(140, 5, 1);
  near(s.el, 160);
  near(s.r, 93.2 - 40 / 10 - 2.5);
  near(s.mos, reference(140, 5, 1));
});

test('clamps R to 0..100', () => {
  near(score(2000, 100, 100).r, 0);
  near(score(2000, 100, 100).mos, 1);
  near(score(0, 0, -100).r, 100);
});

test('bands match docs/metrics.md', () => {
  assert.equal(band(4.3), 'excellent');
  assert.equal(band(4.29), 'good');
  assert.equal(band(4.0), 'good');
  assert.equal(band(3.99), 'fair');
  assert.equal(band(3.6), 'fair');
  assert.equal(band(3.59), 'poor');
});

test('needs latency, jitter and loss', () => {
  assert.equal(score(null, 1, 0), null);
  assert.equal(score(10, null, 0), null);
  assert.equal(score(10, 1, null), null);
  assert.equal(score(Number.NaN, 1, 0), null);
});

test('scores columns point by point', () => {
  const out = scoreColumn([20, null, 300], [2, 1, 10], [0, 0, 5]);
  near(out[0], reference(20, 2, 0));
  assert.equal(out[1], null);
  near(out[2], reference(300, 10, 5));
});
