import { type Locator, type Page, expect } from '@playwright/test'
import { PTZ_TRACKING_PATH } from '../playwright.helpers'
import { pressStartBot, roomBotE2E } from './room-bot'

const roomBot = roomBotE2E('PTZ_TRACKING')
const cameraOrigin = process.env.PTZ_TRACKING_E2E_CAMERA_ORIGIN!
export const SYSTEM_ONE_ORIGIN = roomBot.systemOneOrigin
/// Box corners are fractions of the picture from its top-left corner.
export const placeBoxes = roomBot.placeBoxes

export type PtzCommand = { type: string }

/// Every PTZ command the fake onvif-ingest has received since it started.
export async function receivedCommands(): Promise<PtzCommand[]> {
  const response = await fetch(`${cameraOrigin}/commands`)
  expect(response.ok).toBe(true)
  return response.json()
}

export async function joinCamera(page: Page): Promise<Locator> {
  await roomBot.clickJoin(page, PTZ_TRACKING_PATH, 'ptz-tracking')
  await pressStartBot(page, 'ptz-tracking')
  await expect(page.locator('#tracker-status')).toHaveAttribute('data-state', 'ok', { timeout: 60_000 })
  return page.getByTestId('ptz-tracking-position')
}

export async function trackTargets(page: Page, targets: string[]): Promise<void> {
  for (const [index, target] of targets.entries()) {
    if (index > 0) {
      await page.getByTestId('ptz-tracking-add-target-button').click()
    }
    await page.getByTestId('ptz-tracking-target-input').nth(index).fill(target)
  }
  await page.getByTestId('ptz-tracking-start-button').click()
}
