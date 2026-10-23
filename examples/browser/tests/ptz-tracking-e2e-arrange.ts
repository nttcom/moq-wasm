import { type Locator, type Page, expect } from '@playwright/test'
import { PTZ_TRACKING_PATH } from '../playwright.helpers'

const moqtUrl = process.env.PTZ_TRACKING_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const botUrl = process.env.PTZ_TRACKING_E2E_BOT_URL!
const cameraOrigin = process.env.PTZ_TRACKING_E2E_CAMERA_ORIGIN!
export const D3_FLASH_ORIGIN = process.env.PTZ_TRACKING_E2E_D3_FLASH_ORIGIN!

export type PtzCommand = { type: string }

/// Where the fake d3-flash sees the target, as fractions of the picture from its top-left corner.
export async function placeTargetAt(x: number, y: number): Promise<void> {
  await answerDetectorWith(`${x},${y}`)
}

export async function hideTarget(): Promise<void> {
  await answerDetectorWith('not visible')
}

async function answerDetectorWith(position: string): Promise<void> {
  const response = await fetch(`${D3_FLASH_ORIGIN}/position`, { method: 'PUT', body: position })
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

export async function trackTarget(page: Page, target: string): Promise<void> {
  await page.getByTestId('ptz-tracking-target-input').fill(target)
  await page.getByTestId('ptz-tracking-start-button').click()
}
