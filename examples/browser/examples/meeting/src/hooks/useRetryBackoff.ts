import { useState } from 'react'

const BASE_RETRY_DELAY_MS = 500
const MAX_FAILURES = 6

type RetryAttempt = {
  key: string
  failures: number
  inFlight: boolean
  retryTimerId: number | null
}

class RetryBackoff {
  private readonly attempts = new Map<string, RetryAttempt>()

  constructor(
    private readonly label: string,
    private readonly onRetryDue: () => void
  ) {}

  begin(key: string): RetryAttempt | null {
    const existing = this.attempts.get(key)
    if (existing && (existing.inFlight || existing.retryTimerId !== null || existing.failures >= MAX_FAILURES)) {
      return null
    }
    const attempt = existing ?? { key, failures: 0, inFlight: false, retryTimerId: null }
    attempt.inFlight = true
    this.attempts.set(key, attempt)
    return attempt
  }

  succeed(attempt: RetryAttempt): void {
    if (this.attempts.get(attempt.key) === attempt) {
      this.attempts.delete(attempt.key)
    }
  }

  fail(attempt: RetryAttempt): void {
    if (this.attempts.get(attempt.key) !== attempt) {
      return
    }
    attempt.inFlight = false
    attempt.failures += 1
    if (attempt.failures >= MAX_FAILURES) {
      console.warn(`[meeting][${this.label}] giving up`, { key: attempt.key, failures: attempt.failures })
      return
    }
    const delayMs = BASE_RETRY_DELAY_MS * 2 ** (attempt.failures - 1)
    console.info(`[meeting][${this.label}] retry scheduled`, { key: attempt.key, failures: attempt.failures, delayMs })
    attempt.retryTimerId = window.setTimeout(() => {
      attempt.retryTimerId = null
      this.onRetryDue()
    }, delayMs)
  }

  retain(keys: Set<string>): void {
    for (const attempt of this.attempts.values()) {
      if (!keys.has(attempt.key)) {
        this.forget(attempt)
      }
    }
  }

  private forget(attempt: RetryAttempt): void {
    if (attempt.retryTimerId !== null) {
      window.clearTimeout(attempt.retryTimerId)
    }
    this.attempts.delete(attempt.key)
  }
}

export function useRetryBackoff(label: string): { backoff: RetryBackoff; retryTick: number } {
  const [retryTick, setRetryTick] = useState(0)
  const [backoff] = useState(() => new RetryBackoff(label, () => setRetryTick((tick) => tick + 1)))
  return { backoff, retryTick }
}
