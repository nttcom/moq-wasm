/// Calls a callback of the page and returns its result. What it throws is the
/// page's bug: it is reported with `reportError`, as an event listener's
/// exception is, and `undefined` is returned so the SDK goes on.
export function callPage<T>(callback: () => T): T | undefined {
  try {
    return callback()
  } catch (error) {
    reportError(error)
    return undefined
  }
}
