const INTERVAL_MS = 1_000

/// Returns the function to call with each received object's byte length; it reports the kbps to
/// `onReport` about once a second.
export function createBitrateLogger(onReport: (kbps: number) => void): (byteLength: number) => void {
  let bytesThisInterval = 0
  let lastLogTime = performance.now()

  return (byteLength) => {
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
