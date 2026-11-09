export type BitrateAccumulator = {
  addBytes: (byteLength: number) => void
}

export function createBitrateLogger(onReport: (kbps: number) => void, intervalMs = 1000): BitrateAccumulator {
  let bytesThisInterval = 0
  let lastLogTime = performance.now()

  return {
    addBytes(byteLength: number) {
      bytesThisInterval += byteLength
      const now = performance.now()
      if (now - lastLogTime >= intervalMs) {
        const kbps = (bytesThisInterval * 8) / 1_000
        onReport(kbps)
        bytesThisInterval = 0
        lastLogTime = now
      }
    }
  }
}
