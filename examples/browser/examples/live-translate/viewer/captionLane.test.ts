import { expect, test } from '@playwright/test'
import { CaptionLane } from './captionLane'

const MICROS_PER_MILLI = 1_000

function piece(text: string, final = false): Uint8Array {
  return new TextEncoder().encode(JSON.stringify({ text, final }))
}

test('pieces build a line until a final one, a late piece is dropped, and closing clears the line', async () => {
  // Arrange: a capture time is its distance from now, so 0 is due at once
  const captions: string[] = []
  const playout = { dueAt: (captureMicros: number) => performance.now() + captureMicros / MICROS_PER_MILLI }
  const lane = new CaptionLane(playout, (text) => captions.push(text))

  // Act
  lane.push(piece('こん'), 0)
  lane.push(piece('にちは', true), 0)
  lane.push(piece('次'), 0)
  lane.push(piece('遅い'), -4_000 * MICROS_PER_MILLI)
  await new Promise((resolve) => setTimeout(resolve, 20))
  lane.close()

  // Assert
  expect(captions).toEqual(['こん', 'こんにちは', '次', ''])
})
