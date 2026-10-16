import { type Page } from '@playwright/test'

/// Set by tests/browser-e2e/run-goaway-e2e.mjs: SIGINT makes relay A send GOAWAY naming relay B.
export const relayAPid = Number(process.env.GOAWAY_E2E_RELAY_A_PID)
const MOVE_TIMEOUT_MS = 15_000

/// Call before the drain starts and call the returned function after it; that function resolves with the URL of the
/// relay the page moved to.
export function listenForRelayMove(page: Page): () => Promise<string> {
  const moved = new Promise<string>((resolve) =>
    page.on('console', async (message) => {
      if (message.text().startsWith('[moqt] moved to the relay named by GOAWAY')) {
        const { url } = (await message.args()[1].jsonValue()) as { url: string }
        resolve(url)
      }
    })
  )
  return () =>
    Promise.race([
      moved,
      new Promise<never>((_, reject) => setTimeout(() => reject(new Error('the page did not move')), MOVE_TIMEOUT_MS))
    ])
}

export function processAlive(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}
