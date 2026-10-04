import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '.',
  testMatch: 'workspace.spec.mjs',
  timeout: 45_000,
  retries: 0,
  workers: 1,
  use: {
    browserName: 'chromium',
    headless: true,
    viewport: { width: 1024, height: 768 },
  },
});
