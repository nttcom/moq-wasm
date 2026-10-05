import { type Browser, type Page, expect } from '@playwright/test'

/// Chromium in CI keeps a closed page's WebTransport session open until the relay's
/// idle timeout (about 45 s), and a bot bound to that dead peer waits as long.
export async function leaveAllPages(browser: Browser, leaveButtonTestId: string): Promise<void> {
  const pages = browser.contexts().flatMap((context) => context.pages())
  await Promise.all(pages.map((page) => leavePage(page, leaveButtonTestId)))
}

export async function leavePage(page: Page, leaveButtonTestId: string): Promise<void> {
  const leaveButton = page.getByTestId(leaveButtonTestId)
  if (await leaveButton.isEnabled()) {
    await leaveButton.click()
    await expect(page.locator('#connection-status')).toHaveAttribute('data-state', 'idle')
  }
}
