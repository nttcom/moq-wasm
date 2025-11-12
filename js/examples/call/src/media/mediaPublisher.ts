import { MoqtClientWrapper } from '@moqt/moqtClient'
import type { MOQTClient } from '../../../../pkg/moqt_client_sample'
import { MediaTransportState } from '../../../../utils/media/transportState'
import { sendVideoChunkViaMoqt, type VideoChunkSender } from '../../../../utils/media/videoTransport'
import { sendAudioChunkViaMoqt } from '../../../../utils/media/audioTransport'
import { serializeChunk } from '../../../../utils/media/chunk'
import { KEYFRAME_INTERVAL } from '../../../../utils/media/constants'

type LocalStreamHandler = (stream: MediaStream | null) => void

interface MediaPublisherHandlers {
  onLocalVideoStream?: LocalStreamHandler
  onLocalAudioStream?: LocalStreamHandler
  onEncodedVideoBitrate?: (mbps: number) => void
  onEncodedAudioBitrate?: (mbps: number) => void
}

export class MediaPublisher {
  private readonly transportState = new MediaTransportState()
  private handlers: MediaPublisherHandlers = {}
  private videoWorker: Worker | null = null
  private audioWorker: Worker | null = null
  private videoStream: MediaStream | null = null
  private audioStream: MediaStream | null = null
  private activeVideoAliases = new Set<string>()
  private activeAudioAliases = new Set<string>()
  private videoSendQueue: Promise<void> = Promise.resolve()
  private audioSendQueue: Promise<void> = Promise.resolve()
  private readonly videoGroupStates = new Map<bigint, { groupId: bigint; lastObjectId: bigint }>()

  constructor(
    private readonly client: MoqtClientWrapper,
    private readonly trackNamespace: string[]
  ) {}

  setHandlers(handlers: MediaPublisherHandlers): void {
    this.handlers = handlers
  }

  async startVideo(): Promise<void> {
    if (this.videoWorker) {
      return
    }
    const stream = await navigator.mediaDevices.getUserMedia({
      video: { width: 1280, height: 720, frameRate: 30 },
      audio: false
    })
    this.videoStream = stream
    this.handlers.onLocalVideoStream?.(stream)

    const [track] = stream.getVideoTracks()
    const processor = new MediaStreamTrackProcessor({ track })
    const readable = processor.readable

    const worker = new Worker(new URL('../../../../utils/media/encoders/videoEncoder.ts', import.meta.url), {
      type: 'module'
    })

    worker.onmessage = async (event: MessageEvent) => {
      const data = event.data as
        | {
            type: 'chunk'
            chunk: EncodedVideoChunk
            metadata: EncodedVideoChunkMetadata | undefined
          }
        | {
            type: 'bitrate'
            kbps: number
          }
        | {
            chunk: EncodedVideoChunk
            metadata: EncodedVideoChunkMetadata | undefined
          }
      if ('type' in data && data.type === 'bitrate') {
        this.handlers.onEncodedVideoBitrate?.(data.kbps)
        return
      }
      const chunkData = 'type' in data ? data.chunk : data.chunk
      const metadata = 'type' in data ? data.metadata : data.metadata
      await this.handleVideoChunk(chunkData, metadata)
    }

    worker.postMessage({ type: 'keyframeInterval', keyframeInterval: KEYFRAME_INTERVAL })
    worker.postMessage({ type: 'videoStream', videoStream: readable }, [readable])

    this.videoWorker = worker
    console.log('Video encoding started')
  }

  async stopVideo(): Promise<void> {
    this.videoWorker?.terminate()
    this.videoWorker = null
    this.videoStream?.getTracks().forEach((track) => track.stop())
    this.videoStream = null
    this.activeVideoAliases.clear()
    this.videoSendQueue = Promise.resolve()
    this.videoGroupStates.clear()
    this.handlers.onLocalVideoStream?.(null)
  }

  async startAudio(): Promise<void> {
    if (this.audioWorker) {
      return
    }
    const stream = await navigator.mediaDevices.getUserMedia({
      audio: { echoCancellation: true, noiseSuppression: true },
      video: false
    })
    this.audioStream = stream
    this.handlers.onLocalAudioStream?.(stream)

    const [track] = stream.getAudioTracks()
    const processor = new MediaStreamTrackProcessor({ track })
    const readable = processor.readable

    const worker = new Worker(new URL('../../../../utils/media/encoders/audioEncoder.ts', import.meta.url), {
      type: 'module'
    })

    worker.onmessage = async (event: MessageEvent) => {
      const data = event.data as
        | {
            type: 'chunk'
            chunk: EncodedAudioChunk
            metadata: EncodedAudioChunkMetadata | undefined
          }
        | {
            type: 'bitrate'
            kbps: number
          }
        | {
            chunk: EncodedAudioChunk
            metadata: EncodedAudioChunkMetadata | undefined
          }
      if ('type' in data && data.type === 'bitrate') {
        this.handlers.onEncodedAudioBitrate?.(data.kbps)
        return
      }
      const chunkData = 'type' in data ? data.chunk : data.chunk
      const metadata = 'type' in data ? data.metadata : data.metadata
      await this.handleAudioChunk(chunkData, metadata)
    }

    worker.postMessage({ audioStream: readable }, [readable])
    this.audioWorker = worker
  }

  async stopAudio(): Promise<void> {
    this.audioWorker?.terminate()
    this.audioWorker = null
    this.audioStream?.getTracks().forEach((track) => track.stop())
    this.audioStream = null
    this.activeAudioAliases.clear()
    this.audioSendQueue = Promise.resolve()
    this.handlers.onLocalAudioStream?.(null)
  }

  private handleVideoChunk(chunk: EncodedVideoChunk, metadata: EncodedVideoChunkMetadata | undefined) {
    const aliases = this.collectAliasesSafe('video')
    if (!aliases.length) {
      return
    }

    this.enqueueVideoSend(async (client) => {
      await sendVideoChunkViaMoqt({
        chunk,
        metadata,
        trackAliases: aliases,
        publisherPriority: 0,
        client,
        transportState: this.transportState,
        sender: this.sendVideoObject
      })
    })
  }

  private handleAudioChunk(chunk: EncodedAudioChunk, metadata: EncodedAudioChunkMetadata | undefined) {
    const aliases = this.collectAliasesSafe('audio')
    if (!aliases.length) {
      return
    }

    this.enqueueAudioSend(async (client) => {
      await sendAudioChunkViaMoqt({
        chunk,
        metadata,
        trackAliases: aliases,
        client,
        transportState: this.transportState
      })
    })
  }

  private collectAliasesSafe(trackName: 'video' | 'audio'): bigint[] {
    const rawClient = this.client.getRawClient()
    if (!rawClient) {
      return []
    }

    return this.collectAliases(rawClient, trackName)
  }

  private collectAliases(client: MOQTClient, trackName: 'video' | 'audio'): bigint[] {
    const subscribed = client.getTrackSubscribers(this.trackNamespace, trackName)
    const aliases = Array.from(subscribed ?? [], (value) => BigInt(value))

    const activeSet = trackName === 'video' ? this.activeVideoAliases : this.activeAudioAliases
    const latestKeys = new Set(aliases.map((alias) => alias.toString()))
    for (const key of activeSet) {
      if (!latestKeys.has(key)) {
        this.transportState.resetAlias(key)
        this.videoGroupStates.delete(BigInt(key))
      }
    }
    activeSet.clear()
    for (const alias of aliases) {
      activeSet.add(alias.toString())
    }
    return aliases
  }

  private enqueueVideoSend(task: (client: MOQTClient) => Promise<void>): void {
    this.videoSendQueue = this.videoSendQueue
      .then(async () => {
        const client = this.client.getRawClient()
        if (!client) {
          return
        }
        await task(client)
      })
      .catch((error) => {
        console.error('Video send failed:', error)
      })
  }

  private enqueueAudioSend(task: (client: MOQTClient) => Promise<void>): void {
    this.audioSendQueue = this.audioSendQueue
      .then(async () => {
        const client = this.client.getRawClient()
        if (!client) {
          return
        }
        await task(client)
      })
      .catch((error) => {
        console.error('Audio send failed:', error)
      })
  }

  private readonly sendVideoObject: VideoChunkSender = async (
    trackAlias,
    groupId,
    subgroupId,
    objectId,
    chunk,
    client
  ) => {
    const previousState = this.videoGroupStates.get(trackAlias)
    if (previousState && previousState.groupId !== groupId) {
      const endObjectId = previousState.lastObjectId + 1n
      await client.sendSubgroupStreamObject(
        trackAlias,
        previousState.groupId,
        BigInt(0),
        endObjectId,
        3, // 0x3: EndOfGroup
        new Uint8Array(0)
      )
      console.log(
        `[MediaPublisher] Sent EndOfGroup trackAlias=${trackAlias} groupId=${previousState.groupId} subgroupId=0 objectId=${endObjectId}`
      )
    }

    const payload = serializeChunk({
      type: chunk.type,
      timestamp: chunk.timestamp,
      duration: chunk.duration ?? null,
      byteLength: chunk.byteLength,
      copyTo: (dest) => chunk.copyTo(dest)
    })

    await client.sendSubgroupStreamObject(trackAlias, groupId, subgroupId, objectId, undefined, payload)

    this.videoGroupStates.set(trackAlias, { groupId, lastObjectId: objectId })
  }
}
