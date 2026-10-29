import { type Locator, type Page, expect } from '@playwright/test'
import { PTZ_TRACKING_PATH } from '../playwright.helpers'

const moqtUrl = process.env.PTZ_TRACKING_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const botUrl = process.env.PTZ_TRACKING_E2E_BOT_URL!
const cameraOrigin = process.env.PTZ_TRACKING_E2E_CAMERA_ORIGIN!
export const SYSTEM_ONE_ORIGIN = process.env.PTZ_TRACKING_E2E_SYSTEM_ONE_ORIGIN!

export type PtzCommand = { type: string }

/// [left, top, right, bottom] as fractions of the picture from its top-left corner.
export type Box = [number, number, number, number]

/// The boxes the fake System One server finds for each target; a target left out has none.
export async function placeBoxes(boxes: Record<string, Box[]>): Promise<void> {
  const response = await fetch(`${SYSTEM_ONE_ORIGIN}/boxes`, { method: 'PUT', body: JSON.stringify(boxes) })
  expect(response.ok).toBe(true)
}

/// Every PTZ command the fake onvif-ingest has received since it started.
export async function receivedCommands(): Promise<PtzCommand[]> {
  const response = await fetch(`${cameraOrigin}/commands`)
  expect(response.ok).toBe(true)
  return response.json()
}

export async function joinCamera(page: Page): Promise<Locator> {
  await page.goto(`${PTZ_TRACKING_PATH}?${new URLSearchParams({ moqtUrl, botUrl })}`)
  await page.getByTestId('ptz-tracking-join-button').click()
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
