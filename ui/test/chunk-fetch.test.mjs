import assert from 'node:assert/strict';
import test from 'node:test';
import { zstdCompressSync } from 'node:zlib';
import { fetchChunk, StaleFiles, staleFiles } from '../src/chunk-fetch.ts';

const reply =
  (body, { type = 'application/octet-stream', redirected = false, status = 200 } = {}) =>
  async (url) => ({
    ok: status >= 200 && status < 300,
    status,
    redirected,
    url: redirected ? 'http://192.168.8.1/' : url,
    headers: new Headers({ 'content-type': type }),
    arrayBuffer: async () => new Uint8Array(body).buffer,
  });

const html = Buffer.from('<!DOCTYPE html><html><head><title>GL.iNet Admin Panel</title>');
const zstd = zstdCompressSync(Buffer.from('export default 1;'));

test('accepts a zstd chunk whatever its content type', async () => {
  await fetchChunk('/x.zst', reply(zstd, { type: 'text/html' }));
  const bytes = await fetchChunk('/views/wayline/core-a.js.zst', reply(zstd));
  assert.deepEqual(Array.from(bytes.subarray(0, 4)), [0x28, 0xb5, 0x2f, 0xfd]);
});

test('rejects the admin page that nginx returns after redirecting a missing chunk', async () => {
  await assert.rejects(
    fetchChunk('/views/wayline/old.js.zst', reply(html, { type: 'text/html', redirected: true })),
    (error) => error instanceof StaleFiles && error.message.startsWith(staleFiles),
  );
});

test('rejects HTML by its bytes, whatever the content type', async () => {
  await assert.rejects(fetchChunk('/x.zst', reply(html, { type: 'text/html' })), /not a zstd file/);
  await assert.rejects(fetchChunk('/x.zst', reply(html, { type: '' })), /not a zstd file/);
  await assert.rejects(fetchChunk('/x.zst', reply(new Uint8Array())), /not a zstd file/);
});

test('turns a failed cross-origin redirect and HTTP errors into the reload message', async () => {
  const blocked = async () => {
    throw new TypeError('Failed to fetch');
  };
  await assert.rejects(fetchChunk('/x.zst', blocked), (error) => error instanceof StaleFiles);
  await assert.rejects(fetchChunk('/x.zst', reply(zstd, { status: 404 })), /HTTP 404/);
});
