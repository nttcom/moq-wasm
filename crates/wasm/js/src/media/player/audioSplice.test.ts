import { expect, test } from '@playwright/test'
import { insertRepetition, lastPeriod, repeatPeriod } from './audioSplice'

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

test('repeating the last pitch period of a periodic wave continues the wave', () => {
  // Arrange
  const length = 960
  const period = lastPeriod([sine(length)], SAMPLE_RATE)!

  // Act
  const continued = repeatPeriod(period, 0, 480, Number.POSITIVE_INFINITY)

  // Assert
  expect(period[0].length % PERIOD_SAMPLES).toBe(0)
  const expected = sine(length + 480).subarray(length)
  for (let index = 0; index < expected.length; index += 1) {
    expect(Math.abs(continued[0][index] - expected[index])).toBeLessThan(0.05)
  }
})

test('the end of a wave that does not repeat has no period to continue with', () => {
  // Arrange
  let seed = 1
  const noise = Float32Array.from({ length: 960 }, () => {
    seed = (seed * 16_807) % 2_147_483_647
    return seed / 1_073_741_823.5 - 1
  })

  // Act / Assert
  expect(lastPeriod([noise], SAMPLE_RATE)).toBeUndefined()
})
