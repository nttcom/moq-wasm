import { type Browser, type BrowserContext, type Locator, type Page, expect } from '@playwright/test'
import { CHAT_MODERATION_PATH } from '../playwright.helpers'

const moqtUrl = process.env.CHAT_MODERATION_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'

export const ABUSIVE_TEXT = process.env.CHAT_MODERATION_E2E_ABUSIVE_TEXT ?? 'e2e-abusive'
export const REMOVED_MESSAGE_TEXT = 'モデレーターによって削除されました'

export interface ChatPage {
  context: BrowserContext
  page: Page
  messages: Locator
  send(text: string): Promise<void>
}

export async function joinChat(browser: Browser): Promise<ChatPage> {
  const context = await browser.newContext()
  const page = await context.newPage()
  await page.goto(`${CHAT_MODERATION_PATH}?${new URLSearchParams({ moqtUrl })}`)
  await page.getByTestId('chat-moderation-join-button').click()
  await expect(page.locator('#moderator-status')).toHaveAttribute('data-state', 'ok', { timeout: 60_000 })
  return {
    context,
    page,
    messages: page.locator('.chat-message'),
    send: async (text) => {
      await page.getByTestId('chat-moderation-input').fill(text)
      await page.getByTestId('chat-moderation-send-button').click()
    }
  }
}
