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
  expect(audio).toEqual({ encodedKbps: 8, encodedFramesPerSecond: 2, encoderErrors: 0 })
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
