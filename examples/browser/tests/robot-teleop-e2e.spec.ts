import { expect, test, type Page } from '@playwright/test'
import { ROBOT_TELEOP_INDEX_PATH } from '../playwright.helpers'

const moqtUrl = process.env.ROBOT_TELEOP_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const room = process.env.ROBOT_TELEOP_E2E_ROOM ?? 'anon/robot-teleop'

async function robotX(page: Page): Promise<number> {
  const text = (await page.getByTestId('robot-teleop-position').textContent()) ?? ''
  const match = /x (-?\d+\.\d+)/.exec(text)
  return match ? Number(match[1]) : Number.NaN
}

test('an operator drives the robot with a held key and the robot holds once it is released', async ({ page }) => {
  // Arrange
  await page.goto(`${ROBOT_TELEOP_INDEX_PATH}?${new URLSearchParams({ moqtUrl })}`)
  await page.getByTestId('robot-teleop-room').fill(room)
  await page.getByTestId('robot-teleop-connect').click()
  await expect(page.getByTestId('robot-teleop-status')).toHaveText('接続しました')
  await expect(page.getByTestId('robot-teleop-position')).toContainText('x ', { timeout: 15_000 })
  const operator = (await page.getByTestId('robot-teleop-operator').textContent()) ?? ''
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
