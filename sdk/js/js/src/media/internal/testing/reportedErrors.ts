import { test } from '@playwright/test'

/// Stands in for `reportError`, a browser global the unit tests' Node may lack,
/// and returns what `callPage` reports in each test of the calling file,
/// emptied before each test.
export function collectReportedErrors(): unknown[] {
  const reported: unknown[] = []
  let original: PropertyDescriptor | undefined
  test.beforeEach(() => {
    reported.length = 0
    original = Object.getOwnPropertyDescriptor(globalThis, 'reportError')
    globalThis.reportError = (error: unknown) => reported.push(error)
  })
  test.afterEach(() => {
    if (original) {
      Object.defineProperty(globalThis, 'reportError', original)
    } else {
      Reflect.deleteProperty(globalThis, 'reportError')
    }
  })
  return reported
}
