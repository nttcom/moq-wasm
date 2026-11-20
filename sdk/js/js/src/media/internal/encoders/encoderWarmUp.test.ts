import { expect, test } from '@playwright/test'
import { EncoderWarmUp, type WarmableEncoder } from './encoderWarmUp'

class FakeEncoder implements WarmableEncoder {
  readonly warmedUpOn: VideoFrame[] = []
  private settle: { resolve: () => void; reject: (error: Error) => void } | undefined

  warmUp(frame: VideoFrame): Promise<void> {
    this.warmedUpOn.push(frame)
    return new Promise((resolve, reject) => {
      this.settle = { resolve, reject }
    })
  }

  async flushed(): Promise<void> {
    this.settle!.resolve()
    await Promise.resolve()
    await Promise.resolve()
  }

  async failed(error: Error): Promise<void> {
    this.settle!.reject(error)
    await Promise.resolve()
    await Promise.resolve()
  }
}

function frame(): VideoFrame {
  return {} as VideoFrame
}

function newWarmUp(): { warmUp: EncoderWarmUp; failures: unknown[] } {
  const failures: unknown[] = []
  return { warmUp: new EncoderWarmUp((error) => failures.push(error)), failures }
}

test('the first frame warms every encoder up and is not encoded live', () => {
  // Arrange
  const encoders = [new FakeEncoder(), new FakeEncoder()]
  const { warmUp } = newWarmUp()
  const first = frame()

  // Act
  const admitted = warmUp.admit(first, encoders)

  // Assert
  expect(admitted).toBe(false)
  expect(encoders.map((encoder) => encoder.warmedUpOn)).toEqual([[first], [first]])
})

test('frames read while the encoders warm up are dropped without another warm-up', () => {
  // Arrange
  const encoders = [new FakeEncoder()]
  const { warmUp } = newWarmUp()
  warmUp.admit(frame(), encoders)

  // Act
  const admitted = [warmUp.admit(frame(), encoders), warmUp.admit(frame(), encoders)]

  // Assert
  expect(admitted).toEqual([false, false])
  expect(encoders[0].warmedUpOn).toHaveLength(1)
})

test('frames are admitted once every encoder has flushed its warm-up frame', async () => {
  // Arrange
  const encoders = [new FakeEncoder(), new FakeEncoder()]
  const { warmUp } = newWarmUp()
  warmUp.admit(frame(), encoders)
  await encoders[0].flushed()
  const whileOneWarms = warmUp.admit(frame(), encoders)

  // Act
  await encoders[1].flushed()
  const afterBoth = warmUp.admit(frame(), encoders)

  // Assert
  expect(whileOneWarms).toBe(false)
  expect(afterBoth).toBe(true)
})

test('a failed warm-up is reported and stops holding frames back', async () => {
  // Arrange
  const encoders = [new FakeEncoder()]
  const { warmUp, failures } = newWarmUp()
  warmUp.admit(frame(), encoders)

  // Act
  await encoders[0].failed(new Error('encoder closed'))

  // Assert
  expect(failures).toEqual([new Error('encoder closed')])
  expect(warmUp.admit(frame(), encoders)).toBe(true)
})

test('a reset during a warm-up makes the next frame warm the new encoders up', async () => {
  // Arrange
  const previous = [new FakeEncoder()]
  const recreated = [new FakeEncoder()]
  const { warmUp } = newWarmUp()
  warmUp.admit(frame(), previous)
  warmUp.reset()
  const next = frame()

  // Act
  const admitted = warmUp.admit(next, recreated)
  await previous[0].flushed()

  // Assert: the stale warm-up does not admit frames to the new encoders
  expect(admitted).toBe(false)
  expect(recreated[0].warmedUpOn).toEqual([next])
  expect(warmUp.admit(frame(), recreated)).toBe(false)
})
