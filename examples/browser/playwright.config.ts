import { defineConfig } from '@playwright/test'
import { computeCertificateSpkiBase64, ensureFakeVideoCaptureFile, ensureLinuxEnvironment } from './playwright.helpers'

ensureLinuxEnvironment()

const certificateSpki = computeCertificateSpkiBase64()
const fakeVideoPath = ensureFakeVideoCaptureFile()
const baseURL = process.env.MEDIA_E2E_BASE_URL ?? 'http://127.0.0.1:4173'
const forceQuicOrigins = new Set(
  [
    process.env.MEETING_E2E_RELAY_A_URL ?? 'https://127.0.0.1:4433',
    process.env.MEETING_E2E_RELAY_B_URL ?? 'https://127.0.0.1:4434',
    process.env.MEDIA_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
  ].map((url) => new URL(url).host)
)

export default defineConfig({
  testDir: './tests',
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
        `--origin-to-force-quic-on=${[...forceQuicOrigins].join(',')}`,
        `--ignore-certificate-errors-spki-list=${certificateSpki}`,
        // Fallback for environments where SPKI pinning alone is insufficient.
        '--ignore-certificate-errors',
        // getDisplayMedia captures the calling tab, at its viewport size, instead of a fake monitor.
        '--use-fake-device-for-media-stream=display-media-type=browser',
        '--use-file-for-fake-video-capture=' + fakeVideoPath,
        '--use-fake-ui-for-media-stream',
        '--autoplay-policy=no-user-gesture-required',
        // A runner that needs speech on the fake microphone points this at a WAV file.
        ...(process.env.E2E_FAKE_AUDIO_FILE
          ? [`--use-file-for-fake-audio-capture=${process.env.E2E_FAKE_AUDIO_FILE}`]
          : [])
      ]
    },
    trace: 'retain-on-failure',
    video: 'retain-on-failure'
  }
})
