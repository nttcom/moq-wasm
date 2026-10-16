import { expect, test } from '@playwright/test'
import { insertRepetition } from './audioSplice'

const SAMPLE_RATE = 48_000
const PERIOD_SAMPLES = 100

function sine(length: number): Float32Array<ArrayBuffer> {
  return Float32Array.from({ length }, (_, index) => Math.sin((2 * Math.PI * index) / PERIOD_SAMPLES))
}

test('inserting a repetition into a periodic wave extends it by whole periods', () => {
  // Arrange
  const channels = [sine(960)]

  // Act
  const stretched = insertRepetition(channels, SAMPLE_RATE, 480)

  // Assert
  expect(stretched).toBeDefined()
  const added = stretched![0].length - 960
  expect(added).toBeGreaterThan(0)
  expect(added % PERIOD_SAMPLES).toBe(0)
  const expected = sine(stretched![0].length)
  for (let index = 0; index < expected.length; index += 1) {
    expect(Math.abs(stretched![0][index] - expected[index])).toBeLessThan(0.05)
  }
})

test('nothing is inserted when fewer samples than a pitch period may be added', () => {
  // Act / Assert
  expect(insertRepetition([sine(960)], SAMPLE_RATE, 100)).toBeUndefined()
})
