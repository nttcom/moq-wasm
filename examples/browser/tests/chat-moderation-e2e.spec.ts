import { expect, test } from '@playwright/test'
import {
  ABUSIVE_TEXT,
  LEAVE_BUTTON_TEST_ID,
  REMOVED_MESSAGE_TEXT,
  SYSTEM_ONE_ORIGIN,
  clickJoin,
  joinChat
} from './chat-moderation-e2e-arrange'
import { delaySystemOneAnswers } from './fake-system-one'
import { leaveAllPages } from './leave-all-pages'

const ORDINARY_TEXT = 'こんにちは'
const JOYFUL_TEXT = 'e2e-喜 e2e-楽'
const LATE_ANSWER_MS = 6_000
const LATE_JOIN_ANSWER_MS = 15_000

test.describe('MoQ Chat Moderation', () => {
  test.afterEach(async ({ browser }) => {
    await leaveAllPages(browser, LEAVE_BUTTON_TEST_ID)
    await delaySystemOneAnswers(SYSTEM_ONE_ORIGIN, 0)
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
    await expect(chat.texts.nth(1)).toHaveText(REMOVED_MESSAGE_TEXT)
  })

  test('labels a message with every label the moderator gives it', async ({ page }) => {
    // Arrange
    const chat = await joinChat(page)

    // Act
    await chat.send(JOYFUL_TEXT)

    // Assert
    await expect(chat.chips(0)).toHaveText(['喜', '楽'])
    await expect(chat.messages.nth(0)).toHaveAttribute('data-verdict', 'ok')
    await expect(chat.texts.nth(0)).toHaveText(JOYFUL_TEXT)
  })

  test('shows every label probability of a clicked message, highest first', async ({ page }) => {
    // Arrange
    const chat = await joinChat(page)
    await chat.send(ABUSIVE_TEXT)
    await expect(chat.messages.nth(0)).toHaveAttribute('data-verdict', 'removed')

    // Act
    await chat.messages.nth(0).click()

    // Assert
    const scores = chat.sidebar.getByTestId('chat-moderation-label-score')
    await expect(scores).toHaveCount(6)
    await expect(scores.nth(0)).toHaveText(/暴言\s*95%/)
    await expect(scores.nth(5)).toContainText('2%')
    await page.getByTestId('chat-moderation-sidebar-close-button').click()
    await expect(chat.sidebar).toBeHidden()
  })

  test('does not give a label removed before the message was sent', async ({ page }) => {
    // Arrange
    const chat = await joinChat(page)
    await chat.removeLabel('暴言')

    // Act
    await chat.send(ABUSIVE_TEXT)

    // Assert
    await expect(chat.messages.nth(0)).toHaveAttribute('data-verdict', 'ok')
    await expect(chat.texts.nth(0)).toHaveText(ABUSIVE_TEXT)
    await expect(chat.chips(0)).toHaveCount(0)
    await chat.messages.nth(0).click()
    const scores = chat.sidebar.getByTestId('chat-moderation-label-score')
    await expect(scores).toHaveCount(5)
    await expect(scores.filter({ hasText: '暴言' })).toHaveCount(0)
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
    await expect(chat.texts.nth(0)).toHaveText(REMOVED_MESSAGE_TEXT)
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
    await expect(chat.texts.nth(0)).toHaveText(REMOVED_MESSAGE_TEXT)
  })

  test('moderates each room with its own moderator', async ({ browser, page }) => {
    // Arrange
    const otherPage = await browser.newPage()
    const otherChat = await joinChat(otherPage, 'e2e-other')
    const chat = await joinChat(page)

    // Act
    await otherChat.send(ABUSIVE_TEXT)
    await chat.send(ABUSIVE_TEXT)

    // Assert
    await expect(otherChat.texts.nth(0)).toHaveText(REMOVED_MESSAGE_TEXT)
    await expect(chat.texts.nth(0)).toHaveText(REMOVED_MESSAGE_TEXT)
  })

  test('shows the System One status while its answers are late', async ({ page }) => {
    // Arrange
    const chat = await joinChat(page)
    await delaySystemOneAnswers(SYSTEM_ONE_ORIGIN, LATE_ANSWER_MS)

    // Act
    await chat.send(ABUSIVE_TEXT)

    // Assert
    const systemOneStatus = page.getByTestId('chat-moderation-system-one-status')
    await expect(systemOneStatus).toContainText('d1-2B')
    await expect(chat.texts.nth(0)).toHaveText(REMOVED_MESSAGE_TEXT, { timeout: 30_000 })
    await expect(systemOneStatus).toBeHidden()
  })

  test('shows the System One status to a page that joins while answers are late', async ({ browser, page }) => {
    // Arrange
    const earlierPage = await browser.newPage()
    const earlierChat = await joinChat(earlierPage)
    await delaySystemOneAnswers(SYSTEM_ONE_ORIGIN, LATE_JOIN_ANSWER_MS)
    await earlierChat.send(ABUSIVE_TEXT)
    await expect(earlierPage.getByTestId('chat-moderation-system-one-status')).toContainText('d1-2B', {
      timeout: 10_000
    })

    // Act
    await clickJoin(page)

    // Assert
    await expect(page.getByTestId('chat-moderation-system-one-status')).toContainText('d1-2B')
  })
})
