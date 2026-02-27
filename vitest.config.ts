import { defineConfig } from 'vitest/config';

export default defineConfig({
   test: {
      environment: 'jsdom',
      include: [ 'guest-js/**/*.test.ts' ],
      globals: true,
   },
});
