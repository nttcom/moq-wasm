import { expect, test } from '@playwright/test'
import { DATAGRAM_INDEX_PATH } from '../playwright.helpers'

const moqtUrl = process.env.DATAGRAM_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const RECEIVED_AT_LEAST = 30

test('the cursor reaches the page through the relay on both the stream and the datagram track', async ({ page }) => {
  // Arrange
  await page.goto(`${DATAGRAM_INDEX_PATH}?${new URLSearchParams({ moqtUrl })}`)
  await expect(page.locator('#url')).toHaveValue(moqtUrl)

  // Act
  await page.getByTestId('datagram-start-button').click()

  // Assert: the page subscribes to its own two tracks, so each counts the samples it wrote
  await expect(page.getByTestId('datagram-status')).toContainText('送受信中')
  for (const lane of ['stream', 'datagram']) {
    await expect
      .poll(async () => Number(await page.getByTestId(`datagram-stats-${lane}`).getAttribute('data-received')))
      .toBeGreaterThanOrEqual(RECEIVED_AT_LEAST)
  }
  await page.getByTestId('datagram-stop-button').click()
  await expect(page.getByTestId('datagram-status')).toHaveText('停止しました')
})
