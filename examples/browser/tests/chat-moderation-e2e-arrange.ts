import { type Page, expect } from '@playwright/test'
import { CHAT_MODERATION_PATH } from '../playwright.helpers'
import { leavePage } from './leave-all-pages'

const moqtUrl = process.env.CHAT_MODERATION_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const botUrl = process.env.CHAT_MODERATION_E2E_BOT_URL!
/// The room every page joins unless a test passes another, so the pages of one test meet one moderator.
const ROOM = 'e2e'
export const SYSTEM_ONE_ORIGIN = process.env.CHAT_MODERATION_E2E_SYSTEM_ONE_ORIGIN!

export const LEAVE_BUTTON_TEST_ID = 'chat-moderation-leave-button'
/// The fake System One gives a label 0.95 when the message contains `e2e-` and the label's name.
export const ABUSIVE_TEXT = 'e2e-暴言'
export const REMOVED_MESSAGE_TEXT = 'モデレーターによって削除されました'

export async function joinChat(page: Page, room = ROOM) {
  const chat = await clickJoin(page, room)
  await expect(page.locator('#moderator-status')).toHaveAttribute('data-state', 'ok', { timeout: 60_000 })
  return chat
}

export async function clickJoin(page: Page, room = ROOM) {
  await page.goto(`${CHAT_MODERATION_PATH}?${new URLSearchParams({ moqtUrl, botUrl, room })}`)
  await page.getByTestId('chat-moderation-join-button').click()
  const messages = page.locator('.chat-message')
  return {
    messages,
    texts: messages.locator('.chat-text'),
    chips: (index: number) => messages.nth(index).locator('.chat-chip'),
    sidebar: page.getByTestId('chat-moderation-sidebar'),
    removeLabel: async (name: string) => {
      const names = await page
        .getByTestId('chat-moderation-label-name-input')
        .evaluateAll((inputs) => inputs.map((input) => (input as HTMLInputElement).value))
      await page.getByTestId('chat-moderation-remove-label-button').nth(names.indexOf(name)).click()
    },
    send: async (text: string) => {
      await page.getByTestId('chat-moderation-input').fill(text)
      await page.getByTestId('chat-moderation-send-button').click()
    },
    leave: () => leavePage(page, LEAVE_BUTTON_TEST_ID)
  }
}
