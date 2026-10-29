import { expect } from '@playwright/test'

export async function delaySystemOneAnswers(systemOneOrigin: string, milliseconds: number): Promise<void> {
  const response = await fetch(`${systemOneOrigin}/delay`, { method: 'PUT', body: String(milliseconds) })
  expect(response.ok).toBe(true)
}
