import { MoqtClientWrapper } from '@moqt/moqtClient'
import { SubgroupStreamObjectMessage } from '../../../../pkg/moqt_client_sample'

interface MediaSubscriberHandlers {
  onRemoteVideoStream?: (userId: string, stream: MediaStream) => void
  onRemoteAudioStream?: (userId: string, stream: MediaStream) => void
  onRemoteVideoBitrate?: (userId: string, mbps: number) => void
  onRemoteAudioBitrate?: (userId: string, mbps: number) => void
  onRemoteVideoLatency?: (userId: string, ms: number) => void
  onRemoteAudioLatency?: (userId: string, ms: number) => void
  onRemoteVideoJitter?: (userId: string, ms: number) => void
  onRemoteAudioJitter?: (userId: string, ms: number) => void
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
        | { type: 'frame'; frame: VideoFrame }
        | { type: 'bitrate'; kbps: number }
        | { type: 'latency'; media: 'video'; ms: number }
        | { type: 'jitter'; media: 'video'; ms: number }
      if (data.type === 'bitrate') {
        this.handlers.onRemoteVideoBitrate?.(userId, data.kbps)
        return
      }
      if (data.type === 'latency') {
        this.handlers.onRemoteVideoLatency?.(userId, data.ms)
        return
      }
      if (data.type === 'jitter') {
        this.handlers.onRemoteVideoJitter?.(userId, data.ms)
        return
      }
      await writer.ready
      await writer.write(data.frame)
    }

    const stream = new MediaStream([generator])
    this.videoContexts.set(trackAlias, { userId, worker, writer, stream })
    this.handlers.onRemoteVideoStream?.(userId, stream)

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
        | { type: 'latency'; media: 'audio'; ms: number }
        | { type: 'jitter'; media: 'audio'; ms: number }
      if (data.type === 'bitrate') {
        this.handlers.onRemoteAudioBitrate?.(userId, data.kbps)
        return
      }
      if (data.type === 'latency') {
        this.handlers.onRemoteAudioLatency?.(userId, data.ms)
        return
      }
      if (data.type === 'jitter') {
        this.handlers.onRemoteAudioJitter?.(userId, data.ms)
        return
      }
      const audioData = data.audioData
      await writer.ready
      await writer.write(audioData)
    }

    this.audioContexts.set(trackAlias, context)
    this.handlers.onRemoteAudioStream?.(userId, context.stream)

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
}
