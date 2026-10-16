import { expect, test } from '@playwright/test'
import {
  ABUSIVE_TEXT,
  LEAVE_BUTTON_TEST_ID,
  REMOVED_MESSAGE_TEXT,
  D3_FLASH_ORIGIN,
  clickJoin,
  joinChat
} from './chat-moderation-e2e-arrange'
import { delayD3FlashAnswers } from './fake-d3-flash'
import { leaveAllPages } from './leave-all-pages'

const ORDINARY_TEXT = 'こんにちは'
const LATE_ANSWER_MS = 6_000
const LATE_JOIN_ANSWER_MS = 15_000

test.describe('MoQ Chat Moderation', () => {
  test.afterEach(async ({ browser }) => {
    await leaveAllPages(browser, LEAVE_BUTTON_TEST_ID)
    await delayD3FlashAnswers(D3_FLASH_ORIGIN, 0)
  })

  test('replaces only the message the moderator judges abusive', async ({ page }) => {
    // Arrange
    const chat = await joinChat(page)

    // Act
    await chat.send(ORDINARY_TEXT)
    await chat.send(`${ABUSIVE_TEXT} です`)

    // Assert
    await expect(chat.messages.nth(0)).toHaveAttribute('data-verdict', 'ok')
    await expect(chat.messages.nth(0)).toContainText(ORDINARY_TEXT)
    await expect(chat.messages.nth(1)).toHaveAttribute('data-verdict', 'removed')
    await expect(chat.messages.nth(1)).toHaveText(REMOVED_MESSAGE_TEXT)
  })

  test('moderates a page that joins after another one left', async ({ browser, page }) => {
    // Arrange
    const earlierPage = await browser.newPage()
    const earlierChat = await joinChat(earlierPage)
    await earlierChat.send(ORDINARY_TEXT)
    await expect(earlierChat.messages.nth(0)).toHaveAttribute('data-verdict', 'ok')
    await earlierChat.leave()
    const chat = await joinChat(page)

    // Act
    await chat.send(ABUSIVE_TEXT)

    // Assert
    await expect(chat.messages.nth(0)).toHaveText(REMOVED_MESSAGE_TEXT)
  })

  test('moderates a page that joined before another one left', async ({ browser, page }) => {
    // Arrange
    const earlierPage = await browser.newPage()
    const earlierChat = await joinChat(earlierPage)
    await earlierChat.send(ORDINARY_TEXT)
    await expect(earlierChat.messages.nth(0)).toHaveAttribute('data-verdict', 'ok')
    const chat = await clickJoin(page)
    await earlierChat.leave()
    await expect(page.locator('#moderator-status')).toHaveAttribute('data-state', 'ok', { timeout: 30_000 })

    // Act
    await chat.send(ABUSIVE_TEXT)

    // Assert
    await expect(chat.messages.nth(0)).toHaveText(REMOVED_MESSAGE_TEXT)
  })

  test('shows the d3-flash status while its answers are late', async ({ page }) => {
    // Arrange
    const chat = await joinChat(page)
    await delayD3FlashAnswers(D3_FLASH_ORIGIN, LATE_ANSWER_MS)

    // Act
    await chat.send(ABUSIVE_TEXT)

    // Assert
    const d3FlashStatus = page.getByTestId('chat-moderation-d3-flash-status')
    await expect(d3FlashStatus).toContainText('d3-flash')
    await expect(chat.messages.nth(0)).toHaveText(REMOVED_MESSAGE_TEXT, { timeout: 30_000 })
    await expect(d3FlashStatus).toBeHidden()
  })

  test('shows the d3-flash status to a page that joins while answers are late', async ({ browser, page }) => {
    // Arrange
    const earlierPage = await browser.newPage()
    const earlierChat = await joinChat(earlierPage)
    await delayD3FlashAnswers(D3_FLASH_ORIGIN, LATE_JOIN_ANSWER_MS)
    await earlierChat.send(ABUSIVE_TEXT)
    await expect(earlierPage.getByTestId('chat-moderation-d3-flash-status')).toContainText('d3-flash', {
      timeout: 10_000
    })

    // Act
    await clickJoin(page)

    // Assert
    await expect(page.getByTestId('chat-moderation-d3-flash-status')).toContainText('d3-flash')
  })
})
