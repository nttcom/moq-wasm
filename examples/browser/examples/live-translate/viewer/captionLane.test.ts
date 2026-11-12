import { expect, test } from '@playwright/test'
import { CaptionLane } from './captionLane'

function piece(text: string, final = false): string {
  return JSON.stringify({ text, final })
}

test('pieces build a line until a final one, and closing clears the line', () => {
  // Arrange
  const captions: string[] = []
  const lane = new CaptionLane((text) => captions.push(text))

  // Act
  lane.push(piece('こん'))
  lane.push(piece('にちは', true))
  lane.push(piece('次'))
  lane.close()
  lane.push(piece('閉じた後'))

  // Assert
  expect(captions).toEqual(['こん', 'こんにちは', '次', ''])
})
