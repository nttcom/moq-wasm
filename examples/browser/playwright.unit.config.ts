import { defineConfig } from '@playwright/test'

export default defineConfig({
  testDir: '.',
  testIgnore: ['tests/**'],
  testMatch: /\.test\.ts$/,
  outputDir: './test-results/unit',
  reporter: 'list'
})
