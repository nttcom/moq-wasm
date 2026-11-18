import { type Page, expect, test } from '@playwright/test'
import { WHITEBOARD_INDEX_PATH } from '../playwright.helpers'

const moqtUrl = process.env.WHITEBOARD_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'

async function joinRoom(page: Page, room: string): Promise<void> {
  await page.goto(`${WHITEBOARD_INDEX_PATH}?${new URLSearchParams({ moqtUrl, room })}`)
  await expect(page.locator('#url')).toHaveValue(moqtUrl)
  await page.getByTestId('whiteboard-join-button').click()
  await expect(page.getByTestId('whiteboard-status')).toContainText('参加中')
}

/// `y` is in [0, 1] of the board's height.
async function drawLine(page: Page, y: number): Promise<void> {
  const canvas = page.getByTestId('whiteboard-canvas')
  // page.mouse acts only within the viewport, and the board starts below the connection card.
  await canvas.scrollIntoViewIfNeeded()
  const box = (await canvas.boundingBox())!
  await page.mouse.move(box.x + box.width * 0.2, box.y + box.height * y)
  await page.mouse.down()
  await page.mouse.move(box.x + box.width * 0.8, box.y + box.height * y, { steps: 10 })
  await page.mouse.up()
}

async function expectStrokes(page: Page, count: number): Promise<void> {
  await expect(page.getByTestId('whiteboard-stroke-count')).toHaveAttribute('data-count', count.toString())
}

async function expectParticipants(page: Page, count: number): Promise<void> {
  await expect(page.getByTestId('whiteboard-participants')).toHaveAttribute('data-count', count.toString())
}

test('a late joiner sees the board, including the strokes of a participant who left', async ({ browser }) => {
  // Arrange
  const room = `e2e-${Date.now().toString(36)}`
  const [alice, bob, carol] = await Promise.all([1, 2, 3].map(async () => (await browser.newContext()).newPage()))
  await joinRoom(alice, room)
  await drawLine(alice, 0.3)
  await drawLine(alice, 0.5)
  await expectStrokes(alice, 2)

  // Act
  await joinRoom(bob, room)
  await expectStrokes(bob, 2)
  await drawLine(bob, 0.7)
  await expectStrokes(alice, 3)
  await alice.getByTestId('whiteboard-leave-button').click()
  await expectParticipants(bob, 1)
  await joinRoom(carol, room)

  // Assert: carol reads alice's lines from bob's snapshot, and her clear reaches bob
  await expectStrokes(carol, 3)
  await expectParticipants(carol, 2)
  await carol.getByTestId('whiteboard-clear-button').click()
  await expectStrokes(bob, 0)
})
