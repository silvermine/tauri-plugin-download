const config = require('@silvermine/eslint-config'),
      node = require('@silvermine/eslint-config/partials/node');

module.exports = [
   {
      ignores: [
         'dist-js/**',
         'src-tauri/**',
         'examples/tauri-app/src-tauri/**',
         'android/**/build/**',
      ],
   },
   ...config,
   { ...node },
   {
      files: [ '**/*.vue' ],
      rules: {
         // We have to disable the no-deprecated-slot-attribute rule for this project
         // because there is an unfortunate naming collision between Vue's deprecated
         // 'slot' prop (https://vuejs.org/guide/components/slots.html) and the
         // 'slot' attribute that Ionic uses on many of its components. At its lowest
         // level, Ionic components are actually Web Components.
         //
         // Web Components use a 'slot' attribute to allow parent components to define
         // content that appears in specified locations within the child component:
         // https://developer.mozilla.org/en-US/docs/Web/API/Web_components/Using_templates_and_slots#adding_flexibility_with_slots
         'vue/no-deprecated-slot-attribute': 'off',
         // Ionic provides methods on objects (like Ion-Nav) that are passed as props
         // to child components. Methods like `.pop()` and `.push()` interact with the
         // `nav` object, by invoking navigation actions which alters the UI but not the
         // prop's data. The `vue/no-mutating-props` rule flags these usages as errors.
         // Disabling this rule allows us to properly use Ionic's navigation API without
         // false positives.
         'vue/no-mutating-props':
         [
            'error',
            {
               'shallowOnly': true,
            },
         ],
      },
   },
   {
      files: [ '**/*.vue', '**/*.ts' ],
      rules: {
         // During prototype development we will allow console logging, except for "log"
         'no-console': 'off',
         'no-restricted-properties': [
            2,
            {
               'object': 'console',
               'property': 'log',
            },
         ],
      },
   },
];
