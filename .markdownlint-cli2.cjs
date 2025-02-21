'use strict';

const sharedStandards = require('@silvermine/standardization/.markdownlint-cli2.shared.cjs');

module.exports = {

   ...sharedStandards,

   ignores: [
      ...sharedStandards.ignores,
      './src-tauri/target/**/*',
      './src-tauri/gen/**/*',
   ],

};
