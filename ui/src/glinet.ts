import { Decompress, decompress } from 'fzstd';
import { manifest } from '\0wayline-manifest';
import { fetchChunk, StaleFiles } from './chunk-fetch.ts';
import type { MountOptions } from './core.tsx';
import type { ChunkName } from './runtime.ts';
import type { GlinetRpcSource, Transport } from './sources/glinet-rpc.ts';
import type { DataSource } from './sources/source.ts';

interface Host {
  $el: HTMLElement;
  api?: Record<string, (params: unknown) => unknown> | null;
  source?: DataSource | null;
  preview?: boolean;
  $rpcRequest?: (method: string, params: unknown[], options: { timeout: number }) => unknown;
  waylineUnmount?: (() => void) | null;
  waylineGone?: boolean;
}

interface Core {
  mount: (el: HTMLElement, options: MountOptions) => () => void;
  GlinetRpcSource: typeof GlinetRpcSource;
}

type Module = Record<string, unknown>;
type CreateElement = (tag: string, data?: Record<string, unknown>) => unknown;

let plain = false;
const urls = new Map<string, Promise<string>>();
const modules = new Map<string, Promise<Module>>();

const importUrl = (url: string): Promise<Module> => import(url);

// Cache a pending load, but forget it if it fails, so the next attempt
// downloads again instead of replaying one transient network error.
function cached<T>(map: Map<string, Promise<T>>, file: string, start: () => Promise<T>) {
  let entry = map.get(file);
  if (!entry) {
    entry = start();
    map.set(file, entry);
    entry.catch(() => {
      if (map.get(file) === entry) map.delete(file);
    });
  }
  return entry;
}

const describe = (e: unknown): string =>
  e && typeof e === 'object' && 'message' in e ? String(e.message) : String(e);

const yieldToMain = (): Promise<void> => {
  const s = (globalThis as { scheduler?: { yield?: () => Promise<void> } }).scheduler;
  if (s && typeof s.yield === 'function') return s.yield();
  return new Promise((resolve) => {
    const channel = new MessageChannel();
    channel.port1.onmessage = () => resolve();
    channel.port2.postMessage(0);
  });
};

async function inflate(bytes: Uint8Array): Promise<string> {
  const text = new TextDecoder();
  if (bytes.length < 32768) return text.decode(decompress(bytes));
  let code = '';
  const stream = new Decompress((chunk) => {
    code += text.decode(chunk, { stream: true });
  });
  for (let at = 0; at < bytes.length; at += 16384) {
    const last = at + 16384 >= bytes.length;
    stream.push(bytes.subarray(at, at + 16384), last);
    if (!last) await yieldToMain();
  }
  return code + text.decode();
}

async function blobsWork(): Promise<boolean> {
  const url = URL.createObjectURL(new Blob(['export default 1'], { type: 'text/javascript' }));
  try {
    return (await importUrl(url)).default === 1;
  } catch {
    return false;
  } finally {
    URL.revokeObjectURL(url);
  }
}

async function unpack(file: string): Promise<string> {
  const deps = manifest.deps[file] || [];
  const [bytes, ...depUrls] = await Promise.all([
    fetchChunk(manifest.base + file + '.zst'),
    ...deps.map(blobUrl),
  ]);
  const start = performance.now();
  let code = await inflate(bytes);
  deps.forEach((dep, i) => {
    for (const quote of ['"', "'", '`'])
      code = code.split(quote + './' + dep + quote).join(JSON.stringify(depUrls[i]));
  });
  performance.measure('wayline decode ' + file, { start, end: performance.now() });
  return URL.createObjectURL(new Blob([code], { type: 'text/javascript' }));
}

const blobUrl = (file: string): Promise<string> => cached(urls, file, () => unpack(file));

const load = (file: string): Promise<Module> =>
  cached(modules, file, () =>
    plain
      ? importUrl(manifest.base + file).catch((error: unknown) => {
          throw new StaleFiles(manifest.base + file + ': ' + describe(error));
        })
      : blobUrl(file).then(importUrl),
  );

async function loadCore(): Promise<Core> {
  if (!plain && !(await blobsWork())) {
    plain = true;
    console.info('Wayline loads plain JavaScript chunks because this page blocks blob: scripts.');
  }
  try {
    return (await load(manifest.core)) as unknown as Core;
  } catch (error) {
    if (plain || error instanceof StaleFiles) throw error;
    console.info(
      'Wayline loads plain JavaScript chunks because compressed chunks failed: ' + describe(error),
    );
    plain = true;
    modules.clear();
    return (await load(manifest.core)) as unknown as Core;
  }
}

const sourceFor = (host: Host, Rpc: typeof GlinetRpcSource): DataSource => {
  if (host.source) return host.source;
  const transport: Transport = (object, method, params, timeout) => {
    if (host.api) return host.api[method](params);
    if (typeof host.$rpcRequest !== 'function')
      throw Error('The router page API is missing. Reload the page or update the firmware.');
    return host.$rpcRequest('call', ['sid', object, method, params], { timeout });
  };
  return new Rpc(transport);
};

async function draw(host: Host): Promise<void> {
  try {
    const core = await loadCore();
    if (host.waylineGone) return;
    host.waylineUnmount = core.mount(host.$el, {
      source: sourceFor(host, core.GlinetRpcSource),
      preview: !!host.preview,
      load: (name: ChunkName) => load(manifest.entries[name]),
      decompress,
    });
  } catch (error) {
    if (host.waylineGone) return;
    const message = document.createElement('p');
    message.setAttribute('role', 'alert');
    message.textContent =
      error instanceof StaleFiles
        ? describe(error)
        : 'Wayline could not load its page files (' +
          describe(error) +
          '). Reload the page. If this persists, reinstall Wayline.';
    host.$el.replaceChildren(message);
  }
}

export default {
  name: 'Wayline',
  props: { api: Object, preview: Boolean, source: Object },
  render(this: Host, create: CreateElement) {
    return create('div', { class: 'wl-host' });
  },
  mounted(this: Host) {
    this.waylineGone = false;
    void draw(this);
  },
  beforeDestroy(this: Host) {
    this.waylineGone = true;
    this.waylineUnmount?.();
    this.waylineUnmount = null;
  },
};
