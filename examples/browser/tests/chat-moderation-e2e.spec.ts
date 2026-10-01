import { expect, test } from '@playwright/test'
import { ABUSIVE_TEXT, REMOVED_MESSAGE_TEXT, joinChat } from './chat-moderation-e2e-arrange'

const ORDINARY_TEXT = 'こんにちは'

test.describe('MoQ Chat Moderation', () => {
  test('replaces only the message the moderator judges abusive', async ({ browser }) => {
    // Arrange
    const chat = await joinChat(browser)

    try {
      // Act
      await chat.send(ORDINARY_TEXT)
      await chat.send(`${ABUSIVE_TEXT} です`)

      // Assert
      await expect(chat.messages.nth(0)).toHaveAttribute('data-verdict', 'ok')
      await expect(chat.messages.nth(0)).toContainText(ORDINARY_TEXT)
      await expect(chat.messages.nth(1)).toHaveAttribute('data-verdict', 'removed')
      await expect(chat.messages.nth(1)).toHaveText(REMOVED_MESSAGE_TEXT)
    } finally {
      await chat.context.close()
    }
  })

  test('moderates a page that joins after another one left', async ({ browser }) => {
    // Arrange
    const earlierChat = await joinChat(browser)
    await earlierChat.send(ORDINARY_TEXT)
    await expect(earlierChat.messages.nth(0)).toHaveAttribute('data-verdict', 'ok')
    await earlierChat.context.close()
    const chat = await joinChat(browser)

    try {
      // Act
      await chat.send(ABUSIVE_TEXT)

      // Assert
      await expect(chat.messages.nth(0)).toHaveText(REMOVED_MESSAGE_TEXT)
    } finally {
      await chat.context.close()
    }
  })
})
