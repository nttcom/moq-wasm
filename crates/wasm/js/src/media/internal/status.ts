export type StatusState = 'idle' | 'ok' | 'review' | 'error'

export function getErrorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message
  }
  if (typeof error === 'string') {
    return error
  }
  try {
    return JSON.stringify(error)
  } catch (_error) {
    return String(error)
  }
}
