const path = require('path');
const CopyWebpackPlugin = require("copy-webpack-plugin");
module.exports = {
  entry: "./bootstrap.js",
  output: {
    path: path.resolve(__dirname, "dist"),
    filename: "bootstrap.js",
  },
  mode: "development",
  plugins: [
    new CopyWebpackPlugin({ patterns: ['index.html'] })
  ],
  experiments: {
    asyncWebAssembly: true
  }
}
