import { defineConfig } from '@playwright/test'

export default defineConfig({
  testDir: './lib',
  testMatch: /\.test\.ts$/,
  outputDir: './test-results/unit',
  reporter: 'list'
})
