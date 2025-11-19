import { MoqtClientWrapper } from '@moqt/moqtClient'
import { SubgroupStreamObjectMessage } from '../../../../pkg/moqt_client_sample'

interface MediaSubscriberHandlers {
  onRemoteVideoStream?: (userId: string, stream: MediaStream) => void
  onRemoteAudioStream?: (userId: string, stream: MediaStream) => void
  onRemoteVideoBitrate?: (userId: string, mbps: number) => void
  onRemoteAudioBitrate?: (userId: string, mbps: number) => void
  onRemoteVideoLatency?: (userId: string, ms: number) => void
  onRemoteAudioLatency?: (userId: string, ms: number) => void
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
  baseTimestamp: number | null
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
        | { frame: VideoFrame }
      if ('type' in data && data.type === 'bitrate') {
        this.handlers.onRemoteVideoBitrate?.(userId, data.kbps)
        return
      }
      if ('type' in data && data.type === 'latency') {
        this.handlers.onRemoteVideoLatency?.(userId, data.ms)
        return
      }
      const frame = 'type' in data ? data.frame : data.frame
      await writer.ready
      await writer.write(frame)
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
      stream: new MediaStream([generator]),
      baseTimestamp: null
    }
    worker.onmessage = async (event: MessageEvent) => {
      const data = event.data as
        | { type: 'audioData'; audioData: AudioData }
        | { type: 'bitrate'; kbps: number }
        | { type: 'latency'; media: 'audio'; ms: number }
        | { audioData: AudioData }
      if ('type' in data && data.type === 'bitrate') {
        this.handlers.onRemoteAudioBitrate?.(userId, data.kbps)
        return
      }
      if ('type' in data && data.type === 'latency') {
        this.handlers.onRemoteAudioLatency?.(userId, data.ms)
        return
      }
      const audioData = 'type' in data ? data.audioData : data.audioData
      const retimedAudioData = this.retimeAudioData(audioData, context)
      console.debug(audioData.timestamp, '->', retimedAudioData.timestamp)
      await writer.ready
      await writer.write(retimedAudioData)
      audioData.close()
    }

    this.audioContexts.set(trackAlias, context)
    this.handlers.onRemoteAudioStream?.(userId, context.stream)

    this.client.setOnSubgroupObjectHandler(trackAlias, (groupId, message) =>
      this.forwardToWorker(worker, trackAlias, groupId, message)
    )
  }

  private forwardToWorker(worker: Worker, trackAlias: bigint, groupId: bigint, message: SubgroupStreamObjectMessage) {
    if (message.objectStatus === 3) {
      console.log(`[MediaSubscriber] Received EndOfGroup trackAlias=${trackAlias} groupId=${groupId}`)
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

  private retimeAudioData(audioData: AudioData, context: AudioSubscriptionContext): AudioData {
    const originalTimestamp = audioData.timestamp
    if (context.baseTimestamp === null) {
      context.baseTimestamp = originalTimestamp
    }
    const playbackTimestamp = originalTimestamp - context.baseTimestamp
    return this.cloneAudioDataWithTimestamp(audioData, playbackTimestamp)
  }

  private cloneAudioDataWithTimestamp(audioData: AudioData, timestamp: number): AudioData {
    const options: AudioDataCopyToOptions = {
      planeIndex: 0,
      frameCount: audioData.numberOfFrames
    }
    const byteLength = audioData.allocationSize(options)
    const format = audioData.format as AudioSampleFormat
    const buffer = new ArrayBuffer(byteLength)
    const view = this.createAudioBufferView(format, buffer)
    audioData.copyTo(view, options)
    return new AudioData({
      format,
      sampleRate: audioData.sampleRate,
      numberOfFrames: audioData.numberOfFrames,
      numberOfChannels: audioData.numberOfChannels,
      timestamp,
      data: buffer
    })
  }

  private createAudioBufferView(format: AudioSampleFormat, buffer: ArrayBuffer): ArrayBufferView {
    switch (format) {
      case 'u8':
      case 'u8-planar':
        return new Uint8Array(buffer)
      case 's16':
      case 's16-planar':
        return new Int16Array(buffer)
      case 's32':
      case 's32-planar':
        return new Int32Array(buffer)
      case 'f32':
      case 'f32-planar':
      default:
        return new Float32Array(buffer)
    }
  }
}
