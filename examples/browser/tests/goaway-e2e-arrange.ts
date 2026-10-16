/// Set by tests/browser-e2e/run-goaway-e2e.mjs: SIGINT makes relay A send GOAWAY naming relay B.
export const relayAPid = Number(process.env.GOAWAY_E2E_RELAY_A_PID)

export function processAlive(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}
