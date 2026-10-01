import { type Locator, type Page, expect } from '@playwright/test'
import { CAMERA_DETECTION_PATH } from '../playwright.helpers'

const moqtUrl = process.env.CAMERA_DETECTION_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const answerUrl = process.env.CAMERA_DETECTION_E2E_ANSWER_URL!

export async function answerDetectorWith(answer: string): Promise<void> {
  const response = await fetch(answerUrl, { method: 'PUT', body: answer })
  expect(response.ok).toBe(true)
}

export async function joinCamera(page: Page): Promise<Locator> {
  await page.goto(`${CAMERA_DETECTION_PATH}?${new URLSearchParams({ moqtUrl })}`)
  await page.getByTestId('camera-detection-join-button').click()
  await expect(page.locator('#detector-status')).toHaveAttribute('data-state', 'ok', { timeout: 60_000 })
  return page.getByTestId('camera-detection-verdict')
}
