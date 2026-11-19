import { type Locator, type Page, expect } from '@playwright/test'
import { CAMERA_DETECTION_PATH } from '../playwright.helpers'
import { pressStartBot, roomBotE2E } from './room-bot'

const roomBot = roomBotE2E('CAMERA_DETECTION')
/// The bot runs one room at a time, so every test shares this room.
const ROOM = 'e2e'
export const SYSTEM_ONE_ORIGIN = roomBot.systemOneOrigin
export const placeBoxes = roomBot.placeBoxes

export async function answerDetectorWith(choice: string): Promise<void> {
  const response = await fetch(`${SYSTEM_ONE_ORIGIN}/answer`, { method: 'PUT', body: choice })
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
  await roomBot.clickJoin(page, CAMERA_DETECTION_PATH, 'camera-detection', room)
  await pressStartBot(page, 'camera-detection')
}
