export interface CaptureRetryOptions {
  maxAttempts: number
  retryDelayMs: number
  /// Checked before and after each `capture` call; a stream that arrives after it turns true is stopped.
  isCancelled: () => boolean
}

/// Calls `capture` until it returns a stream whose first track is live.
/// Resolves to `null` when cancelled, and rejects with the last error once
/// `maxAttempts` attempts have failed.
export async function captureWithRetry(
  capture: () => Promise<MediaStream>,
  { maxAttempts, retryDelayMs, isCancelled }: CaptureRetryOptions
): Promise<MediaStream | null> {
  let lastError: unknown
  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    if (attempt > 1) {
      await new Promise((resolve) => setTimeout(resolve, retryDelayMs))
    }
    if (isCancelled()) {
      return null
    }
    try {
      const stream = await capture()
      if (isCancelled()) {
        stopTracks(stream)
        return null
      }
      if (stream.getTracks()[0]?.readyState === 'live') {
        return stream
      }
      stopTracks(stream)
      lastError = new Error('captured track is not live')
    } catch (error) {
      if (isCancelled()) {
        return null
      }
      lastError = error
    }
  }
  throw lastError
}

function stopTracks(stream: MediaStream): void {
  stream.getTracks().forEach((track) => track.stop())
}
