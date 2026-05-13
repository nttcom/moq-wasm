export type BitrateLogger = {
  addBytes: (byteLength: number) => void
}

export function createBitrateLogger(label: string, intervalMs = 1000): BitrateLogger {
  let bytesThisInterval = 0
  let lastLogTime = performance.now()

  return {
    addBytes(byteLength: number) {
      bytesThisInterval += byteLength
      const now = performance.now()
      if (now - lastLogTime >= intervalMs) {
        const mbps = (bytesThisInterval * 8) / 1_000_000
        console.log(`${label}: ${mbps.toFixed(2)} Mbps`)
        bytesThisInterval = 0
        lastLogTime = now
      }
    }
  }
}
