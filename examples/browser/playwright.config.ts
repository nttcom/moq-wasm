import { defineConfig } from '@playwright/test'
import { computeCertificateSpkiBase64, ensureFakeVideoCaptureFile, ensureLinuxEnvironment } from './playwright.helpers'

ensureLinuxEnvironment()

const certificateSpki = computeCertificateSpkiBase64()
const fakeVideoPath = ensureFakeVideoCaptureFile()
const baseURL = process.env.MEDIA_E2E_BASE_URL ?? 'http://127.0.0.1:4173'
const forceQuicOrigins = [
  process.env.CALL_E2E_RELAY_A_URL ?? 'https://127.0.0.1:4433',
  process.env.CALL_E2E_RELAY_B_URL ?? 'https://127.0.0.1:4434',
  process.env.MEDIA_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
]
  .map(toHttp3Authority)
  .filter((origin, index, origins) => origins.indexOf(origin) === index)

export default defineConfig({
  testDir: './tests',
  testMatch: /(media-e2e|call-e2e)\.spec\.ts/,
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
        // Force QUIC on the relay origins used by the E2E runners.
        `--origin-to-force-quic-on=${forceQuicOrigins.join(',')}`,
        `--ignore-certificate-errors-spki-list=${certificateSpki}`,
        // Fallback for environments where SPKI pinning alone is insufficient.
        '--ignore-certificate-errors',
        '--use-fake-device-for-media-stream',
        '--use-file-for-fake-video-capture=' + fakeVideoPath,
        '--use-fake-ui-for-media-stream',
        '--autoplay-policy=no-user-gesture-required'
      ]
    },
    trace: 'retain-on-failure',
    video: 'retain-on-failure'
  }
})

function toHttp3Authority(url: string): string {
  return new URL(url).host
}
