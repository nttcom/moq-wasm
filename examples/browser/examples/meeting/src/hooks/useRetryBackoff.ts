import { useState } from 'react'

const BASE_RETRY_DELAY_MS = 500
const MAX_FAILURES = 6

type RetryAttempt = {
  key: string
  failures: number
  waiting: boolean
}

class RetryBackoff {
  private readonly attempts = new Map<string, RetryAttempt>()

  constructor(
    private readonly label: string,
    private readonly onRetryDue: () => void
  ) {}

  run(key: string, task: () => Promise<void>): void {
    const attempt = this.begin(key)
    if (!attempt) {
      return
    }
    task()
      .then(() => this.succeed(attempt))
      .catch((error) => {
        console.error(`[meeting][${this.label}] failed`, { key, error })
        this.fail(attempt)
      })
  }

  retain(keys: ReadonlyMap<string, unknown>): void {
    for (const key of this.attempts.keys()) {
      if (!keys.has(key)) {
        this.attempts.delete(key)
      }
    }
  }

  private begin(key: string): RetryAttempt | null {
    const existing = this.attempts.get(key)
    if (existing && (existing.waiting || existing.failures >= MAX_FAILURES)) {
      return null
    }
    const attempt = existing ?? { key, failures: 0, waiting: false }
    attempt.waiting = true
    this.attempts.set(key, attempt)
    return attempt
  }

  private succeed(attempt: RetryAttempt): void {
    if (this.attempts.get(attempt.key) === attempt) {
      this.attempts.delete(attempt.key)
    }
  }

  private fail(attempt: RetryAttempt): void {
    if (this.attempts.get(attempt.key) !== attempt) {
      return
    }
    attempt.failures += 1
    if (attempt.failures >= MAX_FAILURES) {
      console.warn(`[meeting][${this.label}] giving up`, { key: attempt.key, failures: attempt.failures })
      return
    }
    const delayMs = BASE_RETRY_DELAY_MS * 2 ** (attempt.failures - 1)
    console.info(`[meeting][${this.label}] retry scheduled`, { key: attempt.key, failures: attempt.failures, delayMs })
    window.setTimeout(() => {
      attempt.waiting = false
      this.onRetryDue()
    }, delayMs)
  }
}

export function useRetryBackoff(label: string): { backoff: RetryBackoff; retryTick: number } {
  const [retryTick, setRetryTick] = useState(0)
  const [backoff] = useState(() => new RetryBackoff(label, () => setRetryTick((tick) => tick + 1)))
  return { backoff, retryTick }
}
