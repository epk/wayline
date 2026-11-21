import assert from 'node:assert/strict';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import test from 'node:test';
import { zstdDecompressSync } from 'node:zlib';

const dir = new URL('../views/wayline/', import.meta.url);
const report = JSON.parse(
  readFileSync(new URL('../dist/size-report.json', import.meta.url), 'utf8'),
);
const loader = readFileSync(
  new URL('../dist/gl-sdk4-ui-wayline.common.js', import.meta.url),
  'utf8',
);

test('every chunk ships as plain and zstd JavaScript with identical content', () => {
  for (const file of Object.keys(report.chunks)) {
    const plain = readFileSync(new URL(file, dir));
    const packed = readFileSync(new URL(file + '.zst', dir));
    assert.ok(zstdDecompressSync(packed).equals(plain), file);
  }
});

test('the output directory holds only the chunks the loader knows', () => {
  const known = new Set(Object.keys(report.chunks).flatMap((f) => [f, f + '.zst']));
  assert.deepEqual(
    readdirSync(dir).filter((f) => !known.has(f)),
    [],
  );
});

test('the loader embeds the content-hashed manifest for every entry', () => {
  for (const [name, file] of Object.entries(report.entries)) {
    assert.match(file, new RegExp('^' + name + '-[A-Za-z0-9_-]{8}\\.js$'));
    assert.ok(loader.includes(JSON.stringify(file)), file);
    assert.ok(existsSync(new URL(file, dir)), file);
  }
  assert.match(loader, /["'`]\/views\/wayline\/["'`]/);
});

test('chunk references use forms the loader rewrites', () => {
  for (const file of Object.keys(report.chunks)) {
    const code = readFileSync(new URL(file, dir), 'utf8');
    for (const ref of code.matchAll(/["'`]\.\/([^"'`]+\.js)["'`]/g))
      assert.ok(report.chunks[ref[1]], `${file} refers to unknown ${ref[1]}`);
  }
});

test('the first load stays within budget and excludes lazy views', () => {
  assert.ok(report.firstLoad.zstd <= 125000, String(report.firstLoad.zstd));
  assert.ok(report.total.minified <= 600000, String(report.total.minified));
  for (const name of ['paths', 'wifi', 'traffic', 'router', 'events', 'settings', 'agent'])
    assert.ok(!report.firstLoad.files.includes(report.entries[name]), name);
  assert.ok(!report.firstLoad.files.some((f) => f.startsWith('chart-')));
});
