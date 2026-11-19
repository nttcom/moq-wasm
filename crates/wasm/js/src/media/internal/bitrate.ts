const INTERVAL_MS = 1_000

/// Returns the function to call with each received object's byte length; it reports the kbps to
/// `onReport` about once a second, over the time since the previous report.
export function createBitrateLogger(
  onReport: (kbps: number) => void,
  now: () => number = () => performance.now()
): (byteLength: number) => void {
  let bytesThisInterval = 0
  let lastLogTime = now()

  return (byteLength) => {
    bytesThisInterval += byteLength
    const time = now()
    const elapsedMs = time - lastLogTime
    if (elapsedMs >= INTERVAL_MS) {
      onReport((bytesThisInterval * 8) / elapsedMs)
      bytesThisInterval = 0
      lastLogTime = time
    }
  }
}
