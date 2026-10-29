import { expect, test } from '@playwright/test'
import { delaySystemOneAnswers } from './fake-system-one'
import { leaveAllPages } from './leave-all-pages'
import {
  SYSTEM_ONE_ORIGIN,
  type PtzCommand,
  hideTarget,
  joinCamera,
  placeTargetAt,
  receivedCommands,
  trackTarget
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

  test('pans the camera toward a target on the right', async ({ page }) => {
    // Arrange
    await placeTargetAt(0.9, 0.5)
    const position = await joinCamera(page)
    const alreadyReceived = (await receivedCommands()).length

    // Act
    await trackTarget(page, '人')

    // Assert: onvif-ingest receives ContinuousMove then Stop
    await expect(position).toHaveText('横 90%・縦 50%', { timeout: 30_000 })
    await expect(page.getByTestId('ptz-tracking-move')).toHaveText(PAN_RIGHT_TEXT)
    await expect
      .poll(async () => stepCommands((await receivedCommands()).slice(alreadyReceived)), { timeout: 15_000 })
      .toEqual([{ type: 'continuous', pan: 0.5, tilt: 0, zoom: 0, speed: 1 }, { type: 'stop' }])
  })

  test('turns the other way once the detector moves the target', async ({ page }) => {
    // Arrange
    await placeTargetAt(0.9, 0.5)
    await joinCamera(page)
    await trackTarget(page, '人')
    const move = page.getByTestId('ptz-tracking-move')
    await expect(move).toHaveText(PAN_RIGHT_TEXT, { timeout: 30_000 })

    // Act
    await placeTargetAt(0.1, 0.5)

    // Assert
    await expect(move).toHaveText(PAN_LEFT_TEXT, { timeout: 30_000 })
  })

  test('reports a target the detector does not see', async ({ page }) => {
    // Arrange
    await hideTarget()
    const position = await joinCamera(page)

    // Act
    await trackTarget(page, '人')

    // Assert
    await expect(position).toHaveText('見つかりません', { timeout: 30_000 })
  })

  test('shows the System One status while its answers are late', async ({ page }) => {
    // Arrange
    await placeTargetAt(0.5, 0.5)
    await delaySystemOneAnswers(SYSTEM_ONE_ORIGIN, LATE_ANSWER_MS)
    const position = await joinCamera(page)

    // Act
    await trackTarget(page, '人')

    // Assert
    const systemOneStatus = page.getByTestId('ptz-tracking-system-one-status')
    await expect(systemOneStatus).toContainText('d1-2B', { timeout: 30_000 })
    await delaySystemOneAnswers(SYSTEM_ONE_ORIGIN, 0)
    await expect(position).toHaveText('横 50%・縦 50%', { timeout: 30_000 })
    await expect(systemOneStatus).toBeHidden()
  })
})

/// The first ContinuousMove and the command after it; a Stop that ends an earlier sweep may precede them.
function stepCommands(commands: PtzCommand[]): PtzCommand[] {
  const start = commands.findIndex((command) => command.type === 'continuous')
  return start < 0 ? [] : commands.slice(start, start + 2)
}
