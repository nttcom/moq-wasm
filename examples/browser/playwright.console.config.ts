import { defineConfig } from '@playwright/test'

export default defineConfig({
  testDir: './tests',
  testMatch: /console-e2e\.spec\.ts$/,
  outputDir: './test-results/console',
  workers: 1,
  timeout: 60_000,
  reporter: 'list',
  use: {
    baseURL: process.env.CONSOLE_E2E_BASE_URL,
    browserName: 'chromium',
    headless: process.env.PLAYWRIGHT_HEADLESS !== 'false',
    trace: 'retain-on-failure'
  }
})
