import { expect, test } from '@playwright/test'
import { ABUSIVE_TEXT, REMOVED_MESSAGE_TEXT, clickJoin, delayJevAnswers, joinChat } from './chat-moderation-e2e-arrange'

const ORDINARY_TEXT = 'こんにちは'
const LATE_ANSWER_MS = 6_000
const LATE_JOIN_ANSWER_MS = 15_000

test.describe('MoQ Chat Moderation', () => {
  test.afterEach(() => delayJevAnswers(0))

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
    await earlierPage.close()
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
    await earlierPage.getByTestId('chat-moderation-leave-button').click()
    await expect(page.locator('#moderator-status')).toHaveAttribute('data-state', 'ok', { timeout: 30_000 })

    // Act
    await chat.send(ABUSIVE_TEXT)

    // Assert
    await expect(chat.messages.nth(0)).toHaveText(REMOVED_MESSAGE_TEXT)
  })

  test('shows the djev status while its answers are late', async ({ page }) => {
    // Arrange
    const chat = await joinChat(page)
    await delayJevAnswers(LATE_ANSWER_MS)

    // Act
    await chat.send(ABUSIVE_TEXT)

    // Assert
    const djevStatus = page.getByTestId('chat-moderation-djev-status')
    await expect(djevStatus).toContainText('djev')
    await expect(chat.messages.nth(0)).toHaveText(REMOVED_MESSAGE_TEXT, { timeout: 30_000 })
    await expect(djevStatus).toBeHidden()
  })

  test('shows the djev status to a page that joins while answers are late', async ({ browser, page }) => {
    // Arrange
    const earlierPage = await browser.newPage()
    const earlierChat = await joinChat(earlierPage)
    await delayJevAnswers(LATE_JOIN_ANSWER_MS)
    await earlierChat.send(ABUSIVE_TEXT)
    await expect(earlierPage.getByTestId('chat-moderation-djev-status')).toContainText('djev', { timeout: 10_000 })

    // Act
    await clickJoin(page)

    // Assert
    await expect(page.getByTestId('chat-moderation-djev-status')).toContainText('djev')
  })
})
