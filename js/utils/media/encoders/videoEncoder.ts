import { createBitrateLogger } from '../bitrate'
import { monotonicUnixMicros } from '../clock'

let videoEncoder: VideoEncoder | undefined
let keyframeInterval: number
let encoderConfig: VideoEncoderConfig | null = null
let timestampOffset: number | null = null
const captureTimestampByChunkTimestamp = new Map<number, number>()

// H264 プロファイル
// Baseline: 42 Main: 4D High: 64
// H264 Level
// 4.0: 28 4.1: 29 4.2: 2A
// 5.0: 32 5.1: 33 5.2: 34
// M1 macではHWEncoderがL1T1しかサポートしていない

// const VIDEO_ENCODER_CONFIG = {
//   codec: 'av01.0.08M.08',
//   width: 1280,
//   height: 720,
//   bitrate: 1_000_000, //10 Mbps
//   // scalabilityMode: 'L1T3',
//   scalabilityMode: 'L1T1',
//   framerate: 30
// }

const videoBitrateLogger = createBitrateLogger((kbps) => {
  self.postMessage({ type: 'bitrate', kbps })
})

function sendVideoChunkMessage(chunk: EncodedVideoChunk, metadata: EncodedVideoChunkMetadata | undefined) {
  videoBitrateLogger.addBytes(chunk.byteLength)

  // 最初のチャンクでtimestampのoffsetを保存
  if (timestampOffset === null) {
    timestampOffset = chunk.timestamp
    console.info('[videoEncoder] Set timestamp offset:', timestampOffset)
  }

  // timestampを0起点に調整したチャンクを作成
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
    console.error('[videoEncoder] configure failed', e)
    self.postMessage({ type: 'configError', reason: 'unsupported', config })
    return undefined
  }
  console.info('[videoEncoder] initialized', config)
  return encoder
}

async function startVideoEncode(videoReadableStream: ReadableStream<VideoFrame>) {
  let frameCounter = 0
  // 新しいストリーム開始時にtimestampのoffsetをリセット
  timestampOffset = null
  captureTimestampByChunkTimestamp.clear()
  console.log('initializeVideoEncoder')
  videoEncoder = await initializeVideoEncoder()
  if (!videoEncoder) {
    return
  }
  const videoReader = videoReadableStream.getReader()
  while (true) {
    const videoResult = await videoReader.read()
    if (videoResult.done) break
    const videoFrame = videoResult.value

    // Check if encoder needs re-initialization
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

    // Too many frames in flight, encoder is overwhelmed. let's drop this frame.
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
  console.debug('videoEncoder worker received message', event.data)
  if (event.data.type === 'keyframeInterval') {
    keyframeInterval = event.data.keyframeInterval
  } else if (event.data.type === 'encoderConfig') {
    const newConfig = buildConfigFromMessage(event.data.config)
    encoderConfig = newConfig
    if (videoEncoder && videoEncoder.state !== 'closed') {
      try {
        videoEncoder.configure(newConfig)
        console.info('[videoEncoder] reconfigured', newConfig)
      } catch (e) {
        console.error('[videoEncoder] reconfigure failed', e)
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
    avc: config.codec.startsWith('avc') ? { format: 'annexb' } : undefined,
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
