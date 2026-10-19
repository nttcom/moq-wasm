import { type Page, expect } from '@playwright/test'
import { CHAT_MODERATION_PATH } from '../playwright.helpers'
import { leavePage } from './leave-all-pages'

const moqtUrl = process.env.CHAT_MODERATION_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const botUrl = process.env.CHAT_MODERATION_E2E_BOT_URL!
/// Every test joins one room unless it asks for another, so pages that follow one another meet one moderator.
const ROOM = 'e2e'
export const D3_FLASH_ORIGIN = process.env.CHAT_MODERATION_E2E_D3_FLASH_ORIGIN!

export const LEAVE_BUTTON_TEST_ID = 'chat-moderation-leave-button'
export const ABUSIVE_TEXT = 'e2e-abusive'
export const REMOVED_MESSAGE_TEXT = 'モデレーターによって削除されました'

export async function joinChat(page: Page, room = ROOM) {
  const chat = await clickJoin(page, room)
  await expect(page.locator('#moderator-status')).toHaveAttribute('data-state', 'ok', { timeout: 60_000 })
  return chat
}

export async function clickJoin(page: Page, room = ROOM) {
  await page.goto(`${CHAT_MODERATION_PATH}?${new URLSearchParams({ moqtUrl, botUrl, room })}`)
  await page.getByTestId('chat-moderation-join-button').click()
  return {
    messages: page.locator('.chat-message'),
    send: async (text: string) => {
      await page.getByTestId('chat-moderation-input').fill(text)
      await page.getByTestId('chat-moderation-send-button').click()
    },
    leave: () => leavePage(page, LEAVE_BUTTON_TEST_ID)
  }
}
