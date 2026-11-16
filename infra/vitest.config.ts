import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    include: ['test/**/*.test.ts'],
    // Synthesizing the stack is CPU heavy, so give each test room.
    testTimeout: 60_000,
  },
});
