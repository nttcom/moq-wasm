import { monotonicUnixMicros } from '../clock'
import { avcCodecForSize } from './avcLevel'
import { CaptureTimestamps } from './captureTimestamps'
import { EncoderWarmUp } from './encoderWarmUp'
import { fitEncodeSize } from './encodeSize'
import { FrameRateLimit } from './frameRateLimit'
import { KeyframeSchedule } from './keyframeSchedule'
import { DEFAULT_KEYFRAME_INTERVAL_SECONDS, videoEncoderConfig, type VideoEncoderSettings } from './videoEncoderConfig'
import { workerLog } from '../../../core/log'

const TARGET = 'moq-wasm::media::video_encoder'

/// The track a `videoTrack` message transferred; `stop` stops it.
let capturedTrack: MediaStreamTrack | undefined
const keyframeSchedule = new KeyframeSchedule(DEFAULT_KEYFRAME_INTERVAL_SECONDS * 1_000_000)
let forceNextKeyframe = false

/// One encoder of the stream's frames. Rendition 0 takes `encoderConfig`;
/// `renditionConfigs`, which must arrive before `videoStream`, adds the others.
/// All encode the same keyframes, so the outputs of one frame share a
/// timestamp; a rendition with a frame rate set skips the frames above it.
class Rendition {
  encoder: VideoEncoder | undefined
  // null makes the next frame reconfigure the encoder.
  configuredFrameSize: string | null = null
  private encodedAny = false
  // The encoder whose outputs are dropped, until its warm-up frame is flushed.
  private warmingUp: VideoEncoder | undefined
  // Chrome gives the decoder config only with the first output after a
  // configure, which is a warm-up output.
  private warmUpDecoderConfig: VideoDecoderConfig | undefined
  readonly captureTimestamps = new CaptureTimestamps()
  config = videoEncoderConfig()
  frameRateLimit: FrameRateLimit | undefined

  constructor(
    readonly index: number,
    config: Partial<VideoEncoderConfig>
  ) {
    this.setConfig(config)
  }

  setConfig(config: Partial<VideoEncoderConfig>): void {
    this.config = videoEncoderConfig(config)
    this.frameRateLimit = frameRateLimit(this.config, config)
    this.configuredFrameSize = null
  }

  async initialize(frame: VideoFrame): Promise<boolean> {
    const init: VideoEncoderInit = {
      output: (chunk, metadata) => this.send(chunk, metadata),
      error: (e: DOMException) => {
        workerLog('error', TARGET, 'encoder error', { error: e, rendition: this.index })
        self.postMessage({ type: 'encoderError', message: e.message, rendition: this.index })
      }
    }
    const config = this.configFor(frame)
    try {
      const supported = await VideoEncoder.isConfigSupported(config)
      if (!supported.supported) {
        workerLog('error', TARGET, 'encoder config unsupported', { config, rendition: this.index })
        this.postConfigError('unsupported', config)
        return false
      }
    } catch (e) {
      workerLog('error', TARGET, 'encoder config support check failed', { error: e, config, rendition: this.index })
      this.postConfigError('unsupported', config, e)
      return false
    }
    const encoder = new VideoEncoder(init)
    try {
      encoder.configure(config)
    } catch (e) {
      workerLog('error', TARGET, 'encoder configure failed', { error: e, config, rendition: this.index })
      this.postConfigError('unsupported', config, e)
      return false
    }
    this.encoder = encoder
    this.configuredFrameSize = frameSize(frame)
    this.encodedAny = false
    this.warmingUp = undefined
    this.warmUpDecoderConfig = undefined
    workerLog('info', TARGET, 'encoder initialized', { config, rendition: this.index })
    return true
  }

  /// Whether the encoder was reconfigured for a new frame size, which needs a keyframe.
  configureForFrame(frame: VideoFrame): boolean {
    const size = frameSize(frame)
    if (!this.encoder || size === this.configuredFrameSize) {
      return false
    }
    this.configuredFrameSize = size
    const config = this.configFor(frame)
    try {
      this.encoder.configure(config)
    } catch (e) {
      workerLog('error', TARGET, 'encoder reconfigure failed', { error: e, config, rendition: this.index })
      this.postConfigError('reconfigure_failed', config, e)
      return false
    }
    return true
  }

  private postConfigError(reason: 'unsupported' | 'reconfigure_failed', config: VideoEncoderConfig, error?: unknown) {
    const cause = error === undefined ? '' : `: ${error instanceof Error ? error.message : String(error)}`
    const message = `${reason} ${config.codec} ${config.width}x${config.height} at ${config.framerate} fps${cause}`
    self.postMessage({ type: 'configError', reason, message, config, rendition: this.index })
  }

  private configFor(frame: VideoFrame): VideoEncoderConfig {
    const size = fitEncodeSize(this.config, { width: frame.displayWidth, height: frame.displayHeight })
    const codec = avcCodecForSize(this.config.codec, size, this.config.framerate ?? 30)
    return { ...this.config, ...size, codec }
  }

  /// Encodes `frame` as a keyframe and drops its output, sized like the live frames.
  async warmUp(frame: VideoFrame): Promise<void> {
    const encoder = this.encoder
    if (!encoder) {
      return
    }
    this.configureForFrame(frame)
    this.warmingUp = encoder
    try {
      encoder.encode(frame, { keyFrame: true })
      await encoder.flush()
    } finally {
      if (this.warmingUp === encoder) {
        this.warmingUp = undefined
      }
    }
  }

  private send(chunk: EncodedVideoChunk, metadata: EncodedVideoChunkMetadata | undefined) {
    if (this.warmingUp) {
      this.warmUpDecoderConfig = metadata?.decoderConfig ?? this.warmUpDecoderConfig
      return
    }
    if (this.warmUpDecoderConfig) {
      metadata = { ...metadata, decoderConfig: metadata?.decoderConfig ?? this.warmUpDecoderConfig }
      this.warmUpDecoderConfig = undefined
    }
    if (!this.encodedAny) {
      this.encodedAny = true
      workerLog('info', TARGET, 'first chunk encoded', {
        rendition: this.index,
        codec: metadata?.decoderConfig?.codec ?? 'none'
      })
    }
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

let renditions: Rendition[] = [new Rendition(0, {})]

/// A limit only for a frame rate the page set: the default one is the encoder's
/// estimate, and a camera faster than it keeps its frames.
function frameRateLimit(
  settings: VideoEncoderSettings,
  config: Partial<VideoEncoderConfig>
): FrameRateLimit | undefined {
  return config.framerate === undefined || settings.framerate === undefined
    ? undefined
    : new FrameRateLimit(settings.framerate)
}

function frameSize(frame: VideoFrame): string {
  return `${frame.displayWidth}x${frame.displayHeight}`
}

function needsEncoder(rendition: Rendition): boolean {
  return !rendition.encoder || rendition.encoder.state === 'closed'
}

/// Whether rendition 0 has an encoder. A rendition whose encoder fails is
/// dropped, after its `configError`, so the video goes on without it.
async function initializeEncoders(frame: VideoFrame): Promise<boolean> {
  const pending = renditions.filter(needsEncoder)
  const initialized = await Promise.all(pending.map((rendition) => rendition.initialize(frame)))
  const failed = new Set(pending.filter((_, index) => !initialized[index]))
  renditions = renditions.filter((rendition) => rendition.index === 0 || !failed.has(rendition))
  return !needsEncoder(renditions[0])
}

async function startVideoEncode(videoReadableStream: ReadableStream<VideoFrame>) {
  keyframeSchedule.reset()
  const warmUp = new EncoderWarmUp((error) => workerLog('warn', TARGET, 'encoder warm-up failed', { error }))
  for (const rendition of renditions) {
    if (rendition.encoder && rendition.encoder.state !== 'closed') {
      rendition.encoder.close()
    }
    rendition.encoder = undefined
    rendition.captureTimestamps.reset()
  }
  let started = false
  const videoReader = videoReadableStream.getReader()
  while (true) {
    let videoResult: ReadableStreamReadResult<VideoFrame>
    try {
      videoResult = await videoReader.read()
    } catch (e) {
      workerLog('warn', TARGET, 'video frame read failed', { error: e })
      return
    }
    if (videoResult.done) {
      workerLog('info', TARGET, 'video stream ended')
      return
    }
    const videoFrame = videoResult.value

    if (renditions.some(needsEncoder)) {
      if (started) {
        workerLog('info', TARGET, 'encoders re-initializing')
      }
      keyframeSchedule.reset()
      const initialized = await initializeEncoders(videoFrame)
      if (!initialized && !started) {
        videoFrame.close()
        return
      }
      started = true
      if (!initialized) {
        workerLog('error', TARGET, 'encoders failed to initialize, frame dropped')
        self.postMessage({ type: 'frameDropped' })
        videoFrame.close()
        continue
      }
      warmUp.reset()
    }

    if (!warmUp.admit(videoFrame, renditions)) {
      self.postMessage({ type: 'frameDropped' })
      videoFrame.close()
      continue
    }

    // A frame any encoder cannot take is dropped for all, so the renditions keep the same keyframes.
    const busiest = Math.max(...renditions.map((rendition) => rendition.encoder?.encodeQueueSize ?? 0))
    if (busiest > 5) {
      workerLog('debug', TARGET, 'encode queue full, frame dropped', { encode_queue_size: busiest })
      self.postMessage({ type: 'frameDropped' })
      videoFrame.close()
      continue
    }

    let reconfigured = false
    for (const rendition of renditions) {
      reconfigured = rendition.configureForFrame(videoFrame) || reconfigured
    }
    const keyFrame = keyframeSchedule.isKeyframe(videoFrame.timestamp, forceNextKeyframe || reconfigured)
    forceNextKeyframe = false
    for (const rendition of renditions) {
      if (rendition.frameRateLimit?.admit(videoFrame.timestamp, keyFrame) === false) {
        continue
      }
      rendition.captureTimestamps.recordCapture(videoFrame.timestamp)
      rendition.encoder?.encode(videoFrame, { keyFrame })
    }
    videoFrame.close()
  }
}

self.onmessage = async (event) => {
  if (event.data.type === 'keyframeInterval') {
    keyframeSchedule.intervalMicros = event.data.keyframeIntervalMicros
  } else if (event.data.type === 'forceKeyframe') {
    forceNextKeyframe = true
  } else if (event.data.type === 'encoderConfig') {
    renditions[0].setConfig(event.data.config)
  } else if (event.data.type === 'renditionConfigs') {
    const configs: Partial<VideoEncoderConfig>[] = event.data.configs
    renditions = [renditions[0], ...configs.map((config, index) => new Rendition(index + 1, config))]
  } else if (event.data.type === 'videoStream') {
    const videoReadableStream: ReadableStream<VideoFrame> = event.data.videoStream
    if (!videoReadableStream) {
      workerLog('error', TARGET, 'no video stream given')
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
