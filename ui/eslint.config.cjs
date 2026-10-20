const vue = require('eslint-plugin-vue');
module.exports = [
  ...vue.configs['flat/vue2-essential'],
  {
    files: ['**/*.js', '**/*.cjs', '**/*.vue'],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: 'module',
      globals: { window: 'readonly', document: 'readonly', setTimeout: 'readonly', clearTimeout: 'readonly', ResizeObserver: 'readonly', module: 'readonly', require: 'readonly', __dirname: 'readonly' },
    },
    rules: {
      'no-unused-vars': 'error',
      'no-eval': 'error',
      'no-implied-eval': 'error',
      'no-new-func': 'error',
      'no-debugger': 'error',
      'no-constant-condition': 'error',
      'vue/no-v-html': 'error',
    },
  },
];
