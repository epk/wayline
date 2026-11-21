import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { constants, gzipSync, zstdCompressSync } from 'node:zlib';
import { transform } from 'lightningcss';
import { build } from 'rolldown';

const root = fileURLToPath(new URL('.', import.meta.url));
const entry = root + 'dist/gl-sdk4-ui-wayline.common.js';
const chunkDir = root + 'views/wayline/';
const base = '/views/wayline/';
const budget = { total: 600 * 1000, firstLoad: 125 * 1000 };
const version = readFileSync(root + '../monitor/Cargo.toml', 'utf8').match(
  /^version = "([^"]+)"/m,
)[1];
const styles = '\0wayline-styles';
const targets = { chrome: 111 << 16, firefox: 113 << 16, safari: (16 << 16) | (4 << 8) };

const usedSlots = [
  'alert-dialog',
  'button',
  'checkbox',
  'input',
  'label',
  'segmented-control',
  'switch',
  'tabs',
  'toggle',
  'visually-hidden',
];

const blocks = (text) => {
  const out = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    if (text[i] === '{') depth++;
    else if (text[i] === '}' && --depth === 0) {
      out.push(text.slice(start, i + 1));
      start = i + 1;
    }
  }
  return out;
};

const dropped = new Set(['a', 'hr', 'code', 'kbd', ':is(a, summary):focus-visible']);

const usedSlot = (slot) => usedSlots.some((used) => slot === used || slot.startsWith(used + '-'));
const slotsUsed = (text) =>
  [...text.matchAll(/data-slot="([a-z-]+)"/g)].every((m) => usedSlot(m[1]));

const narrow = (selector) =>
  selector.replace(/:(is|where)\(([^()]*)\)/g, (all, kind, list) => {
    const kept = list.split(',').filter(slotsUsed);
    return kept.length ? ':' + kind + '(' + kept.join(',') + ')' : ':is(#wl-unused)';
  });

const keepSelector = (selector) =>
  !dropped.has(selector.trim()) &&
  !narrow(selector).includes('#wl-unused') &&
  slotsUsed(narrow(selector));

const splitList = (text) => {
  const parts = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    if (text[i] === '(') depth++;
    else if (text[i] === ')') depth--;
    else if (text[i] === ',' && depth === 0) {
      parts.push(text.slice(start, i));
      start = i + 1;
    }
  }
  parts.push(text.slice(start));
  return parts;
};

const prune = (text) =>
  blocks(text)
    .map((block) => {
      const open = block.indexOf('{');
      const head = block.slice(0, open);
      const body = block.slice(open + 1, -1);
      if (head.trim().startsWith('@')) return head + '{' + prune(body) + '}';
      const kept = splitList(head).filter(keepSelector).map(narrow);
      return kept.length ? kept.join(',') + '{' + body + '}' : '';
    })
    .join('\n');

const scopeWebCss = (code) => {
  const bare = code.replace(/\/\*[\s\S]*?\*\//g, '');
  const inner = bare.slice(bare.indexOf('{') + 1, bare.lastIndexOf('}'));
  return (
    '@layer ui-sdk { :where(.wl) {' + prune(inner.replace(':where(:root, :host)', '&')) + '} }'
  );
};

const styleCode = `
const sheets = [];
let element = null;
export const add = (css) => {
  sheets.push(css);
  if (typeof document === 'undefined' || !document.head) return;
  element ||= document.getElementById('wayline-style');
  if (!element) {
    element = document.createElement('style');
    element.id = 'wayline-style';
    document.head.appendChild(element);
  }
  element.textContent = sheets.join('\\n');
};
`;

const css = {
  name: 'wayline-css',
  resolveId(id) {
    return id === styles ? id : null;
  },
  load(id) {
    return id === styles ? styleCode : null;
  },
  transform(code, id) {
    if (!id.endsWith('.css')) return null;
    const source = id.endsWith('@ui-sdk/core/dist/web.css') ? scopeWebCss(code) : code;
    const result = transform({
      filename: id,
      code: Buffer.from(source),
      minify: true,
      targets,
      errorRecovery: false,
    });
    if (result.warnings.length) this.error(result.warnings.map((w) => w.message).join('\n'));
    return {
      code: `import { add } from ${JSON.stringify(styles)}; add(${JSON.stringify(result.code.toString())});`,
      moduleType: 'js',
    };
  },
};

const geoStub = {
  name: 'wayline-no-geoarrow',
  resolveId: (id) => (id === '@uwdata/wk-geojson' ? '\0wk-geojson' : null),
  load: (id) =>
    id === '\0wk-geojson'
      ? 'export const wkbToGeoJSON = () => { throw Error("GeoArrow is not supported."); }; export const wktToGeoJSON = wkbToGeoJSON;'
      : null,
};

const shared = {
  platform: 'browser',
  resolve: { conditionNames: ['browser', 'import', 'production', 'default'] },
  transform: {
    define: {
      WAYLINE_VERSION: JSON.stringify(version),
      'process.env.NODE_ENV': JSON.stringify('production'),
    },
    jsx: { runtime: 'automatic', importSource: 'react' },
    target: 'es2022',
  },
  logLevel: 'warn',
  onLog(level, log) {
    if (log.code === 'MODULE_LEVEL_DIRECTIVE' && log.message.includes('use client')) return;
    throw Error(`rolldown ${level}: ${log.message}`);
  },
};

const minify = { compress: { target: 'es2022', maxIterations: 10 }, mangle: { toplevel: true } };
const comments = { legal: false, annotation: false, jsdoc: false };

rmSync(chunkDir, { recursive: true, force: true });
mkdirSync(chunkDir, { recursive: true });

const input = {
  core: root + 'src/core.tsx',
  paths: root + 'src/views/paths.tsx',
  wifi: root + 'src/views/wifi.tsx',
  traffic: root + 'src/views/traffic.tsx',
  router: root + 'src/views/router.tsx',
  events: root + 'src/views/events.tsx',
  settings: root + 'src/settings.tsx',
};

const probe = await build({
  ...shared,
  input,
  plugins: [css, geoStub],
  write: false,
  output: { dir: chunkDir, format: 'esm' },
});
const probeChunks = probe.output.filter((item) => item.type === 'chunk');
const byFile = new Map(probeChunks.map((item) => [item.fileName, item]));
const coreModules = new Set();
const visit = (file) => {
  const item = byFile.get(file);
  if (!item || item.seen) return;
  item.seen = true;
  item.moduleIds.forEach((id) => coreModules.add(id));
  item.imports.forEach(visit);
};
visit(probeChunks.find((item) => item.isEntry && item.name === 'core').fileName);
const chartModule = /[\\/](uplot[\\/]|chart\.tsx|uplot\.css)/;

const chunks = await build({
  ...shared,
  preserveEntrySignatures: 'allow-extension',
  input: {
    core: root + 'src/core.tsx',
    paths: root + 'src/views/paths.tsx',
    wifi: root + 'src/views/wifi.tsx',
    traffic: root + 'src/views/traffic.tsx',
    router: root + 'src/views/router.tsx',
    events: root + 'src/views/events.tsx',
    settings: root + 'src/settings.tsx',
  },
  plugins: [css, geoStub],
  output: {
    dir: chunkDir,
    format: 'esm',
    entryFileNames: '[name]-[hash].js',
    chunkFileNames: '[name]-[hash].js',
    codeSplitting: {
      groups: [
        {
          name: 'chart',
          test: (id) => chartModule.test(id),
          priority: 3,
          includeDependenciesRecursively: false,
        },
        { name: 'core', test: (id) => coreModules.has(id), priority: 2 },
      ],
    },
    minify,
    comments,
  },
});

const files = chunks.output.filter((item) => item.type === 'chunk');
const entries = Object.fromEntries(
  files.filter((item) => item.isEntry).map((item) => [item.name, item.fileName]),
);
const deps = Object.fromEntries(
  files.map((item) => [item.fileName, [...new Set([...item.imports, ...item.dynamicImports])]]),
);
const sizes = {};
for (const item of files) {
  const code = Buffer.from(item.code);
  for (const dep of deps[item.fileName])
    if (!['"', "'", '`'].some((q) => code.includes(q + './' + dep + q)))
      throw Error(`${item.fileName} refers to ${dep} in a form the loader does not rewrite.`);
  const zstd = zstdCompressSync(code, { params: { [constants.ZSTD_c_compressionLevel]: 19 } });
  writeFileSync(chunkDir + item.fileName + '.zst', zstd);
  sizes[item.fileName] = {
    minified: code.length,
    zstd: zstd.length,
    gzip: gzipSync(code, { level: 9 }).length,
  };
}

const { core, ...lazy } = entries;
const manifest = { base, core, entries: lazy, deps };
const manifestId = '\0wayline-manifest';

await build({
  ...shared,
  input: root + 'src/glinet.ts',
  plugins: [
    {
      name: 'wayline-manifest',
      resolveId: (id) => (id === manifestId ? id : null),
      load: (id) =>
        id === manifestId ? `export const manifest = ${JSON.stringify(manifest)};` : null,
    },
  ],
  output: {
    file: entry,
    format: 'cjs',
    exports: 'default',
    codeSplitting: false,
    minify,
    comments,
  },
});

await build({
  ...shared,
  input: root + 'preview/demo.js',
  plugins: [geoStub],
  output: {
    file: root + 'preview/dist/demo.js',
    format: 'iife',
    codeSplitting: false,
    minify: true,
    comments,
  },
});

const loaderCode = readFileSync(entry);
const loader = {
  minified: loaderCode.length,
  zstd: zstdCompressSync(loaderCode, { params: { [constants.ZSTD_c_compressionLevel]: 19 } })
    .length,
  gzip: gzipSync(loaderCode, { level: 9 }).length,
};
const closure = (file, seen = new Set()) => {
  if (seen.has(file)) return seen;
  seen.add(file);
  deps[file].forEach((dep) => closure(dep, seen));
  return seen;
};
const sum = (list, key) => list.reduce((total, file) => total + sizes[file][key], 0);
const first = [...closure(core)];
const all = Object.keys(sizes);
const report = {
  version,
  loader,
  firstLoad: {
    files: first,
    minified: loader.minified + sum(first, 'minified'),
    zstd: loader.gzip + sum(first, 'zstd'),
    gzip: loader.gzip + sum(first, 'gzip'),
  },
  total: {
    minified: loader.minified + sum(all, 'minified'),
    zstd: loader.gzip + sum(all, 'zstd'),
    gzip: loader.gzip + sum(all, 'gzip'),
  },
  chunks: sizes,
  entries,
};
writeFileSync(root + 'dist/size-report.json', JSON.stringify(report, null, 2) + '\n');

const failures = [];
if (report.total.minified > budget.total)
  failures.push(`all chunks are ${report.total.minified} bytes minified (budget ${budget.total})`);
if (report.firstLoad.zstd > budget.firstLoad)
  failures.push(
    `the first load is ${report.firstLoad.zstd} bytes (budget ${budget.firstLoad}; loader gzip, chunks zstd)`,
  );
if (failures.length) {
  console.error('The UI is over budget: ' + failures.join('; ') + '.');
  process.exit(1);
}
console.log(
  `Built Wayline ${version}: loader ${loader.minified} B, first load ${report.firstLoad.zstd} B zstd ` +
    `(${report.firstLoad.gzip} B as gzip), ${all.length} chunks ${report.total.minified} B minified.`,
);
