const path = require('path');
const { VueLoaderPlugin } = require('vue-loader');
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
  plugins: [new VueLoaderPlugin()],
  resolve: { extensions: ['.js', '.vue'] },
  performance: { hints: 'error', maxAssetSize: 80000, maxEntrypointSize: 80000 },
};
