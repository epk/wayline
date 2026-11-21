import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { runInNewContext } from 'node:vm';

const source = readFileSync(new URL('../../xtask/src/package.rs', import.meta.url), 'utf8');
const constant = (name) => {
  const raw = new RegExp(`const ${name}: &str = r"([\\s\\S]*?)";\\n`).exec(source);
  if (raw) return raw[1];
  const plain = new RegExp(`const ${name}: &str =\\s*"((?:[^"\\\\]|\\\\.)*)";`).exec(source);
  assert.ok(plain, `${name} is missing from xtask/src/package.rs`);
  return JSON.parse(`"${plain[1]}"`);
};

test('the wrapped bundle passes the package loader check', () => {
  const bundle = readFileSync(new URL('../dist/gl-sdk4-ui-wayline.common.js', import.meta.url));
  const dir = mkdtempSync(join(tmpdir(), 'wayline-loader-'));
  try {
    const file = join(dir, 'bundle.js');
    writeFileSync(file, constant('LOADER_PREFIX') + bundle + constant('LOADER_SUFFIX'));
    const out = execFileSync(process.execPath, ['-e', constant('LOADER_CHECK'), file], {
      encoding: 'utf8',
      timeout: 20000,
    });
    assert.equal(out.trim(), 'ok');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('the bundle is a Vue 2 component shape without the template compiler', () => {
  const bundle = readFileSync(
    new URL('../dist/gl-sdk4-ui-wayline.common.js', import.meta.url),
    'utf8',
  );
  const module = { exports: {} };
  const required = [];
  runInNewContext(bundle, {
    module,
    exports: module.exports,
    require: (name) => {
      required.push(name);
      return {};
    },
  });
  const component = module.exports.default || module.exports;
  assert.equal(component.name, 'Wayline');
  assert.equal(typeof component.render, 'function');
  assert.equal(typeof component.mounted, 'function');
  assert.equal(typeof component.beforeDestroy, 'function');
  assert.deepEqual(Object.keys(component.props).sort(), ['api', 'preview', 'source']);
  assert.deepEqual(required, []);
  assert.doesNotMatch(bundle, /fonts\.googleapis|https?:\/\/cdn/);
});
