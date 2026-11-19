import { type Page, expect } from '@playwright/test'

/// The servers and token an example whose bot asks the fake System One server reads from the
/// `<envPrefix>_E2E_*` variables its runner sets.
export function roomBotE2E(envPrefix: string) {
  const env = (name: string) => process.env[`${envPrefix}_E2E_${name}`]
  const moqtUrl = env('MOQT_URL') ?? 'https://127.0.0.1:4433'
  // The pages join the runner's temporary app, whose pages may start bots through temporary-apps.
  const temporaryAppsUrl = env('TEMPORARY_APPS_URL')!
  const appFragment = `#jwt=${env('TEMPORARY_APP_TOKEN')!}`
  const systemOneOrigin = env('SYSTEM_ONE_ORIGIN')!
  return {
    systemOneOrigin,
    /// The boxes the fake System One server finds for each target, as [left, top, right, bottom];
    /// a target left out has none.
    placeBoxes: async (boxes: Record<string, [number, number, number, number][]>): Promise<void> => {
      const response = await fetch(`${systemOneOrigin}/boxes`, { method: 'PUT', body: JSON.stringify(boxes) })
      expect(response.ok).toBe(true)
    },
    clickJoin: async (page: Page, path: string, testIdPrefix: string, room?: string): Promise<void> => {
      const params = new URLSearchParams({ moqtUrl, temporaryAppsUrl, ...(room === undefined ? {} : { room }) })
      await page.goto(`${path}?${params}${appFragment}`)
      await page.getByTestId(`${testIdPrefix}-join-button`).click()
    }
  }
}

/// Presses the page's 「bot を参加させる」 once it has joined the room. Clicks through the DOM because a
/// room whose bot is already in it keeps the button disabled, and Playwright's click would wait for it
/// to be enabled.
export async function pressStartBot(page: Page, testIdPrefix: string): Promise<void> {
  await expect(page.locator('#connection-status')).toHaveAttribute('data-state', 'ok', { timeout: 30_000 })
  await page.getByTestId(`${testIdPrefix}-bot-button`).evaluate((button: HTMLButtonElement) => button.click())
}
