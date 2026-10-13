import { expect } from '@playwright/test'

export async function delayDjevAnswers(djevOrigin: string, milliseconds: number): Promise<void> {
  const response = await fetch(`${djevOrigin}/delay`, { method: 'PUT', body: String(milliseconds) })
  expect(response.ok).toBe(true)
}
