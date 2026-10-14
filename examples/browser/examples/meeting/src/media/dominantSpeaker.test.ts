import { expect, test } from '@playwright/test'
import { DominantSpeaker } from './dominantSpeaker'

const LOUD = { level: 20, voiceActivity: true }
const SOFT = { level: 40, voiceActivity: true }
const SILENT = { level: 90, voiceActivity: false }

test('the loudest member speaking takes the focus', () => {
  // Arrange
  const speakers = new DominantSpeaker()
  speakers.observe('alice', SOFT, 0)
  speakers.observe('bob', LOUD, 0)

  // Act / Assert
  expect(speakers.current(100)).toBe('bob')
})

test('the speaker keeps the focus through a short pause while another member speaks', () => {
  // Arrange
  const speakers = new DominantSpeaker()
  speakers.observe('alice', SOFT, 0)
  speakers.current(0)
  speakers.observe('bob', LOUD, 1_000)

  // Act / Assert
  expect(speakers.current(1_000)).toBe('alice')
})

test('another member takes the focus once the speaker has been quiet long enough', () => {
  // Arrange
  const speakers = new DominantSpeaker()
  speakers.observe('alice', SOFT, 0)
  speakers.current(0)
  speakers.observe('alice', SILENT, 1_000)
  speakers.observe('bob', SOFT, 1_600)

  // Act / Assert
  expect(speakers.current(1_600)).toBe('bob')
})

test('the last speaker keeps the focus while nobody speaks', () => {
  // Arrange
  const speakers = new DominantSpeaker()
  speakers.observe('alice', SOFT, 0)
  speakers.current(0)

  // Act / Assert
  expect(speakers.current(10_000)).toBe('alice')
  expect(speakers.speaking(10_000).size).toBe(0)
})
