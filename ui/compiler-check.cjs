const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const path = require('node:path');
const { pathToFileURL } = require('node:url');

const builds = [
  'compiler-sfc/index.js',
  'packages/compiler-sfc/dist/compiler-sfc.js',
  'dist/vue.common.dev.js',
  'dist/vue.common.prod.js',
  'dist/vue.js',
  'dist/vue.min.js',
  'dist/vue.esm.js',
  'dist/vue.esm.browser.js',
  'dist/vue.esm.browser.min.js',
];

async function check(build) {
  const root = path.dirname(require.resolve('vue/package.json'));
  const file = path.join(root, build);
  const compiler = build.includes('.esm.')
    ? (await import(pathToFileURL(file).href)).default
    : require(file);
  if (compiler.parseComponent) {
    for (const tag of ['script', 'style', 'textarea']) {
      const body = 'İ < value';
      const parsed = compiler.parseComponent(`<${tag}>${body}</${tag.toUpperCase()}>`);
      if (tag === 'script') assert.equal(parsed.script.content, body);
      if (tag === 'style') assert.equal(parsed.styles[0].content, body);
      for (const body of ['<'.repeat(1_000_000), `</${tag}`.repeat(100_000)]) {
        compiler.parseComponent(`<${tag}>${body}</different>`);
        compiler.compileTemplate({ source: `<${tag}>${body}`, filename: 'security.vue' });
      }
    }
    const body = '\nİ < value';
    const source = `<template><div><textarea>${body}</TeXtArEa><p>{{ value }}</p></div></template>`;
    const result = compiler.parseComponent(source);
    assert.equal(
      result.template.content,
      `<div><textarea>${body}</TeXtArEa><p>{{ value }}</p></div>`,
    );
    const compiled = compiler.compileTemplate({
      source: result.template.content,
      filename: 'valid.vue',
    });
    assert.deepEqual(compiled.errors, []);
    assert.match(compiled.code, /textarea/);
    assert.match(compiled.code, /İ < value/);
  } else {
    for (const tag of ['script', 'style']) {
      for (const body of ['<'.repeat(1_000_000), `</${tag}`.repeat(100_000)]) {
        const result = compiler.compile(`<${tag}>${body}</different>`, { warn() {} });
        assert.equal(typeof result.render, 'function');
        assert.equal(typeof compiler.compile(`<${tag}>${body}`, { warn() {} }).render, 'function');
      }
    }
    const result = compiler.compile('<div><script>x</SCRIPT><style>x</STYLE></div>', { warn() {} });
    assert.equal(typeof result.render, 'function');
  }
}

if (process.argv[2]) {
  check(process.argv[2]).catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
} else {
  for (const build of builds) {
    const result = spawnSync(process.execPath, [__filename, build], {
      encoding: 'utf8',
      timeout: 5000,
      maxBuffer: 65536,
    });
    assert.ifError(result.error);
    assert.equal(result.status, 0, `${build}: ${result.stderr}`);
  }
  console.log('Vue compiler malformed-input and compatibility checks passed.');
}
