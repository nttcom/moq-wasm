import { expect, test } from '@playwright/test'
import { delaySystemOneAnswers } from './fake-system-one'
import { leaveAllPages } from './leave-all-pages'
import {
  SYSTEM_ONE_ORIGIN,
  type PtzCommand,
  joinCamera,
  placeBoxes,
  receivedCommands,
  trackTargets
} from './ptz-tracking-e2e-arrange'

const LATE_ANSWER_MS = 6_000
// A pan of 0.5 for (0.9 - 0.5) / 0.27 s, the bot's default calibration.
const PAN_RIGHT_TEXT = 'パン +1.48 秒・チルト 0.00 秒'
const PAN_LEFT_TEXT = 'パン -1.48 秒・チルト 0.00 秒'

test.describe('MoQ PTZ Tracking', () => {
  test.afterEach(async ({ browser }) => {
    await leaveAllPages(browser, 'ptz-tracking-leave-button')
    await delaySystemOneAnswers(SYSTEM_ONE_ORIGIN, 0)
  })

  test('pans the camera toward a box past the right margin', async ({ page }) => {
    // Arrange
    await placeBoxes({ 人: [[0.8, 0.4, 1.0, 0.6]] })
    const position = await joinCamera(page)
    const alreadyReceived = (await receivedCommands()).length

    // Act
    await trackTargets(page, ['人'])

    // Assert: onvif-ingest receives ContinuousMove then Stop
    await expect(position).toHaveText('人 1', { timeout: 30_000 })
    await expect(page.getByTestId('ptz-tracking-move')).toHaveText(PAN_RIGHT_TEXT)
    await expect
      .poll(async () => stepCommands((await receivedCommands()).slice(alreadyReceived)), { timeout: 15_000 })
      .toEqual([{ type: 'continuous', pan: 0.5, tilt: 0, zoom: 0, speed: 1 }, { type: 'stop' }])
  })

  test('turns the other way once the box moves past the left margin', async ({ page }) => {
    // Arrange
    await placeBoxes({ 人: [[0.8, 0.4, 1.0, 0.6]] })
    await joinCamera(page)
    await trackTargets(page, ['人'])
    const move = page.getByTestId('ptz-tracking-move')
    await expect(move).toHaveText(PAN_RIGHT_TEXT, { timeout: 30_000 })

    // Act
    await placeBoxes({ 人: [[0.0, 0.4, 0.2, 0.6]] })

    // Assert
    await expect(move).toHaveText(PAN_LEFT_TEXT, { timeout: 30_000 })
  })

  test('draws every target box and keeps the camera still while they all fit', async ({ page }) => {
    // Arrange
    await placeBoxes({
      人: [
        [0.15, 0.2, 0.35, 0.8],
        [0.4, 0.2, 0.55, 0.8]
      ],
      椅子: [[0.6, 0.5, 0.85, 0.85]]
    })
    const position = await joinCamera(page)

    // Act
    await trackTargets(page, ['人', '椅子'])

    // Assert
    await expect(position).toHaveText('人 2・椅子 1', { timeout: 30_000 })
    await expect(page.getByTestId('ptz-tracking-box')).toHaveCount(3)
    await expect(page.getByTestId('ptz-tracking-box').last()).toHaveText('椅子')
    await expect(page.getByTestId('ptz-tracking-move')).toHaveText('')
  })

  test('reports targets the detector does not see', async ({ page }) => {
    // Arrange
    await placeBoxes({})
    const position = await joinCamera(page)

    // Act
    await trackTargets(page, ['人'])

    // Assert
    await expect(position).toHaveText('見つかりません', { timeout: 30_000 })
    await expect(page.getByTestId('ptz-tracking-box')).toHaveCount(0)
  })

  test('shows the System One status while its answers are late', async ({ page }) => {
    // Arrange
    await placeBoxes({ 人: [[0.4, 0.4, 0.6, 0.6]] })
    await delaySystemOneAnswers(SYSTEM_ONE_ORIGIN, LATE_ANSWER_MS)
    const position = await joinCamera(page)

    // Act
    await trackTargets(page, ['人'])

    // Assert
    const systemOneStatus = page.getByTestId('ptz-tracking-system-one-status')
    await expect(systemOneStatus).toContainText('d1-2B', { timeout: 30_000 })
    await delaySystemOneAnswers(SYSTEM_ONE_ORIGIN, 0)
    await expect(position).toHaveText('人 1', { timeout: 30_000 })
    await expect(systemOneStatus).toBeHidden()
  })
})

/// The first ContinuousMove and the command after it; a Stop that ends an earlier sweep may precede them.
function stepCommands(commands: PtzCommand[]): PtzCommand[] {
  const start = commands.findIndex((command) => command.type === 'continuous')
  return start < 0 ? [] : commands.slice(start, start + 2)
}
