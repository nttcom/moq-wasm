import type { MoqSession } from '@moqt/session'
import type { MediaBroadcast } from '../../pkg/moqt'

export type PublishOptions = {
  /// The video's label in the catalog.
  videoLabel?: string
  /// Frames between keyframes; each keyframe starts a group.
  keyframeIntervalFrames?: number
  /// Replaces the encoder's H.264 default; the broadcast takes H.264 only.
  videoEncoderConfig?: VideoEncoderConfig
  onError?: (error: unknown) => void
}

export type Publication = {
  /// Stops encoding. The session stays open, and the relay keeps the
  /// broadcast's tracks until the session ends.
  stop(): void
}

const DEFAULT_KEYFRAME_INTERVAL_FRAMES = 60

type ChunkMessage<Chunk, Metadata> = { type: 'chunk'; chunk: Chunk; metadata: Metadata | undefined }

/// Publishes `stream` as an MSF broadcast in `namespace`, encoding its first
/// video track as H.264 and its first audio track as Opus.
export async function publish(
  session: MoqSession,
  namespace: string[],
  stream: MediaStream,
  options: PublishOptions = {}
): Promise<Publication> {
  const broadcast = session.broadcast(namespace, options.videoLabel)
  await broadcast.announce()
  const report = options.onError ?? ((error: unknown) => console.error('[publish]', error))
  const workers: Worker[] = []
  const [video] = stream.getVideoTracks()
  if (video) {
    workers.push(encodeVideo(broadcast, video, options, report))
  }
  const [audio] = stream.getAudioTracks()
  if (audio) {
    workers.push(encodeAudio(broadcast, audio, report))
  }
  return {
    stop: () => {
      for (const worker of workers) {
        worker.terminate()
      }
    }
  }
}

function encodeVideo(
  broadcast: MediaBroadcast,
  track: MediaStreamTrack,
  options: PublishOptions,
  report: (error: unknown) => void
): Worker {
  const worker = new Worker(new URL('../../utils/media/encoders/videoEncoder.ts', import.meta.url), {
    type: 'module'
  })
  worker.onmessage = (event: MessageEvent<ChunkMessage<EncodedVideoChunk, EncodedVideoChunkMetadata>>) => {
    if (event.data.type !== 'chunk') {
      return
    }
    const { chunk } = event.data
    try {
      broadcast.pushVideo(copied(chunk), chunk.timestamp)
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
  const videoStream = new MediaStreamTrackProcessor({ track }).readable
  worker.postMessage({ type: 'videoStream', videoStream }, [videoStream])
  return worker
}

function encodeAudio(broadcast: MediaBroadcast, track: MediaStreamTrack, report: (error: unknown) => void): Worker {
  const worker = new Worker(new URL('../../utils/media/encoders/audioEncoder.ts', import.meta.url), {
    type: 'module'
  })
  let configured = false
  worker.onmessage = (event: MessageEvent<ChunkMessage<EncodedAudioChunk, EncodedAudioChunkMetadata>>) => {
    if (event.data.type !== 'chunk') {
      return
    }
    const { chunk, metadata } = event.data
    try {
      const config = metadata?.decoderConfig
      if (config) {
        broadcast.configureAudio(
          config.codec,
          config.sampleRate,
          config.numberOfChannels,
          config.description && bytes(config.description)
        )
        configured = true
      }
      if (configured) {
        broadcast.pushAudio(copied(chunk), chunk.timestamp)
      }
    } catch (error) {
      report(error)
    }
  }
  const audioStream = new MediaStreamTrackProcessor({ track }).readable
  worker.postMessage({ audioStream }, [audioStream])
  return worker
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
