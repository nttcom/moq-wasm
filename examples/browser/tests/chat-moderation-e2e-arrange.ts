import { type Locator, type Page, expect } from '@playwright/test'
import { CHAT_MODERATION_PATH } from '../playwright.helpers'
import { leavePage } from './leave-all-pages'

const moqtUrl = process.env.CHAT_MODERATION_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const jevOrigin = process.env.CHAT_MODERATION_E2E_JEV_ORIGIN!

export const LEAVE_BUTTON_TEST_ID = 'chat-moderation-leave-button'
export const ABUSIVE_TEXT = 'e2e-abusive'
export const REMOVED_MESSAGE_TEXT = 'モデレーターによって削除されました'

export interface ChatPage {
  messages: Locator
  send(text: string): Promise<void>
  leave(): Promise<void>
}

export async function delayJevAnswers(milliseconds: number): Promise<void> {
  const response = await fetch(`${jevOrigin}/delay`, { method: 'PUT', body: String(milliseconds) })
  expect(response.ok).toBe(true)
}

export async function joinChat(page: Page): Promise<ChatPage> {
  const chat = await clickJoin(page)
  await expect(page.locator('#moderator-status')).toHaveAttribute('data-state', 'ok', { timeout: 60_000 })
  return chat
}

export async function clickJoin(page: Page): Promise<ChatPage> {
  await page.goto(`${CHAT_MODERATION_PATH}?${new URLSearchParams({ moqtUrl })}`)
  await page.getByTestId('chat-moderation-join-button').click()
  return {
    messages: page.locator('.chat-message'),
    send: async (text) => {
      await page.getByTestId('chat-moderation-input').fill(text)
      await page.getByTestId('chat-moderation-send-button').click()
    },
    leave: () => leavePage(page, LEAVE_BUTTON_TEST_ID)
  }
}
