import { expect, test } from '@playwright/test'
import type { PcmBatch } from './pcmCapture'

type Processor = {
  port: { onmessage: ((event: { data: unknown }) => void) | null }
  process: (inputs: Float32Array[][]) => boolean
}

const SAMPLE_RATE = 48_000
const QUANTUM_FRAMES = 128

const scope = globalThis as Record<string, unknown>
let registered: (new () => Processor) | undefined
scope.sampleRate = SAMPLE_RATE
scope.AudioWorkletProcessor = class {
  port = { onmessage: null }
}
scope.registerProcessor = (_name: string, processor: new () => Processor) => {
  registered = processor
}
const loaded = import('./pcmCaptureWorklet.js')

async function pcmCapture(): Promise<{ processor: Processor; batches: PcmBatch[]; advance: (frames: number) => void }> {
  await loaded
  scope.currentFrame = 0
  const processor = new registered!()
  const batches: PcmBatch[] = []
  processor.port.onmessage?.({ data: { postMessage: (batch: PcmBatch) => batches.push(batch) } })
  return {
    processor,
    batches,
    advance: (frames) => {
      scope.currentFrame = (scope.currentFrame as number) + frames
    }
  }
}

function quantum(firstSample: number, channels: number): Float32Array[] {
  return Array.from({ length: channels }, (_, channel) =>
    Float32Array.from({ length: QUANTUM_FRAMES }, (_, index) => channel * 1000 + firstSample + index)
  )
}

test('the input goes out in planar 10 ms batches stamped with their first frame', async () => {
  // Arrange
  const { processor, batches, advance } = await pcmCapture()

  // Act: 4 render quanta are 512 frames, one 480-frame batch and 32 frames over
  for (let index = 0; index < 4; index++) {
    processor.process([quantum(index * QUANTUM_FRAMES, 2)])
    advance(QUANTUM_FRAMES)
  }

  // Assert
  expect(
    batches.map(({ timestampMicros, numberOfFrames, numberOfChannels }) => [
      timestampMicros,
      numberOfFrames,
      numberOfChannels
    ])
  ).toEqual([[0, 480, 2]])
  const [{ data }] = batches
  expect([data[0], data[479], data[480], data[959]]).toEqual([0, 479, 1000, 1479])
})

test('the next batch starts at the frame after the last one sent', async () => {
  // Arrange
  const { processor, batches, advance } = await pcmCapture()

  // Act: 8 quanta are 1024 frames, two batches
  for (let index = 0; index < 8; index++) {
    processor.process([quantum(index * QUANTUM_FRAMES, 1)])
    advance(QUANTUM_FRAMES)
  }

  // Assert: the second batch starts at frame 480, 10 ms in
  expect(batches.map(({ timestampMicros }) => timestampMicros)).toEqual([0, 10_000])
  expect(batches[1].data[0]).toBe(480)
})
