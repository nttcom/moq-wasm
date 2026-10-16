import { expect } from '@playwright/test'

export async function delayD3FlashAnswers(d3FlashOrigin: string, milliseconds: number): Promise<void> {
  const response = await fetch(`${d3FlashOrigin}/delay`, { method: 'PUT', body: String(milliseconds) })
  expect(response.ok).toBe(true)
}
