import { expect, test, type Page } from '@playwright/test'
import { ROBOT_TELEOP_INDEX_PATH } from '../playwright.helpers'

const moqtUrl = process.env.ROBOT_TELEOP_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const room = process.env.ROBOT_TELEOP_E2E_ROOM ?? 'anon/robot-teleop'
const policyRoom = process.env.ROBOT_TELEOP_E2E_POLICY_ROOM ?? 'anon/robot-teleop-policy'

async function robotX(page: Page): Promise<number> {
  const text = (await page.getByTestId('robot-teleop-position').textContent()) ?? ''
  const match = /x (-?\d+\.\d+)/.exec(text)
  return match ? Number(match[1]) : Number.NaN
}

/// Opens the operator page connected to `roomName`, once it shows the robot; returns this page's operator.
async function connect(page: Page, roomName: string): Promise<string> {
  await page.goto(`${ROBOT_TELEOP_INDEX_PATH}?${new URLSearchParams({ moqtUrl })}`)
  await page.getByTestId('robot-teleop-room').fill(roomName)
  await page.getByTestId('robot-teleop-connect').click()
  await expect(page.getByTestId('robot-teleop-status')).toHaveText('接続しました')
  await expect(page.getByTestId('robot-teleop-position')).toContainText('x ', { timeout: 15_000 })
  return (await page.getByTestId('robot-teleop-operator').textContent()) ?? ''
}

test('an operator drives the robot with a held key and the robot holds once it is released', async ({ page }) => {
  // Arrange
  const operator = await connect(page, room)
  const startX = await robotX(page)

  // Act
  await page.keyboard.down('ArrowRight')

  // Assert: the robot follows this page's actions
  await expect(page.getByTestId('robot-teleop-driver')).toHaveText(`${operator} (action)`, { timeout: 10_000 })
  await expect.poll(() => robotX(page), { timeout: 10_000 }).toBeGreaterThan(startX + 0.1)

  // Act
  await page.keyboard.up('ArrowRight')

  // Assert: without fresh actions the operator stops driving and the robot stays put
  await expect(page.getByTestId('robot-teleop-driver')).toHaveText('なし (stale)', { timeout: 10_000 })
  const stoppedX = await robotX(page)
  await page.waitForTimeout(500)
  expect(await robotX(page)).toBe(stoppedX)
})

test('an operator intervenes on the policy, which drives again once the operator stops', async ({ page }) => {
  // Arrange: the policy drives the robot towards (0.5, 0.5)
  const operator = await connect(page, policyRoom)
  const driver = page.getByTestId('robot-teleop-driver')
  await expect(driver).toHaveText(/^bots\/policy /, { timeout: 10_000 })
  await expect.poll(() => robotX(page), { timeout: 10_000 }).toBeGreaterThan(0.3)

  // Act
  await page.keyboard.down('ArrowLeft')

  // Assert
  await expect(driver).toHaveText(`${operator} (action)`, { timeout: 10_000 })
  await expect.poll(() => robotX(page), { timeout: 10_000 }).toBeLessThan(0.2)

  // Act
  await page.keyboard.up('ArrowLeft')

  // Assert
  await expect(driver).toHaveText('bots/policy (stale)', { timeout: 10_000 })
  await expect.poll(() => robotX(page), { timeout: 10_000 }).toBeGreaterThan(0.3)
})
