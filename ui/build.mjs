import { readFileSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { gzipSync } from 'node:zlib';
import { transform } from 'lightningcss';
import { build } from 'rolldown';

const root = fileURLToPath(new URL('.', import.meta.url));
const output = root + 'dist/gl-sdk4-ui-wayline.common.js';
const budget = { minified: 600 * 1000, gzip: 200 * 1000 };
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

const css = {
  name: 'wayline-css',
  resolveId(id) {
    return id === styles ? id : null;
  },
  load(id) {
    if (id !== styles) return null;
    return 'export const sheets = []; export const add = (css) => { sheets.push(css); };';
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

await build({
  ...shared,
  input: root + 'src/glinet.ts',
  plugins: [css],
  output: {
    file: output,
    format: 'cjs',
    exports: 'default',
    codeSplitting: false,
    minify: { compress: { target: 'es2022', maxIterations: 10 }, mangle: { toplevel: true } },
    comments: { legal: false, annotation: false, jsdoc: false },
  },
});

await build({
  ...shared,
  input: root + 'preview/demo.js',
  output: {
    file: root + 'preview/dist/demo.js',
    format: 'iife',
    codeSplitting: false,
    minify: true,
    comments: { legal: false, annotation: false, jsdoc: false },
  },
});

const bytes = readFileSync(output);
const size = statSync(output).size;
const gzip = gzipSync(bytes, { level: 9 }).length;
const report = `${size} bytes minified (budget ${budget.minified}), ${gzip} bytes gzip (budget ${budget.gzip})`;
if (size > budget.minified || gzip > budget.gzip) {
  console.error(`The bundle is over budget: ${report}.`);
  process.exit(1);
}
console.log(`Built ${output} for Wayline ${version}: ${report}.`);
