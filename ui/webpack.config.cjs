const path = require('path');
const { VueLoaderPlugin } = require('vue-loader');
const { DefinePlugin } = require('webpack');
const fs = require('fs');
const version = fs
  .readFileSync(path.resolve(__dirname, '../monitor/Cargo.toml'), 'utf8')
  .match(/^version = "([^"]+)"/m)[1];
module.exports = {
  entry: './src/boundary.js',
  output: {
    path: path.resolve(__dirname, 'dist'),
    filename: 'gl-sdk4-ui-wayline.common.js',
    libraryTarget: 'commonjs2',
    libraryExport: 'default',
  },
  externals: { vue: 'vue' },
  module: {
    rules: [
      { test: /\.vue$/, loader: 'vue-loader' },
      { test: /\.css$/, use: ['vue-style-loader', 'css-loader'] },
    ],
  },
  plugins: [new VueLoaderPlugin(), new DefinePlugin({ WAYLINE_VERSION: JSON.stringify(version) })],
  resolve: { extensions: ['.js', '.vue'] },
  performance: { hints: 'error', maxAssetSize: 150000, maxEntrypointSize: 150000 },
};
