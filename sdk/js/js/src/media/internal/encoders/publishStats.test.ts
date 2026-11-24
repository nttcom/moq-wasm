import { expect, test } from '@playwright/test'
import { AudioEncodeStats, VideoEncodeStats } from './publishStats'

function videoChunk(byteLength: number): EncodedVideoChunk {
  return { byteLength } as EncodedVideoChunk
}

function sizedMetadata(width: number, height: number): EncodedVideoChunkMetadata {
  return { decoderConfig: { codec: 'avc1.42e01f', codedWidth: width, codedHeight: height } }
}

test('the rates count only the chunks of the last second', () => {
  // Arrange: 1000 bytes leave the window; two chunks of 500 bytes stay in it
  const stats = new AudioEncodeStats()
  stats.add({ byteLength: 1000 } as EncodedAudioChunk, 0)
  stats.add({ byteLength: 500 } as EncodedAudioChunk, 600)
  stats.add({ byteLength: 500 } as EncodedAudioChunk, 1200)

  // Act
  const audio = stats.stats(1500)

  // Assert
  expect(audio).toMatchObject({ encodedKbps: 8, encodedFramesPerSecond: 2, encoderErrors: 0 })
})

test('the audio input counts the frames of the last second and keeps the latest format', () => {
  // Arrange: the 44.1 kHz stereo frame leaves the window; two mono 48 kHz frames stay in it
  const stats = new AudioEncodeStats()
  stats.addInput({ sampleRate: 44100, numberOfChannels: 2, level: 10 }, 0)
  stats.addInput({ sampleRate: 48000, numberOfChannels: 1, level: 30 }, 600)
  stats.addInput({ sampleRate: 48000, numberOfChannels: 1, level: 127 }, 1200)

  // Act
  const { input } = stats.stats(1500)

  // Assert: the mean is of the power, so the silent frame lowers it by about 3 dB
  expect(input).toMatchObject({ framesPerSecond: 2, sampleRate: 48000, numberOfChannels: 1, minLevelDbov: -127 })
  expect(input.meanLevelDbov).toBeCloseTo(-33.01, 2)
})

test('an audio input without a frame in the last second has no level', () => {
  // Arrange
  const stats = new AudioEncodeStats()
  stats.addInput({ sampleRate: 48000, numberOfChannels: 1, level: 30 }, 0)

  // Act
  const { input } = stats.stats(2000)

  // Assert: the format stays the last one seen
  expect(input).toEqual({
    framesPerSecond: 0,
    sampleRate: 48000,
    numberOfChannels: 1,
    minLevelDbov: undefined,
    meanLevelDbov: undefined
  })
})

test('the frame size is the last one an encoder configuration reported', () => {
  // Arrange
  const stats = new VideoEncodeStats([])
  stats.add(0, videoChunk(100), sizedMetadata(1280, 720), 0)
  stats.add(0, videoChunk(100), undefined, 10)

  // Act
  const video = stats.stats(20)

  // Assert
  expect(video.frameSize).toEqual({ width: 1280, height: 720 })
})

test('each rendition reports its own chunks under its name', () => {
  // Arrange
  const stats = new VideoEncodeStats(['low'])
  stats.add(0, videoChunk(1000), sizedMetadata(1280, 720), 0)
  stats.add(1, videoChunk(250), sizedMetadata(320, 180), 0)
  stats.droppedFrames = 3

  // Act
  const video = stats.stats(500)

  // Assert
  expect(video).toEqual({
    encodedKbps: 8,
    encodedFramesPerSecond: 1,
    frameSize: { width: 1280, height: 720 },
    droppedFrames: 3,
    encoderErrors: 0,
    renditions: { low: { encodedKbps: 2, encodedFramesPerSecond: 1, frameSize: { width: 320, height: 180 } } }
  })
})
