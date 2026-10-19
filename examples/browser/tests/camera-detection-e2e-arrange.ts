import { type Locator, type Page, expect } from '@playwright/test'
import { CAMERA_DETECTION_PATH } from '../playwright.helpers'

const moqtUrl = process.env.CAMERA_DETECTION_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const botUrl = process.env.CAMERA_DETECTION_E2E_BOT_URL!
/// The bot runs one room at a time, so every test shares this room.
const ROOM = 'e2e'
export const D3_FLASH_ORIGIN = process.env.CAMERA_DETECTION_E2E_D3_FLASH_ORIGIN!

export async function answerDetectorWith(choice: string): Promise<void> {
  const response = await fetch(`${D3_FLASH_ORIGIN}/answer`, { method: 'PUT', body: choice })
  expect(response.ok).toBe(true)
}

export async function askCamera(page: Page, question: string, choices: string): Promise<void> {
  await page.getByTestId('camera-detection-question-input').fill(question)
  await page.getByTestId('camera-detection-choices-input').fill(choices)
  await page.getByTestId('camera-detection-apply-button').click()
}

export async function joinCamera(page: Page): Promise<Locator> {
  await clickJoinCamera(page)
  await expect(page.locator('#detector-status')).toHaveAttribute('data-state', 'ok', { timeout: 60_000 })
  return page.getByTestId('camera-detection-verdict')
}

export async function clickJoinCamera(page: Page, room = ROOM): Promise<void> {
  await page.goto(`${CAMERA_DETECTION_PATH}?${new URLSearchParams({ moqtUrl, botUrl, room })}`)
  await page.getByTestId('camera-detection-join-button').click()
}
