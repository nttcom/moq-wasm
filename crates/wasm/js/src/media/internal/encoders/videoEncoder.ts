import { createBitrateLogger } from '../bitrate'
import { monotonicUnixMicros } from '../clock'
import { CaptureTimestamps } from './captureTimestamps'
import { fitEncodeSize } from './encodeSize'

/// The track a `videoTrack` message transferred; `stop` stops it.
let capturedTrack: MediaStreamTrack | undefined
// Default keeps periodic keyframes alive even before the first keyframeInterval message.
let keyframeInterval: number = 30
let forceNextKeyframe = false

// H264 プロファイル
// Baseline: 42 Main: 4D High: 64
// H264 Level
// 4.0: 28 4.1: 29 4.2: 2A
// 5.0: 32 5.1: 33 5.2: 34
// M1 macではHWEncoderがL1T1しかサポートしていない

const videoBitrateLogger = createBitrateLogger((kbps) => {
  self.postMessage({ type: 'bitrate', kbps })
})

/// One encoder of the stream's frames. Rendition 0 takes `encoderConfig`;
/// `renditionConfigs`, which must arrive before `videoStream`, adds the others.
/// All encode the same frames with the same keyframes, so the outputs of one
/// frame share a timestamp.
class Rendition {
  encoder: VideoEncoder | undefined
  // null makes the next frame reconfigure the encoder.
  configuredFrameSize: string | null = null
  readonly captureTimestamps = new CaptureTimestamps()

  constructor(
    readonly index: number,
    public config: VideoEncoderConfig | null
  ) {}

  async initialize(): Promise<boolean> {
    const init: VideoEncoderInit = {
      output: (chunk, metadata) => this.send(chunk, metadata),
      error: (e: DOMException) => {
        console.error('[videoEncoder] encoder error', e)
        self.postMessage({ type: 'encoderError', message: e.message, rendition: this.index })
      }
    }
    const config = this.config ?? buildDefaultConfig()
    try {
      const supported = await VideoEncoder.isConfigSupported(config)
      if (!supported.supported) {
        console.error('[videoEncoder] unsupported encoder config', config)
        self.postMessage({ type: 'configError', reason: 'unsupported', config, rendition: this.index })
        return false
      }
    } catch (e) {
      console.error('[videoEncoder] failed to check encoder config support', e, config)
      self.postMessage({ type: 'configError', reason: 'unsupported', config, rendition: this.index })
      return false
    }
    const encoder = new VideoEncoder(init)
    try {
      encoder.configure(config)
    } catch (e) {
      console.error('[videoEncoder] configure failed', e)
      self.postMessage({ type: 'configError', reason: 'unsupported', config, rendition: this.index })
      return false
    }
    this.encoder = encoder
    this.configuredFrameSize = null
    return true
  }

  /// Whether the encoder was reconfigured for a new frame size, which needs a keyframe.
  configureForFrame(frame: VideoFrame): boolean {
    const frameSize = `${frame.displayWidth}x${frame.displayHeight}`
    if (!this.encoder || frameSize === this.configuredFrameSize) {
      return false
    }
    this.configuredFrameSize = frameSize
    const requested = this.config ?? buildDefaultConfig()
    const config = {
      ...requested,
      ...fitEncodeSize(requested, { width: frame.displayWidth, height: frame.displayHeight })
    }
    try {
      this.encoder.configure(config)
    } catch (e) {
      console.error('[videoEncoder] reconfigure failed', e)
      self.postMessage({ type: 'configError', reason: 'reconfigure_failed', config, rendition: this.index })
      return false
    }
    return true
  }

  private send(chunk: EncodedVideoChunk, metadata: EncodedVideoChunkMetadata | undefined) {
    videoBitrateLogger.addBytes(chunk.byteLength)
    const { init, captureTimestampMicros } = this.captureTimestamps.restamp(chunk)
    const encodeDoneTimestampMicros = monotonicUnixMicros()
    self.postMessage({
      type: 'chunk',
      chunk: new EncodedVideoChunk(init),
      metadata,
      captureTimestampMicros,
      encodeDoneTimestampMicros,
      encodeQueueSize: this.encoder?.encodeQueueSize ?? 0,
      rendition: this.index
    })
  }
}

let renditions: Rendition[] = [new Rendition(0, null)]

function needsEncoder(rendition: Rendition): boolean {
  return !rendition.encoder || rendition.encoder.state === 'closed'
}

/// Whether rendition 0 has an encoder. A rendition whose encoder fails is
/// dropped, after its `configError`, so the video goes on without it.
async function initializeEncoders(): Promise<boolean> {
  const pending = renditions.filter(needsEncoder)
  const initialized = await Promise.all(pending.map((rendition) => rendition.initialize()))
  const failed = new Set(pending.filter((_, index) => !initialized[index]))
  renditions = renditions.filter((rendition) => rendition.index === 0 || !failed.has(rendition))
  return !needsEncoder(renditions[0])
}

async function startVideoEncode(videoReadableStream: ReadableStream<VideoFrame>) {
  let frameCounter = 0
  for (const rendition of renditions) {
    if (rendition.encoder && rendition.encoder.state !== 'closed') {
      rendition.encoder.close()
    }
    rendition.encoder = undefined
    rendition.captureTimestamps.reset()
  }
  if (!(await initializeEncoders())) {
    return
  }
  const videoReader = videoReadableStream.getReader()
  while (true) {
    let videoResult: ReadableStreamReadResult<VideoFrame>
    try {
      videoResult = await videoReader.read()
    } catch (e) {
      self.postMessage({ type: 'streamEnded', error: e instanceof Error ? e.message : String(e) })
      return
    }
    if (videoResult.done) {
      self.postMessage({ type: 'streamEnded' })
      return
    }
    const videoFrame = videoResult.value

    if (renditions.some(needsEncoder)) {
      console.log('Re-initialize video encoder')
      frameCounter = 0
      if (!(await initializeEncoders())) {
        console.error('Failed to initialize video encoder, dropping frame')
        videoFrame.close()
        continue
      }
    }

    // A frame any encoder cannot take is dropped for all, so the renditions keep the same frames.
    const busiest = Math.max(...renditions.map((rendition) => rendition.encoder?.encodeQueueSize ?? 0))
    if (busiest > 5) {
      console.error('videoEncoder.encodeQueueSize > 5', busiest)
      videoFrame.close()
      continue
    }

    let reconfigured = false
    for (const rendition of renditions) {
      reconfigured = rendition.configureForFrame(videoFrame) || reconfigured
    }
    const forced = forceNextKeyframe || reconfigured
    const keyFrame = forced || frameCounter % keyframeInterval === 0
    if (forced) {
      forceNextKeyframe = false
      frameCounter = 0
    }
    for (const rendition of renditions) {
      rendition.captureTimestamps.recordCapture(videoFrame.timestamp)
      rendition.encoder?.encode(videoFrame, { keyFrame })
    }
    frameCounter++
    videoFrame.close()
  }
}

self.onmessage = async (event) => {
  if (event.data.type === 'keyframeInterval') {
    keyframeInterval = event.data.keyframeInterval
  } else if (event.data.type === 'forceKeyframe') {
    forceNextKeyframe = true
  } else if (event.data.type === 'encoderConfig') {
    renditions[0].config = buildConfigFromMessage(event.data.config)
    renditions[0].configuredFrameSize = null
  } else if (event.data.type === 'renditionConfigs') {
    const configs: Parameters<typeof buildConfigFromMessage>[0][] = event.data.configs
    renditions = [
      renditions[0],
      ...configs.map((config, index) => new Rendition(index + 1, buildConfigFromMessage(config)))
    ]
  } else if (event.data.type === 'videoStream') {
    const videoReadableStream: ReadableStream<VideoFrame> = event.data.videoStream
    if (!videoReadableStream) {
      console.error('MediaStreamTrack が渡されていません')
      return
    }
    await startVideoEncode(videoReadableStream)
  } else if (event.data.type === 'videoTrack') {
    // A page without MediaStreamTrackProcessor (Safari) transfers the track to read it here.
    capturedTrack = event.data.track
    await startVideoEncode(new MediaStreamTrackProcessor({ track: event.data.track }).readable)
  } else if (event.data.type === 'stop') {
    capturedTrack?.stop()
    self.close()
  }
}

function buildConfigFromMessage(config: {
  codec: string
  width: number
  height: number
  bitrate: number
  framerate?: number
  hardwareAcceleration?: HardwareAcceleration
}): VideoEncoderConfig {
  return {
    codec: config.codec,
    avc: config.codec.startsWith('avc') ? { format: 'annexb' } : undefined,
    width: config.width,
    height: config.height,
    bitrate: config.bitrate,
    framerate:
      typeof config.framerate === 'number' && Number.isFinite(config.framerate) && config.framerate > 0
        ? Math.min(120, Math.max(1, Math.round(config.framerate)))
        : 30,
    hardwareAcceleration: config.hardwareAcceleration ?? 'prefer-hardware',
    scalabilityMode: 'L1T1',
    latencyMode: 'realtime' as any
  }
}

function buildDefaultConfig(): VideoEncoderConfig {
  return buildConfigFromMessage({
    codec: 'avc1.640028', // H.264 High Profile Level 4.0
    width: 1280,
    height: 720,
    bitrate: 1_000_000,
    framerate: 30,
    hardwareAcceleration: 'prefer-hardware'
  })
}
