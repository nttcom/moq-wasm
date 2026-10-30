import type { MoqSession } from '@moqt/session'
import type { MediaSource } from '../../pkg/moqt'
import type { AudioLevel } from '../../utils/media/loc'
import { capturePcm } from '../../utils/media/encoders/pcmCapture'

export type SourceOptions = {
  /// The video's label in the catalog.
  videoLabel?: string
  /// Frames between keyframes; each keyframe starts a group.
  keyframeIntervalFrames?: number
  /// Replaces the encoder's H.264 default; the broadcast takes H.264, VP8, VP9
  /// and AV1.
  videoEncoderConfig?: VideoEncoderConfig
  /// Renditions of the video, each in the track `<video track>_<name>` and
  /// encoded from the video's frames.
  renditions?: Rendition[]
}

export type PublishOptions = SourceOptions & {
  /// Lists the `chat` track in the catalog even while nothing is published on
  /// `video` or `audio`.
  offerChat?: boolean
  onError?: (error: unknown) => void
}

export type Rendition = {
  name: string
  label: string
  width: number
  height: number
  /// Bits per second.
  bitrate: number
}

/// The codec and bitrate of the video encoder worker's `buildDefaultConfig`.
const DEFAULT_VIDEO_CODEC = 'avc1.640028'
const DEFAULT_VIDEO_BITRATE = 1_000_000

export type MediaKind = 'video' | 'audio'

export type SourcePublication = {
  /// A muted kind is not encoded and its track leaves the catalog with
  /// PUBLISH_DONE; unmuting publishes it again from a new group.
  setMuted(kind: MediaKind, muted: boolean): Promise<void>
  /// A muted kind stays muted. Does nothing once the publication is closed.
  replaceTrack(kind: MediaKind, track: MediaStreamTrack): void
  /// Restarts the video encoder with `config` and the renditions' encoders
  /// with its codec. A muted video takes it when unmuted. The catalog keeps the
  /// bitrate the source was published with. Does nothing once the publication
  /// is closed.
  setVideoEncoderConfig(config: VideoEncoderConfig): void
  /// Ends the tracks with PUBLISH_DONE. The session stays open.
  close(): Promise<void>
}

export type Publication = SourcePublication & {
  /// Publishes `stream` in the tracks `name` and `<name>_audio`. `close` on the
  /// publication closes it too. Fails once the publication is closed, and for a
  /// name already published.
  addSource(name: string, stream: MediaStream, options?: SourceOptions): SourcePublication
}

const DEFAULT_KEYFRAME_INTERVAL_FRAMES = 60

type ChunkMessage<Chunk, Metadata> = { type: 'chunk'; chunk: Chunk; metadata: Metadata | undefined }

/// Stops one kind's encoder worker and what feeds it.
type Encoder = { stop: () => void }

type Encoding = { track: MediaStreamTrack; muted: boolean; encoder?: Encoder }

/// Publishes `stream` as an MSF broadcast in `namespace`, encoding its first
/// video track as H.264 by default and its first audio track as Opus.
export async function publish(
  session: MoqSession,
  namespace: string[],
  stream: MediaStream,
  options: PublishOptions = {}
): Promise<Publication> {
  const broadcast = session.broadcast(namespace)
  await broadcast.announce()
  if (options.offerChat) {
    await broadcast.offerChat()
  }
  const report = options.onError ?? ((error: unknown) => console.error('[publish]', error))
  // A new encoder worker stamps its first chunk 0, so the chunks of a worker
  // started after an unmute or a replaced track are shifted by the time since
  // the publication began: a source's timestamps never run backwards, which
  // would reuse its group ids and capture times.
  const startedAtMs = performance.now()
  const camera = publishSource(
    broadcast.addSource('video', options.videoLabel ?? 'Video', videoBitrate(options), 'audio', 'Audio'),
    stream,
    options,
    startedAtMs,
    report
  )
  const added = new Map<string, SourcePublication>()
  let closed = false
  return {
    ...camera,
    addSource: (name, sourceStream, sourceOptions = {}) => {
      if (closed) {
        throw new Error('the publication is closed')
      }
      if (name === 'video' || added.has(name)) {
        throw new Error(`a source named ${name} is already published`)
      }
      const source = publishSource(
        broadcast.addSource(
          name,
          sourceOptions.videoLabel ?? name,
          videoBitrate(sourceOptions),
          `${name}_audio`,
          `${name} audio`
        ),
        sourceStream,
        sourceOptions,
        startedAtMs,
        report
      )
      added.set(name, source)
      return source
    },
    close: async () => {
      closed = true
      await Promise.all([camera, ...added.values()].map((source) => source.close()))
    }
  }
}

function videoBitrate(options: SourceOptions): number | undefined {
  return options.videoEncoderConfig ? options.videoEncoderConfig.bitrate : DEFAULT_VIDEO_BITRATE
}

function publishSource(
  source: MediaSource,
  stream: MediaStream,
  initialOptions: SourceOptions,
  startedAtMs: number,
  report: (error: unknown) => void
): SourcePublication {
  let options = initialOptions
  const renditionIndices = (options.renditions ?? []).map(({ name, label, bitrate }) =>
    source.addRendition(name, label, bitrate)
  )
  const encode = (kind: MediaKind, track: MediaStreamTrack) => {
    const offsetMicros = Math.round((performance.now() - startedAtMs) * 1000)
    return kind === 'video'
      ? encodeVideo(source, track, options, renditionIndices, offsetMicros, report)
      : encodeAudio(source, track, offsetMicros, report)
  }
  const kinds = new Map<MediaKind, Encoding>()
  for (const [kind, track] of [
    ['video', stream.getVideoTracks()[0]],
    ['audio', stream.getAudioTracks()[0]]
  ] as const) {
    if (track) {
      kinds.set(kind, { track, muted: false, encoder: encode(kind, track) })
    }
  }
  // An unmute that publishes the track before the mute's removal finishes
  // would leave it removed, so mutes, unmutes and the close run one at a time.
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
        const encoding = kinds.get(kind)
        if (closed || !encoding || encoding.muted === muted) {
          return
        }
        encoding.muted = muted
        if (muted) {
          await stop(kind)
        } else {
          encoding.encoder = encode(kind, encoding.track)
        }
      }),
    replaceTrack: (kind, track) => {
      if (closed) {
        return
      }
      const encoding = kinds.get(kind) ?? { track, muted: false }
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
      if (encoding && !encoding.muted) {
        encoding.encoder?.stop()
        encoding.encoder = encode('video', encoding.track)
      }
    },
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
  offsetMicros: number,
  report: (error: unknown) => void
): Encoder {
  const worker = new Worker(new URL('../../utils/media/encoders/videoEncoder.ts', import.meta.url), {
    type: 'module'
  })
  const configuredCodecs: (string | undefined)[] = []
  worker.onmessage = (
    event: MessageEvent<
      (ChunkMessage<EncodedVideoChunk, EncodedVideoChunkMetadata> | { type: 'configError' | 'encoderError' }) & {
        rendition: number
      }
    >
  ) => {
    if (event.data.type === 'configError' || event.data.type === 'encoderError') {
      report(new Error(`video encoder ${event.data.rendition}: ${event.data.type}`))
      return
    }
    if (event.data.type !== 'chunk') {
      return
    }
    const { chunk, metadata, rendition } = event.data
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
        source.pushVideo(copied(chunk), chunk.timestamp + offsetMicros)
      } else {
        source.pushRenditionVideo(renditionIndex, copied(chunk), chunk.timestamp + offsetMicros)
      }
    } catch (error) {
      report(error)
    }
  }
  worker.postMessage({
    type: 'keyframeInterval',
    keyframeInterval: options.keyframeIntervalFrames ?? DEFAULT_KEYFRAME_INTERVAL_FRAMES
  })
  if (options.videoEncoderConfig) {
    worker.postMessage({ type: 'encoderConfig', config: options.videoEncoderConfig })
  }
  if (options.renditions?.length) {
    const codec = options.videoEncoderConfig?.codec ?? DEFAULT_VIDEO_CODEC
    worker.postMessage({
      type: 'renditionConfigs',
      configs: options.renditions.map(({ width, height, bitrate }) => ({
        codec,
        width,
        height,
        bitrate,
        framerate: options.videoEncoderConfig?.framerate,
        hardwareAcceleration: options.videoEncoderConfig?.hardwareAcceleration
      }))
    })
  }
  if (typeof MediaStreamTrackProcessor === 'function') {
    const videoStream = new MediaStreamTrackProcessor({ track }).readable
    worker.postMessage({ type: 'videoStream', videoStream }, [videoStream])
    return { stop: () => worker.terminate() }
  }
  // Safari reads a track only in a worker. The worker stops the clone it was
  // given, which `terminate` would leave running.
  const clone = track.clone()
  worker.postMessage({ type: 'videoTrack', track: clone }, [clone as unknown as Transferable])
  return {
    stop: () => {
      worker.onmessage = null
      worker.postMessage({ type: 'stop' })
    }
  }
}

function encodeAudio(
  source: MediaSource,
  track: MediaStreamTrack,
  offsetMicros: number,
  report: (error: unknown) => void
): Encoder {
  const worker = new Worker(new URL('../../utils/media/encoders/audioEncoder.ts', import.meta.url), {
    type: 'module'
  })
  let configured = false
  worker.onmessage = (
    event: MessageEvent<ChunkMessage<EncodedAudioChunk, EncodedAudioChunkMetadata> & { audioLevel?: AudioLevel }>
  ) => {
    if (event.data.type !== 'chunk') {
      return
    }
    const { chunk, metadata, audioLevel } = event.data
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
        source.pushAudio(copied(chunk), chunk.timestamp + offsetMicros, audioLevel?.level, audioLevel?.voiceActivity)
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
