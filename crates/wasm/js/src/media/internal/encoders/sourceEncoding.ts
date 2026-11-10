import type { MediaSource } from '../../../../pkg/moqt'
import type { AudioLevel } from '../loc'
import { capturePcm } from './pcmCapture'
import { EncoderTimeline } from './encoderTimeline'
import { defaultKeyframeIntervalFrames, videoEncoderConfig } from './videoEncoderConfig'
import { AudioEncodeStats, type SourcePublicationStats, VideoEncodeStats } from './publishStats'
import { receiveWorkerLog } from '../../../core/log'

export type SourceOptions = {
  /// The video's label in the catalog.
  videoLabel?: string
  /// Frames between keyframes; each keyframe starts a group. By default, the
  /// frames of 30 seconds at the encoder's frame rate: a viewer that joins or
  /// switches rendition asks for a keyframe instead of waiting for the next.
  keyframeIntervalFrames?: number
  /// Fields set here replace the encoder's defaults (H.264, 1 Mbps, 30 fps,
  /// `latencyMode: 'realtime'`) for the video and its renditions. The
  /// broadcast takes H.264, VP8, VP9 and AV1. `width` and `height` bound the
  /// encoded size: a frame keeps its aspect ratio and is never scaled up.
  /// Without them a frame is encoded at its own size.
  videoEncoderConfig?: Partial<VideoEncoderConfig>
  /// Renditions of the video, each in the track `<video track>_<name>` and
  /// encoded from the video's frames.
  renditions?: Rendition[]
}

export type Rendition = {
  name: string
  label: string
  width: number
  height: number
  /// Bits per second.
  bitrate: number
}

export type MediaKind = 'video' | 'audio'

type ChunkMessage<Chunk, Metadata> = {
  type: 'chunk'
  chunk: Chunk
  metadata: Metadata | undefined
  captureTimestampMicros: number | undefined
}

/// Stops one kind's encoder worker and what feeds it. `forceKeyframe` makes
/// the next video frame a keyframe of the video and of every rendition.
type Encoder = { stop: () => void; forceKeyframe?: () => void }

type Encoding = { track?: MediaStreamTrack; muted: boolean; encoder?: Encoder }

export type SourceEncoding = {
  /// A muted kind is not encoded and its track leaves the catalog with
  /// PUBLISH_DONE; unmuting publishes it again from a new group.
  setMuted(kind: MediaKind, muted: boolean): Promise<void>
  /// A muted kind stays muted. Does nothing once the source is closed.
  replaceTrack(kind: MediaKind, track: MediaStreamTrack): void
  /// Restarts the video encoder with `config` and the renditions' encoders
  /// with its codec. A muted video takes it when unmuted. The catalog keeps the
  /// bitrate the source was published with. Does nothing once the source
  /// is closed.
  setVideoEncoderConfig(config: Partial<VideoEncoderConfig>): void
  /// What the source's encoders output over the last second, and the frames
  /// they dropped and their failures since it was published.
  stats(): SourcePublicationStats
  /// Ends the tracks with PUBLISH_DONE. The session stays open.
  close(): Promise<void>
  /// Stops encoding `kind` and ends its track with PUBLISH_DONE, keeping
  /// whether it is muted; `replaceTrack` publishes it again. With `track`,
  /// does nothing once `kind` encodes another track.
  removeTrack(kind: MediaKind, track?: MediaStreamTrack): Promise<void>
  isMuted(kind: MediaKind): boolean
}

export function videoBitrate(options: SourceOptions): number | undefined {
  return videoEncoderConfig(options.videoEncoderConfig).bitrate
}

/// `addSource` adds the source to the broadcast with the callback it is given,
/// which the broadcast calls when a subscriber asks for a new group.
export function publishSource(
  addSource: (onKeyframeRequest: () => void) => MediaSource,
  stream: MediaStream,
  initialOptions: SourceOptions,
  startedAtMs: number,
  report: (error: unknown) => void
): SourceEncoding {
  let options = initialOptions
  const kinds = new Map<MediaKind, Encoding>()
  const source = addSource(() => kinds.get('video')?.encoder?.forceKeyframe?.())
  const renditionIndices = (options.renditions ?? []).map(({ name, label, bitrate }) =>
    source.addRendition(name, label, bitrate)
  )
  const videoStats = new VideoEncodeStats((options.renditions ?? []).map(({ name }) => name))
  const audioStats = new AudioEncodeStats()
  const encode = (kind: MediaKind, track: MediaStreamTrack) => {
    const timeline = new EncoderTimeline(
      Math.round((performance.timeOrigin + startedAtMs) * 1000),
      Math.round((performance.now() - startedAtMs) * 1000)
    )
    return kind === 'video'
      ? encodeVideo(source, track, options, renditionIndices, timeline, videoStats, report)
      : encodeAudio(source, track, timeline, audioStats, report)
  }
  for (const [kind, track] of [
    ['video', stream.getVideoTracks()[0]],
    ['audio', stream.getAudioTracks()[0]]
  ] as const) {
    if (track) {
      kinds.set(kind, { track, muted: false, encoder: encode(kind, track) })
    }
  }
  // An unmute that publishes the track before the mute's removal finishes
  // would leave it removed, so mutes, unmutes, removals and the close run one
  // at a time.
  let closed = false

  // A source without audio removes the `audio` placeholder the catalog lists
  // with the default `video`, queued before its first configuration.
  let queue: Promise<void> = kinds.has('audio') ? Promise.resolve() : source.removeTrack('audio').catch(report)
  const serialized = (operation: () => Promise<void>) => (queue = queue.then(operation, operation))
  const stop = async (kind: MediaKind) => {
    const encoding = kinds.get(kind)
    if (!encoding?.encoder) {
      return
    }
    encoding.encoder.stop()
    encoding.encoder = undefined
    await source.removeTrack(kind)
  }
  return {
    setMuted: (kind, muted) =>
      serialized(async () => {
        if (closed) {
          return
        }
        const encoding = kinds.get(kind) ?? { muted: false }
        kinds.set(kind, encoding)
        if (encoding.muted === muted) {
          return
        }
        encoding.muted = muted
        if (muted) {
          await stop(kind)
        } else if (encoding.track) {
          encoding.encoder = encode(kind, encoding.track)
        }
      }),
    replaceTrack: (kind, track) => {
      if (closed) {
        return
      }
      const encoding = kinds.get(kind) ?? { muted: false }
      encoding.track = track
      kinds.set(kind, encoding)
      if (!encoding.muted) {
        encoding.encoder?.stop()
        encoding.encoder = encode(kind, track)
      }
    },
    setVideoEncoderConfig: (config) => {
      if (closed) {
        return
      }
      options = { ...options, videoEncoderConfig: config }
      const encoding = kinds.get('video')
      if (encoding?.track && !encoding.muted) {
        encoding.encoder?.stop()
        encoding.encoder = encode('video', encoding.track)
      }
    },
    removeTrack: (kind, track) =>
      serialized(async () => {
        const encoding = kinds.get(kind)
        if (closed || !encoding || (track && encoding.track !== track)) {
          return
        }
        await stop(kind)
        encoding.track = undefined
      }),
    isMuted: (kind) => kinds.get(kind)?.muted ?? false,
    stats: () => ({
      video: kinds.get('video')?.track ? videoStats.stats() : undefined,
      audio: kinds.get('audio')?.track ? audioStats.stats() : undefined
    }),
    close: () => {
      closed = true
      return serialized(async () => {
        await Promise.all([...kinds.keys()].map(stop))
        kinds.clear()
      })
    }
  }
}

function encodeVideo(
  source: MediaSource,
  track: MediaStreamTrack,
  options: SourceOptions,
  renditionIndices: number[],
  timeline: EncoderTimeline,
  stats: VideoEncodeStats,
  report: (error: unknown) => void
): Encoder {
  const worker = new Worker(new URL('./videoEncoder.ts', import.meta.url), {
    type: 'module'
  })
  worker.onerror = (event) => {
    stats.encoderErrors++
    report(workerError('video', event))
  }
  const configuredCodecs: (string | undefined)[] = []
  worker.onmessage = (
    event: MessageEvent<
      (
        | ChunkMessage<EncodedVideoChunk, EncodedVideoChunkMetadata>
        | { type: 'configError' | 'encoderError'; message: string }
        | { type: 'frameDropped' }
      ) & {
        rendition: number
      }
    >
  ) => {
    if (receiveWorkerLog(event.data)) {
      return
    }
    if (event.data.type === 'frameDropped') {
      stats.droppedFrames++
      return
    }
    if (event.data.type === 'configError' || event.data.type === 'encoderError') {
      stats.encoderErrors++
      report(new Error(`video encoder ${event.data.rendition}: ${event.data.type}: ${event.data.message}`))
      return
    }
    if (event.data.type !== 'chunk') {
      return
    }
    const { chunk, metadata, rendition, captureTimestampMicros } = event.data
    stats.add(rendition, chunk, metadata)
    const timestampMicros = timeline.place(chunk.timestamp, captureTimestampMicros)
    const renditionIndex = rendition === 0 ? undefined : renditionIndices[rendition - 1]
    try {
      const codec = metadata?.decoderConfig?.codec
      if (codec && codec !== configuredCodecs[rendition]) {
        if (renditionIndex === undefined) {
          source.configureVideo(codec)
        } else {
          source.configureRendition(renditionIndex, codec)
        }
        configuredCodecs[rendition] = codec
      }
      if (renditionIndex === undefined) {
        source.pushVideo(copied(chunk), timestampMicros)
      } else {
        source.pushRenditionVideo(renditionIndex, copied(chunk), timestampMicros)
      }
    } catch (error) {
      report(error)
    }
  }
  worker.postMessage({
    type: 'keyframeInterval',
    keyframeInterval: options.keyframeIntervalFrames ?? defaultKeyframeIntervalFrames(options.videoEncoderConfig)
  })
  const forceKeyframe = () => worker.postMessage({ type: 'forceKeyframe' })
  if (options.videoEncoderConfig) {
    worker.postMessage({ type: 'encoderConfig', config: options.videoEncoderConfig })
  }
  if (options.renditions?.length) {
    worker.postMessage({
      type: 'renditionConfigs',
      configs: options.renditions.map(({ width, height, bitrate }) => ({
        ...options.videoEncoderConfig,
        width,
        height,
        bitrate
      }))
    })
  }
  if (typeof MediaStreamTrackProcessor === 'function') {
    const videoStream = new MediaStreamTrackProcessor({ track }).readable
    worker.postMessage({ type: 'videoStream', videoStream }, [videoStream])
    return { stop: () => worker.terminate(), forceKeyframe }
  }
  // Safari reads a track only in a worker. The worker stops the clone it was
  // given, which `terminate` would leave running.
  const clone = track.clone()
  worker.postMessage({ type: 'videoTrack', track: clone }, [clone as unknown as Transferable])
  return {
    stop: () => {
      worker.onmessage = null
      worker.postMessage({ type: 'stop' })
    },
    forceKeyframe
  }
}

function encodeAudio(
  source: MediaSource,
  track: MediaStreamTrack,
  timeline: EncoderTimeline,
  stats: AudioEncodeStats,
  report: (error: unknown) => void
): Encoder {
  const worker = new Worker(new URL('./audioEncoder.ts', import.meta.url), {
    type: 'module'
  })
  worker.onerror = (event) => {
    stats.encoderErrors++
    report(workerError('audio', event))
  }
  let configured = false
  worker.onmessage = (
    event: MessageEvent<
      | (ChunkMessage<EncodedAudioChunk, EncodedAudioChunkMetadata> & { audioLevel?: AudioLevel })
      | { type: 'configError' | 'encoderError' }
    >
  ) => {
    if (receiveWorkerLog(event.data)) {
      return
    }
    if (event.data.type === 'configError' || event.data.type === 'encoderError') {
      stats.encoderErrors++
      report(new Error(`audio encoder: ${event.data.type}`))
      return
    }
    if (event.data.type !== 'chunk') {
      return
    }
    const { chunk, metadata, audioLevel, captureTimestampMicros } = event.data
    stats.add(chunk)
    const timestampMicros = timeline.place(chunk.timestamp, captureTimestampMicros)
    try {
      const config = metadata?.decoderConfig
      if (config) {
        source.configureAudio(
          config.codec,
          config.sampleRate,
          config.numberOfChannels,
          config.description && bytes(config.description)
        )
        configured = true
      }
      if (configured) {
        source.pushAudio(copied(chunk), timestampMicros, audioLevel?.level, audioLevel?.voiceActivity)
      }
    } catch (error) {
      report(error)
    }
  }
  if (typeof MediaStreamTrackProcessor === 'function') {
    const audioStream = new MediaStreamTrackProcessor({ track }).readable
    worker.postMessage({ audioStream }, [audioStream])
    return { stop: () => worker.terminate() }
  }
  const capture = capturePcm(track)
  capture.then(({ port }) => worker.postMessage({ audioPort: port }, [port]), report)
  return {
    stop: () => {
      worker.terminate()
      void capture.then(
        ({ close }) => close(),
        () => {}
      )
    }
  }
}

/// `Worker.onerror` is the only signal of a worker script that failed to load or threw an
/// uncaught exception; a load failure has no message.
function workerError(kind: MediaKind, event: ErrorEvent): Error {
  return new Error(`${kind} encoder worker failed: ${event.message || 'the script did not load'}`)
}

function bytes(source: AllowSharedBufferSource): Uint8Array {
  return ArrayBuffer.isView(source)
    ? new Uint8Array(source.buffer, source.byteOffset, source.byteLength)
    : new Uint8Array(source)
}

function copied(chunk: EncodedVideoChunk | EncodedAudioChunk): Uint8Array {
  const data = new Uint8Array(chunk.byteLength)
  chunk.copyTo(data)
  return data
}
