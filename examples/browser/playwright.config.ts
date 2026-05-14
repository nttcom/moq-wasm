import { defineConfig } from '@playwright/test'
import { computeCertificateSpkiBase64, ensureLinuxEnvironment } from './playwright.helpers'

ensureLinuxEnvironment()

const certificateSpki = computeCertificateSpkiBase64()
const baseURL = process.env.MEDIA_E2E_BASE_URL ?? 'http://127.0.0.1:4173'

export default defineConfig({
  testDir: './tests',
  testMatch: /media-e2e\.spec\.ts/,
  fullyParallel: false,
  workers: 1,
  timeout: 120_000,
  expect: {
    timeout: 15_000
  },
  reporter: 'list',
  use: {
    baseURL,
    browserName: 'chromium',
    headless: process.env.PLAYWRIGHT_HEADLESS !== 'false',
    ignoreHTTPSErrors: true,
    permissions: ['camera', 'microphone'],
    launchOptions: {
      args: [
        '--origin-to-force-quic-on=127.0.0.1:4433',
        `--ignore-certificate-errors-spki-list=${certificateSpki}`,
        '--use-fake-device-for-media-stream',
        '--use-fake-ui-for-media-stream',
        '--autoplay-policy=no-user-gesture-required'
      ]
    },
    trace: 'retain-on-failure',
    video: 'retain-on-failure'
  }
})
