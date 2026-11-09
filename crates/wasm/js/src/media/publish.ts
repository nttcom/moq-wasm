import type { MoqSession } from '../core/session'
import { forwardSdkLog, sdkLog } from '../core/log'
import {
  type SourceOptions,
  type SourcePublication,
  publishSource,
  videoBitrate
} from './internal/encoders/sourceEncoding'

export type { MediaKind, Rendition, SourceOptions, SourcePublication } from './internal/encoders/sourceEncoding'

const TARGET = 'moq-wasm::media::publish'

export type PublishOptions = SourceOptions & {
  /// Lists the `chat` track in the catalog even while nothing is published on
  /// `video` or `audio`.
  offerChat?: boolean
  /// Skips a failure whose `String()` equals the previous one, such as the
  /// "media publishing stopped" every chunk fails with after publishing stops.
  onError?: (error: unknown) => void
}

export type Publication = SourcePublication & {
  /// Publishes `stream` in the tracks `name` and `<name>_audio`. `close` on the
  /// publication closes it too. Fails once the publication is closed, and for a
  /// name already published.
  addSource(name: string, stream: MediaStream, options?: SourceOptions): SourcePublication
}

/// Publishes `stream` as an MSF broadcast in `namespace`, encoding its first
/// video track as H.264 by default and its first audio track as Opus.
export async function publish(
  session: MoqSession,
  namespace: string[],
  stream: MediaStream,
  options: PublishOptions = {}
): Promise<Publication> {
  const broadcast = session.broadcast(namespace)
  // The catalog lists a track only at its first frame; a subscriber that knows
  // the track's name may subscribe before that.
  await broadcast.declareTracks([
    ...(stream.getVideoTracks().length > 0 ? ['video'] : []),
    ...(stream.getAudioTracks().length > 0 ? ['audio'] : [])
  ])
  if (options.offerChat) {
    await broadcast.offerChat()
  }
  let lastReported: string | undefined
  const report = (error: unknown) => {
    const message = String(error)
    if (message === lastReported) {
      return
    }
    lastReported = message
    if (options.onError) {
      forwardSdkLog('error', TARGET, 'publish failed', { error })
      options.onError(error)
    } else {
      sdkLog('error', TARGET, 'publish failed', { error })
    }
  }
  // A new encoder worker stamps its first chunk 0, so the chunks of a worker
  // started after an unmute or a replaced track are placed from the
  // publication's start (see `EncoderTimeline`): a source's timestamps never
  // run backwards, which would reuse its group ids and capture times.
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
