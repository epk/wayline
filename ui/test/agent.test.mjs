import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { after, before, test } from 'node:test';
import { pathToFileURL } from 'node:url';
import { JSDOM, VirtualConsole } from 'jsdom';
import { build } from 'rolldown';

const root = new URL('..', import.meta.url).pathname;
const version = readFileSync(root + '../monitor/Cargo.toml', 'utf8').match(
  /^version = "([^"]+)"/m,
)[1];
const dir = mkdtempSync(join(tmpdir(), 'wayline-agent-'));
const bundle = join(dir, 'agent-app.mjs');
let approve = false;
let page;
let dom;

const until = async (check, what) => {
  for (let i = 0; i < 200; i++) {
    const value = check();
    if (value) return value;
    await page.act(() => new Promise((resolve) => setTimeout(resolve, 25)));
  }
  throw Error('Timed out waiting for ' + what);
};

const entries = () =>
  page.agent.snapshot().filter((entry) => !entry.offPage && entry.component !== 'more');

const run = async (steps) => {
  let result;
  await page.act(async () => {
    result = await page.agent.act({ steps });
  });
  return result;
};

before(async () => {
  await build({
    input: root + 'test/agent-app.tsx',
    platform: 'browser',
    resolve: { conditionNames: ['browser', 'import', 'development', 'default'] },
    transform: {
      define: {
        WAYLINE_VERSION: JSON.stringify(version),
        'process.env.NODE_ENV': JSON.stringify('development'),
      },
      jsx: { runtime: 'automatic', importSource: 'react' },
    },
    plugins: [
      {
        name: 'no-css',
        transform: (code, id) => (id.endsWith('.css') ? { code: '', moduleType: 'js' } : null),
      },
      {
        name: 'no-geoarrow',
        resolveId: (id) => (id === '@uwdata/wk-geojson' ? '\0wk' : null),
        load: (id) =>
          id === '\0wk'
            ? 'export const wkbToGeoJSON = () => {}; export const wktToGeoJSON = () => {};'
            : null,
      },
    ],
    onLog() {},
    output: { file: bundle, format: 'esm', codeSplitting: false },
  });

  const quiet = new VirtualConsole();
  quiet.on('error', (e) => {
    if (!String(e).includes('Not implemented')) console.error(e);
  });
  dom = new JSDOM('<!doctype html><html><head></head><body><div id="root"></div></body></html>', {
    url: 'http://127.0.0.1:8787/preview/index.html',
    pretendToBeVisual: true,
    virtualConsole: quiet,
  });
  const { window } = dom;
  for (const key of Object.getOwnPropertyNames(window))
    if (!(key in globalThis)) globalThis[key] = window[key];
  for (const key of [
    'window',
    'document',
    'navigator',
    'localStorage',
    'location',
    'Event',
    'CustomEvent',
    'EventTarget',
    'KeyboardEvent',
    'MouseEvent',
    'FocusEvent',
    'InputEvent',
    'UIEvent',
  ])
    Object.defineProperty(globalThis, key, {
      value: window[key],
      configurable: true,
      writable: true,
    });
  const Channel = globalThis.MessageChannel;
  globalThis.MessageChannel = window.MessageChannel = class {
    constructor() {
      const channel = new Channel();
      this.port1 = channel.port1;
      this.port2 = channel.port2;
      queueMicrotask(() => {
        this.port1.unref();
        this.port2.unref();
      });
    }
  };
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  globalThis.__UI_SDK_STRICT__ = false;
  const proto = window.HTMLElement.prototype;
  proto.showPopover = function () {};
  proto.hidePopover = function () {};
  window.Element.prototype.getAnimations = () => [];
  window.Element.prototype.scrollIntoView = function () {};
  window.Element.prototype.checkVisibility = () => true;
  window.Element.prototype.getClientRects = () => [];
  window.document.elementFromPoint = () => null;
  window.document.elementsFromPoint = () => [];
  globalThis.ResizeObserver = window.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  window.matchMedia = globalThis.matchMedia = () => ({
    matches: false,
    addEventListener() {},
    removeEventListener() {},
  });
  const dialog = window.HTMLDialogElement.prototype;
  dialog.showModal = function () {
    this.setAttribute('open', '');
  };
  dialog.close = function () {
    this.removeAttribute('open');
  };

  const { boot } = await import(pathToFileURL(bundle).href);
  page = await boot(window.document.getElementById('root'), (step) => !step.confirm || approve);
  await until(() => window.document.querySelector('.wl-cards'), 'the overview');
});

after(async () => {
  await page?.unmount();
  dom?.window.close();
  rmSync(dir, { recursive: true, force: true });
});

test('every control on the page has a name an agent can use', () => {
  const list = entries();
  for (const entry of list) assert.ok(entry.label, `${entry.component} ${entry.ref} has no name`);
  const labels = list.map((entry) => entry.label);
  for (const label of ['Views', 'History range', 'Settings', 'Earlier', 'Later', 'Now'])
    assert.ok(labels.includes(label), `${label} is missing from ${JSON.stringify(labels)}`);
  const tabs = list.find((entry) => entry.label === 'Views');
  assert.deepEqual(
    tabs.items.map((item) => item.label),
    ['Overview', 'Paths', 'Wi-Fi', 'Traffic', 'Router', 'Events', 'Health'],
  );
});

test('an agent changes the range and switches views, and the page follows', async () => {
  const result = await run([
    { ref: 'History range', action: 'setValue', args: ['6h'] },
    { ref: 'Views', action: 'setValue', args: ['Paths'] },
  ]);
  assert.equal(result.ok, true, result.error?.message);
  const checked = document.querySelector('[data-slot="segmented-control-item"][data-checked]');
  assert.equal(checked.textContent, '6h');
  assert.match(document.querySelector('.wl-window').textContent, /15-second buckets/);
  assert.equal(
    document.querySelector('[data-slot="tabs-tab"][data-selected]').textContent,
    'Paths',
  );
  await until(() => document.querySelector('.wl-exits'), 'the paths table');
  const labels = entries().map((entry) => entry.label);
  for (const label of ['Use Tunnel B as exit', 'Run Tunnel A', 'Destination', 'Band of Tunnel A'])
    assert.ok(labels.includes(label), `${label} is missing from ${JSON.stringify(labels)}`);
  await until(
    () => entries().some((entry) => entry.label === 'Show only Tunnel A path'),
    'the series table',
  );
  const exit = entries().find((entry) => entry.label === 'Use Tunnel B as exit');
  assert.match(exit.confirm, /Makes Tunnel B the VPN exit/);
});

test('an agent edits a setting in the settings dialog', async () => {
  let result = await run([{ ref: 'Settings', action: 'press' }]);
  assert.equal(result.ok, true, result.error?.message);
  await until(() => document.querySelector('dialog#wl-settings[open] form'), 'the settings dialog');
  await until(
    () => entries().some((entry) => entry.label === 'Router and interface telemetry'),
    'settings fields',
  );
  const labels = entries().map((entry) => entry.label);
  for (const label of [
    'Measure latency',
    'Measure every',
    'Primary destination',
    'Keep history',
    'Save settings',
  ])
    assert.ok(labels.includes(label), `${label} is missing from ${JSON.stringify(labels)}`);
  result = await run([
    { ref: 'Router and interface telemetry', action: 'setChecked', args: [false] },
    { ref: 'Measure every', action: 'setValue', args: ['15 s'] },
  ]);
  assert.equal(result.ok, true, result.error?.message);
  const box = [...document.querySelectorAll('.wl-check')].find((el) =>
    el.textContent.includes('Router and interface telemetry'),
  );
  assert.equal(box.querySelector('[data-slot="checkbox"]').getAttribute('aria-checked'), 'false');
  const save = entries().find((entry) => entry.label === 'Save settings');
  assert.match(save.confirm, /probe interval 5 → 15/);
  assert.match(save.confirm, /router telemetry on → off/);
});

test('purging history waits for a person', async () => {
  const purge = [
    { ref: 'Purge history', action: 'press' },
    { ref: 'Delete all history', action: 'press' },
  ];
  approve = false;
  let result = await run(purge);
  assert.equal(result.ok, false);
  assert.match(result.error.message, /Deletes all stored Wayline history/);
  assert.equal(document.querySelector('.wl-notice'), null);
  approve = true;
  result = await run(purge);
  assert.equal(result.ok, true, result.error?.message);
  await until(
    () => document.querySelector('.wl-notice')?.textContent.includes('Stored history was purged'),
    'the purge notice',
  );
});
