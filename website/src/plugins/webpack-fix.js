// Fix webpack ProgressPlugin validation in webpack ≥5.100
// Docusaurus passes percentBy: null which newer webpack rejects.

const webpack = require('webpack');

module.exports = function () {
  return {
    name: 'hoard-webpack-fix',
    configureWebpack(config) {
      // Replace broken ProgressPlugin with a working one
      const plugins = (config.plugins || []).map((p) => {
        if (p instanceof webpack.ProgressPlugin) {
          return new webpack.ProgressPlugin({ percentBy: 'entries' });
        }
        return p;
      });
      return { plugins };
    },
  };
};
