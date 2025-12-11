import { MoqtClientWrapper } from '@moqt/moqtClient'
import { SubgroupStreamObjectMessage } from '../../../../pkg/moqt_client_sample'
import type { VideoJitterBufferMode } from '../../../../utils/media/videoJitterBuffer'
import type { AudioJitterBufferMode } from '../../../../utils/media/audioJitterBuffer'
import {
  DEFAULT_VIDEO_JITTER_CONFIG,
  DEFAULT_AUDIO_JITTER_CONFIG,
  normalizeVideoJitterConfig,
  normalizeAudioJitterConfig,
  type VideoJitterConfig,
  type AudioJitterConfig
} from '../types/jitterBuffer'

interface MediaSubscriberHandlers {
  onRemoteVideoStream?: (userId: string, stream: MediaStream) => void
  onRemoteAudioStream?: (userId: string, stream: MediaStream) => void
  onRemoteVideoBitrate?: (userId: string, mbps: number) => void
  onRemoteAudioBitrate?: (userId: string, mbps: number) => void
  onRemoteVideoReceiveLatency?: (userId: string, ms: number) => void
  onRemoteVideoRenderingLatency?: (userId: string, ms: number) => void
  onRemoteAudioReceiveLatency?: (userId: string, ms: number) => void
  onRemoteAudioRenderingLatency?: (userId: string, ms: number) => void
  onRemoteVideoConfig?: (userId: string, config: { codec: string; width?: number; height?: number }) => void
}

interface VideoSubscriptionContext {
  userId: string
  worker: Worker
  writer: WritableStreamDefaultWriter<VideoFrame>
  stream: MediaStream
}

interface AudioSubscriptionContext {
  userId: string
  worker: Worker
  writer: WritableStreamDefaultWriter<AudioData>
  stream: MediaStream
}

export class MediaSubscriber {
  private handlers: MediaSubscriberHandlers = {}
  private readonly videoContexts = new Map<bigint, VideoSubscriptionContext>()
  private readonly audioContexts = new Map<bigint, AudioSubscriptionContext>()
  private readonly videoJitterConfigByUserId = new Map<string, VideoJitterConfig>()
  private readonly audioJitterConfigByUserId = new Map<string, AudioJitterConfig>()
  private readonly videoCodecByTrackAlias = new Map<bigint, string>()
  private readonly videoSizeByTrackAlias = new Map<bigint, { width: number; height: number }>()

  constructor(private readonly client: MoqtClientWrapper) {}

  setHandlers(handlers: MediaSubscriberHandlers): void {
    this.handlers = handlers
  }

  registerVideoTrack(userId: string, trackAlias: bigint): void {
    if (this.videoContexts.has(trackAlias)) {
      return
    }
    const worker = new Worker(new URL('../../../../utils/media/decoders/videoDecoder.ts', import.meta.url), {
      type: 'module'
    })
    const generator = new MediaStreamTrackGenerator({ kind: 'video' })
    const writer = generator.writable.getWriter()
    worker.onmessage = async (event: MessageEvent) => {
      const data = event.data as
        | { type: 'frame'; frame: VideoFrame; width?: number; height?: number }
        | { type: 'bitrate'; kbps: number }
        | { type: 'receiveLatency'; media: 'video'; ms: number }
        | { type: 'renderingLatency'; media: 'video'; ms: number }
        | { type: 'decoderConfig'; codec: string; width?: number; height?: number }
      if (data.type === 'bitrate') {
        this.handlers.onRemoteVideoBitrate?.(userId, data.kbps)
        return
      }
      if (data.type === 'receiveLatency') {
        this.handlers.onRemoteVideoReceiveLatency?.(userId, data.ms)
        return
      }
      if (data.type === 'renderingLatency') {
        this.handlers.onRemoteVideoRenderingLatency?.(userId, data.ms)
        return
      }
      if (data.type === 'decoderConfig') {
        this.videoCodecByTrackAlias.set(trackAlias, data.codec)
        const lastSize = this.videoSizeByTrackAlias.get(trackAlias)
        this.handlers.onRemoteVideoConfig?.(userId, {
          codec: data.codec,
          width: data.width ?? lastSize?.width,
          height: data.height ?? lastSize?.height
        })
        return
      }
      if (data.type === 'frame') {
        const lastCodec = this.videoCodecByTrackAlias.get(trackAlias)
        const lastSize = this.videoSizeByTrackAlias.get(trackAlias)
        if (
          (typeof data.width === 'number' && data.width !== lastSize?.width) ||
          (typeof data.height === 'number' && data.height !== lastSize?.height)
        ) {
          if (typeof data.width === 'number' && typeof data.height === 'number') {
            this.videoSizeByTrackAlias.set(trackAlias, { width: data.width, height: data.height })
            this.handlers.onRemoteVideoConfig?.(userId, {
              codec: lastCodec ?? 'unknown',
              width: data.width,
              height: data.height
            })
          }
        }
      }
      await writer.ready
      await writer.write(data.frame)
    }

    const stream = new MediaStream([generator])
    this.videoContexts.set(trackAlias, { userId, worker, writer, stream })
    this.handlers.onRemoteVideoStream?.(userId, stream)
    const config = this.videoJitterConfigByUserId.get(userId)
    if (config) {
      worker.postMessage({ type: 'config', config })
    }

    this.client.setOnSubgroupObjectHandler(trackAlias, (groupId, message) =>
      this.forwardToWorker(worker, trackAlias, groupId, message)
    )
  }

  registerAudioTrack(userId: string, trackAlias: bigint): void {
    if (this.audioContexts.has(trackAlias)) {
      return
    }
    const worker = new Worker(new URL('../../../../utils/media/decoders/audioDecoder.ts', import.meta.url), {
      type: 'module'
    })
    const generator = new MediaStreamTrackGenerator({ kind: 'audio' })
    const writer = generator.writable.getWriter()
    const context: AudioSubscriptionContext = {
      userId,
      worker,
      writer,
      stream: new MediaStream([generator])
    }
    worker.onmessage = async (event: MessageEvent) => {
      const data = event.data as
        | { type: 'audioData'; audioData: AudioData }
        | { type: 'bitrate'; kbps: number }
        | { type: 'receiveLatency'; media: 'audio'; ms: number }
        | { type: 'renderingLatency'; media: 'audio'; ms: number }
      if (data.type === 'bitrate') {
        this.handlers.onRemoteAudioBitrate?.(userId, data.kbps)
        return
      }
      if (data.type === 'receiveLatency') {
        this.handlers.onRemoteAudioReceiveLatency?.(userId, data.ms)
        return
      }
      if (data.type === 'renderingLatency') {
        this.handlers.onRemoteAudioRenderingLatency?.(userId, data.ms)
        return
      }
      const audioData = data.audioData
      await writer.ready
      await writer.write(audioData)
    }

    this.audioContexts.set(trackAlias, context)
    this.handlers.onRemoteAudioStream?.(userId, context.stream)
    const config = this.audioJitterConfigByUserId.get(userId)
    if (config) {
      worker.postMessage({ type: 'config', config })
    }

    this.client.setOnSubgroupObjectHandler(trackAlias, (groupId, message) =>
      this.forwardToWorker(worker, trackAlias, groupId, message)
    )
  }

  private forwardToWorker(worker: Worker, trackAlias: bigint, groupId: bigint, message: SubgroupStreamObjectMessage) {
    if (message.objectStatus === 3) {
      console.debug(`[MediaSubscriber] Received EndOfGroup trackAlias=${trackAlias} groupId=${groupId}`)
    }
    const payload = new Uint8Array(message.objectPayload)
    const payloadLength = message.objectPayloadLength
    worker.postMessage(
      {
        groupId,
        subgroupStreamObject: {
          objectId: message.objectId,
          objectPayloadLength: payloadLength,
          objectPayload: payload,
          objectStatus: message.objectStatus
        }
      },
      [payload.buffer]
    )
  }

  setVideoJitterBufferConfig(userId: string, config: VideoJitterConfig): void {
    const sanitized = this.sanitizeConfig(config)
    this.videoJitterConfigByUserId.set(userId, sanitized)
    for (const context of this.videoContexts.values()) {
      if (context.userId === userId) {
        context.worker.postMessage({ type: 'config', config: sanitized })
      }
    }
  }

  setAudioJitterBufferConfig(userId: string, config: AudioJitterConfig): void {
    const sanitized = this.sanitizeAudioConfig(config)
    this.audioJitterConfigByUserId.set(userId, sanitized)
    for (const context of this.audioContexts.values()) {
      if (context.userId === userId) {
        context.worker.postMessage({ type: 'config', config: sanitized })
      }
    }
  }

  private sanitizeConfig(config: VideoJitterConfig): VideoJitterConfig {
    const normalized = normalizeVideoJitterConfig(config)
    const mode: VideoJitterBufferMode = normalized.mode ?? DEFAULT_VIDEO_JITTER_CONFIG.mode
    return { ...normalized, mode }
  }

  private sanitizeAudioConfig(config: AudioJitterConfig): AudioJitterConfig {
    const normalized = normalizeAudioJitterConfig(config)
    const mode: AudioJitterBufferMode = normalized.mode ?? DEFAULT_AUDIO_JITTER_CONFIG.mode
    return { ...normalized, mode }
  }
}
