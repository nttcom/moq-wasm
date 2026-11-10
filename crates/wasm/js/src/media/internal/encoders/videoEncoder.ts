import { monotonicUnixMicros } from '../clock'
import { CaptureTimestamps } from './captureTimestamps'
import { fitEncodeSize } from './encodeSize'
import { videoEncoderConfig, type VideoEncoderSettings } from './videoEncoderConfig'
import { workerLog } from '../../../core/log'

const TARGET = 'moq-wasm::media::video_encoder'

/// The track a `videoTrack` message transferred; `stop` stops it.
let capturedTrack: MediaStreamTrack | undefined
// Default keeps periodic keyframes alive even before the first keyframeInterval message.
let keyframeInterval: number = 30
let forceNextKeyframe = false

/// One encoder of the stream's frames. Rendition 0 takes `encoderConfig`;
/// `renditionConfigs`, which must arrive before `videoStream`, adds the others.
/// All encode the same frames with the same keyframes, so the outputs of one
/// frame share a timestamp.
class Rendition {
  encoder: VideoEncoder | undefined
  // null makes the next frame reconfigure the encoder.
  configuredFrameSize: string | null = null
  private encodedAny = false
  readonly captureTimestamps = new CaptureTimestamps()

  constructor(
    readonly index: number,
    public config: VideoEncoderSettings
  ) {}

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
        self.postMessage({ type: 'configError', reason: 'unsupported', config, rendition: this.index })
        return false
      }
    } catch (e) {
      workerLog('error', TARGET, 'encoder config support check failed', { error: e, config, rendition: this.index })
      self.postMessage({ type: 'configError', reason: 'unsupported', config, rendition: this.index })
      return false
    }
    const encoder = new VideoEncoder(init)
    try {
      encoder.configure(config)
    } catch (e) {
      workerLog('error', TARGET, 'encoder configure failed', { error: e, config, rendition: this.index })
      self.postMessage({ type: 'configError', reason: 'unsupported', config, rendition: this.index })
      return false
    }
    this.encoder = encoder
    this.configuredFrameSize = frameSize(frame)
    this.encodedAny = false
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
      self.postMessage({ type: 'configError', reason: 'reconfigure_failed', config, rendition: this.index })
      return false
    }
    return true
  }

  private configFor(frame: VideoFrame): VideoEncoderConfig {
    return { ...this.config, ...fitEncodeSize(this.config, { width: frame.displayWidth, height: frame.displayHeight }) }
  }

  private send(chunk: EncodedVideoChunk, metadata: EncodedVideoChunkMetadata | undefined) {
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

let renditions: Rendition[] = [new Rendition(0, videoEncoderConfig())]

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
  let frameCounter = 0
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
      frameCounter = 0
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
    }

    // A frame any encoder cannot take is dropped for all, so the renditions keep the same frames.
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
    renditions[0].config = videoEncoderConfig(event.data.config)
    renditions[0].configuredFrameSize = null
  } else if (event.data.type === 'renditionConfigs') {
    const configs: Partial<VideoEncoderConfig>[] = event.data.configs
    renditions = [
      renditions[0],
      ...configs.map((config, index) => new Rendition(index + 1, videoEncoderConfig(config)))
    ]
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
