export type BitrateAccumulator = {
  addBytes: (byteLength: number) => void
}

const INTERVAL_MS = 1_000

export function createBitrateLogger(onReport: (kbps: number) => void): BitrateAccumulator {
  let bytesThisInterval = 0
  let lastLogTime = performance.now()

  return {
    addBytes(byteLength: number) {
      bytesThisInterval += byteLength
      const now = performance.now()
      if (now - lastLogTime >= INTERVAL_MS) {
        const kbps = (bytesThisInterval * 8) / 1_000
        onReport(kbps)
        bytesThisInterval = 0
        lastLogTime = now
      }
    }
  }
}
