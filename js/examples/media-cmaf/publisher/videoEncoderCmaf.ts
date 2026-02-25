/**
 * Video encoder worker for CMAF packaging.
 * Based on utils/media/encoders/videoEncoder.ts with one key change:
 *   avc format: 'avc' (AVCC / length-prefixed NALUs) instead of 'annexb'
 * This is required by mp4box.js for muxing into CMAF chunks.
 */
import { createBitrateLogger } from '../../../utils/media/bitrate'
import { monotonicUnixMicros } from '../../../utils/media/clock'

let videoEncoder: VideoEncoder | undefined
let keyframeInterval: number
let encoderConfig: VideoEncoderConfig | null = null
let timestampOffset: number | null = null
const captureTimestampByChunkTimestamp = new Map<number, number>()

const videoBitrateLogger = createBitrateLogger((kbps) => {
  self.postMessage({ type: 'bitrate', kbps })
})

function sendVideoChunkMessage(chunk: EncodedVideoChunk, metadata: EncodedVideoChunkMetadata | undefined) {
  videoBitrateLogger.addBytes(chunk.byteLength)

  if (timestampOffset === null) {
    timestampOffset = chunk.timestamp
    console.info('[videoEncoderCmaf] Set timestamp offset:', timestampOffset)
  }

  const adjustedTimestamp = chunk.timestamp - timestampOffset
  const buffer = new ArrayBuffer(chunk.byteLength)
  chunk.copyTo(buffer)
  const adjustedChunk = new EncodedVideoChunk({
    type: chunk.type,
    timestamp: adjustedTimestamp,
    duration: chunk.duration ?? undefined,
    data: buffer
  })

  const captureTimestampMicros = takeCaptureTimestampMicros(chunk.timestamp)
  self.postMessage({ type: 'chunk', chunk: adjustedChunk, metadata, captureTimestampMicros })
}

async function initializeVideoEncoder() {
  const init: VideoEncoderInit = {
    output: sendVideoChunkMessage,
    error: (e: any) => {
      console.log(e.message)
    }
  }
  const config = encoderConfig ?? buildDefaultConfig()
  try {
    const supported = await VideoEncoder.isConfigSupported(config)
    if (!supported.supported) {
      self.postMessage({ type: 'configError', reason: 'unsupported', config })
      return undefined
    }
  } catch (e) {
    self.postMessage({ type: 'configError', reason: 'unsupported', config })
    return undefined
  }
  const encoder = new VideoEncoder(init)
  try {
    encoder.configure(config)
  } catch (e) {
    console.error('[videoEncoderCmaf] configure failed', e)
    self.postMessage({ type: 'configError', reason: 'unsupported', config })
    return undefined
  }
  console.info('[videoEncoderCmaf] initialized', config)
  return encoder
}

async function startVideoEncode(videoReadableStream: ReadableStream<VideoFrame>) {
  let frameCounter = 0
  timestampOffset = null
  captureTimestampByChunkTimestamp.clear()
  console.log('initializeVideoEncoder (CMAF)')
  videoEncoder = await initializeVideoEncoder()
  if (!videoEncoder) {
    return
  }
  const videoReader = videoReadableStream.getReader()
  while (true) {
    const videoResult = await videoReader.read()
    if (videoResult.done) break
    const videoFrame = videoResult.value

    if (!videoEncoder || videoEncoder.state === 'closed') {
      console.log('Re-initialize video encoder')
      videoEncoder = await initializeVideoEncoder()
      frameCounter = 0
      captureTimestampByChunkTimestamp.clear()
      if (!videoEncoder) {
        console.error('Failed to initialize video encoder, dropping frame')
        videoFrame.close()
        continue
      }
    }

    if (videoEncoder.encodeQueueSize > 10) {
      console.error('videoEncoder.encodeQueueSize > 10', videoEncoder.encodeQueueSize)
      videoFrame.close()
      continue
    }

    const keyFrame = frameCounter % keyframeInterval == 0
    setCaptureTimestampMicros(videoFrame.timestamp, monotonicUnixMicros())
    videoEncoder.encode(videoFrame, { keyFrame })
    frameCounter++
    videoFrame.close()
  }
}

function setCaptureTimestampMicros(timestamp: number | null, captureTimestampMicros: number): void {
  if (typeof timestamp !== 'number' || !Number.isFinite(timestamp)) {
    return
  }
  captureTimestampByChunkTimestamp.set(timestamp, captureTimestampMicros)
  if (captureTimestampByChunkTimestamp.size > 1024) {
    const oldestKey = captureTimestampByChunkTimestamp.keys().next().value
    if (typeof oldestKey === 'number') {
      captureTimestampByChunkTimestamp.delete(oldestKey)
    }
  }
}

function takeCaptureTimestampMicros(timestamp: number): number | undefined {
  if (!Number.isFinite(timestamp)) {
    return undefined
  }
  const value = captureTimestampByChunkTimestamp.get(timestamp)
  if (value !== undefined) {
    captureTimestampByChunkTimestamp.delete(timestamp)
    return value
  }
  return undefined
}

self.onmessage = async (event) => {
  console.debug('videoEncoderCmaf worker received message', event.data)
  if (event.data.type === 'keyframeInterval') {
    keyframeInterval = event.data.keyframeInterval
  } else if (event.data.type === 'encoderConfig') {
    const newConfig = buildConfigFromMessage(event.data.config)
    encoderConfig = newConfig
    if (videoEncoder && videoEncoder.state !== 'closed') {
      try {
        videoEncoder.configure(newConfig)
        console.info('[videoEncoderCmaf] reconfigured', newConfig)
      } catch (e) {
        console.error('[videoEncoderCmaf] reconfigure failed', e)
        self.postMessage({ type: 'configError', reason: 'reconfigure_failed', config: newConfig })
      }
    }
  } else if (event.data.type === 'videoStream') {
    const videoReadableStream: ReadableStream<VideoFrame> = event.data.videoStream
    if (!videoReadableStream) {
      console.error('MediaStreamTrack が渡されていません')
      return
    }
    await startVideoEncode(videoReadableStream)
  }
}

function buildConfigFromMessage(config: {
  codec: string
  width: number
  height: number
  bitrate: number
}): VideoEncoderConfig {
  return {
    codec: config.codec,
    // CMAF requires AVCC format (length-prefixed NALUs), not Annex B
    avc: config.codec.startsWith('avc') ? { format: 'avc' } : undefined,
    width: config.width,
    height: config.height,
    bitrate: config.bitrate,
    framerate: 30,
    scalabilityMode: 'L1T1',
    latencyMode: 'realtime' as any
  }
}

function buildDefaultConfig(): VideoEncoderConfig {
  return buildConfigFromMessage({
    codec: 'avc1.640032',
    width: 1280,
    height: 720,
    bitrate: 1_000_000
  })
}
