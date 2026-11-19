import { type Page, expect } from '@playwright/test'

/// Presses the page's 「bot を参加させる」 once it has joined the room. Clicks through the DOM because a
/// room whose bot is already in it keeps the button disabled, and Playwright's click would wait for it
/// to be enabled.
export async function pressStartBot(page: Page, testIdPrefix: string): Promise<void> {
  await expect(page.locator('#connection-status')).toHaveAttribute('data-state', 'ok', { timeout: 30_000 })
  await page.getByTestId(`${testIdPrefix}-bot-button`).evaluate((button: HTMLButtonElement) => button.click())
}
