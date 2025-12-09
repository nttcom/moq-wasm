import { MoqtClientWrapper } from '@moqt/moqtClient'
import type { MOQTClient } from '../../../../pkg/moqt_client_sample'
import { MediaTransportState } from '../../../../utils/media/transportState'
import { sendVideoChunkViaMoqt, type VideoChunkSender } from '../../../../utils/media/videoTransport'
import { sendAudioChunkViaMoqt } from '../../../../utils/media/audioTransport'
import { serializeChunk } from '../../../../utils/media/chunk'
import { KEYFRAME_INTERVAL } from '../../../../utils/media/constants'
import { DEFAULT_VIDEO_ENCODING_SETTINGS, type VideoEncodingSettings } from '../types/videoEncoding'
import { DEFAULT_AUDIO_ENCODING_SETTINGS, type AudioEncodingSettings } from '../types/audioEncoding'

type LocalStreamHandler = (stream: MediaStream | null) => void

interface MediaPublisherHandlers {
  onLocalVideoStream?: LocalStreamHandler
  onLocalAudioStream?: LocalStreamHandler
  onEncodedVideoBitrate?: (mbps: number) => void
  onEncodedAudioBitrate?: (mbps: number) => void
  onVideoEncodeError?: (message: string) => void
  onAudioEncodeError?: (message: string) => void
  onAudioEncodingAdjusted?: (settings: AudioEncodingSettings) => void
  onScreenShareEncodingApplied?: (settings: VideoEncodingSettings) => void
}

export class MediaPublisher {
  private readonly transportState = new MediaTransportState()
  private handlers: MediaPublisherHandlers = {}
  private videoWorker: Worker | null = null
  private audioWorker: Worker | null = null
  private videoStream: MediaStream | null = null
  private audioStream: MediaStream | null = null
  private videoSource: 'camera' | 'screen' | null = null
  private activeVideoAliases = new Set<string>()
  private activeAudioAliases = new Set<string>()
  private videoSendQueue: Promise<void> = Promise.resolve()
  private audioSendQueue: Promise<void> = Promise.resolve()
  private readonly videoGroupStates = new Map<bigint, { groupId: bigint; lastObjectId: bigint }>()
  private videoEncodingSettings: VideoEncodingSettings = DEFAULT_VIDEO_ENCODING_SETTINGS
  private screenShareEncodingSettings: VideoEncodingSettings = {
    codec: 'av01.0.08M.08',
    width: 1920,
    height: 1080,
    bitrate: 1_000_000
  }
  private currentVideoEncodingInUse: VideoEncodingSettings = DEFAULT_VIDEO_ENCODING_SETTINGS
  private currentVideoDeviceId: string | null = null
  private readonly pendingEndOfGroup = new Map<bigint, bigint>()
  private audioEncodingSettings: AudioEncodingSettings = DEFAULT_AUDIO_ENCODING_SETTINGS
  private currentAudioDeviceId: string | null = null

  constructor(
    private readonly client: MoqtClientWrapper,
    private readonly trackNamespace: string[]
  ) {}

  setHandlers(handlers: MediaPublisherHandlers): void {
    this.handlers = handlers
  }

  async startVideo(deviceId?: string): Promise<void> {
    if (this.videoWorker) {
      return
    }
    const stream = await navigator.mediaDevices.getUserMedia({
      video: {
        width: this.videoEncodingSettings.width,
        height: this.videoEncodingSettings.height,
        frameRate: 30,
        ...(deviceId ? { deviceId: { exact: deviceId } } : {})
      },
      audio: false
    })
    this.currentVideoEncodingInUse = { ...this.videoEncodingSettings }
    this.currentVideoDeviceId = deviceId ?? null
    this.videoSource = 'camera'
    this.videoStream = stream
    this.handlers.onLocalVideoStream?.(stream)

    const [track] = stream.getVideoTracks()
    track.onended = () => {
      this.stopVideo().catch((e) => console.warn('Failed to stop video on track end', e))
    }
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
        | { type: 'configError'; reason: string; config: any }
        | {
            chunk: EncodedVideoChunk
            metadata: EncodedVideoChunkMetadata | undefined
          }
      if ('type' in data && data.type === 'bitrate') {
        this.handlers.onEncodedVideoBitrate?.(data.kbps)
        return
      }
      if ('type' in data && data.type === 'configError') {
        const cfg = data.config as { codec?: string; width?: number; height?: number; bitrate?: number }
        const codecText = cfg?.codec ? `codec ${cfg.codec}` : 'selected codec'
        const resText =
          cfg?.width && cfg?.height ? `resolution ${cfg.width}x${cfg.height}` : 'selected resolution/bitrate'
        this.handlers.onVideoEncodeError?.(
          `Encoder configuration unsupported (${codecText}, ${resText}). Please lower resolution/bitrate or choose another codec.`
        )
        return
      }
      const chunkData = 'type' in data ? data.chunk : data.chunk
      const metadata = 'type' in data ? data.metadata : data.metadata
      await this.handleVideoChunk(chunkData, metadata)
    }

    worker.postMessage({ type: 'encoderConfig', config: this.videoEncodingSettings })
    worker.postMessage({ type: 'keyframeInterval', keyframeInterval: KEYFRAME_INTERVAL })
    worker.postMessage({ type: 'videoStream', videoStream: readable }, [readable])

    this.videoWorker = worker
    console.log('Video encoding started')
  }

  async startScreenShare(): Promise<void> {
    if (this.videoWorker) {
      await this.stopVideo()
    }
    const stream = await navigator.mediaDevices.getDisplayMedia({
      video: {
        frameRate: 30
      },
      audio: false
    })
    const [track] = stream.getVideoTracks()
    const settings = track.getSettings()
    console.info('[screenShare] track settings', {
      width: settings.width,
      height: settings.height,
      frameRate: settings.frameRate,
      aspectRatio: settings.aspectRatio
    })
    const effectiveEncoding: VideoEncodingSettings = {
      ...this.screenShareEncodingSettings,
      width: settings.width ?? this.screenShareEncodingSettings.width,
      height: settings.height ?? this.screenShareEncodingSettings.height
    }
    this.currentVideoEncodingInUse = effectiveEncoding
    this.handlers.onScreenShareEncodingApplied?.(effectiveEncoding)
    this.currentVideoDeviceId = null
    this.videoSource = 'screen'
    this.videoStream = stream
    this.handlers.onLocalVideoStream?.(stream)

    track.onended = () => {
      this.stopVideo().catch((e) => console.warn('Failed to stop screen share on end', e))
    }

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
        | { type: 'configError'; reason: string; config: any }
        | {
            chunk: EncodedVideoChunk
            metadata: EncodedVideoChunkMetadata | undefined
          }
      if ('type' in data && data.type === 'bitrate') {
        this.handlers.onEncodedVideoBitrate?.(data.kbps)
        return
      }
      if ('type' in data && data.type === 'configError') {
        const cfg = data.config as { codec?: string; width?: number; height?: number; bitrate?: number }
        const codecText = cfg?.codec ? `codec ${cfg.codec}` : 'selected codec'
        const resText =
          cfg?.width && cfg?.height ? `resolution ${cfg.width}x${cfg.height}` : 'selected resolution/bitrate'
        this.handlers.onVideoEncodeError?.(
          `Encoder configuration unsupported (${codecText}, ${resText}). Please lower resolution/bitrate or choose another codec.`
        )
        return
      }
      const chunkData = 'type' in data ? data.chunk : data.chunk
      const metadata = 'type' in data ? data.metadata : data.metadata
      await this.handleVideoChunk(chunkData, metadata)
    }

    worker.postMessage({ type: 'encoderConfig', config: effectiveEncoding })
    worker.postMessage({ type: 'keyframeInterval', keyframeInterval: KEYFRAME_INTERVAL })
    worker.postMessage({ type: 'videoStream', videoStream: readable }, [readable])

    this.videoWorker = worker
    console.log('Screen share encoding started')
  }

  async stopVideo(): Promise<void> {
    await this.flushPendingEndOfGroup()
    this.videoWorker?.terminate()
    this.videoWorker = null
    this.videoStream?.getTracks().forEach((track) => track.stop())
    this.videoStream = null
    this.currentVideoDeviceId = null
    this.videoSource = null
    this.currentVideoEncodingInUse = { ...this.videoEncodingSettings }
    this.activeVideoAliases.clear()
    this.videoSendQueue = Promise.resolve()
    this.videoGroupStates.clear()
    this.handlers.onLocalVideoStream?.(null)
  }

  async startAudio(deviceId?: string): Promise<void> {
    if (this.audioWorker) {
      return
    }
    const desiredChannels = this.audioEncodingSettings.channels
    const stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        echoCancellation: true,
        noiseSuppression: true,
        channelCount: { ideal: desiredChannels },
        ...(deviceId ? { deviceId: { exact: deviceId } } : {})
      },
      video: false
    })
    this.currentAudioDeviceId = deviceId ?? null
    this.audioStream = stream
    this.handlers.onLocalAudioStream?.(stream)

    const [track] = stream.getAudioTracks()
    const actualChannels = track.getSettings().channelCount ?? desiredChannels
    let effectiveChannels = desiredChannels
    if (desiredChannels > 1 && (!actualChannels || actualChannels < desiredChannels)) {
      effectiveChannels = actualChannels > 0 ? actualChannels : 1
      this.audioEncodingSettings = { ...this.audioEncodingSettings, channels: effectiveChannels }
      this.handlers.onAudioEncodeError?.('Stereo capture is not available on this device. Falling back to mono.')
      this.handlers.onAudioEncodingAdjusted?.(this.audioEncodingSettings)
    }

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
        | { type: 'configError'; media: 'audio'; reason: string; config: any }
        | {
            chunk: EncodedAudioChunk
            metadata: EncodedAudioChunkMetadata | undefined
          }
      if ('type' in data && data.type === 'bitrate') {
        this.handlers.onEncodedAudioBitrate?.(data.kbps)
        return
      }
      if ('type' in data && data.type === 'configError') {
        const cfg = data.config as { codec?: string; bitrate?: number }
        const codecText = cfg?.codec ?? 'selected codec'
        const suggestion =
          cfg?.codec && cfg.codec.startsWith('mp4a')
            ? 'Please choose a higher bitrate for AAC.'
            : 'Please choose another codec or adjust bitrate.'
        this.handlers.onAudioEncodeError?.(`Audio encoder configuration unsupported (${codecText}). ${suggestion}`)
        return
      }
      const chunkData = 'type' in data ? data.chunk : data.chunk
      const metadata = 'type' in data ? data.metadata : data.metadata
      await this.handleAudioChunk(chunkData, metadata)
    }

    const encoderConfig: AudioEncoderConfig = {
      codec: this.audioEncodingSettings.codec,
      sampleRate: 48_000,
      numberOfChannels: effectiveChannels,
      bitrate: this.audioEncodingSettings.bitrate
    }

    worker.postMessage({ type: 'config', config: encoderConfig })
    worker.postMessage({ audioStream: readable }, [readable])
    this.audioWorker = worker
  }

  async stopAudio(): Promise<void> {
    this.audioWorker?.terminate()
    this.audioWorker = null
    this.audioStream?.getTracks().forEach((track) => track.stop())
    this.audioStream = null
    this.currentAudioDeviceId = null
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
        sender: this.sendVideoObject,
        fallbackCodec: this.currentVideoEncodingInUse.codec
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
        transportState: this.transportState,
        fallbackCodec: this.audioEncodingSettings.codec,
        fallbackSampleRate: 48_000,
        fallbackChannels: this.audioEncodingSettings.channels
      })
    })
  }

  async setVideoEncodingSettings(settings: VideoEncodingSettings, deviceId?: string, restartIfActive: boolean = false) {
    this.videoEncodingSettings = settings
    this.currentVideoEncodingInUse = { ...settings }
    if (restartIfActive && this.videoWorker) {
      const targetDevice = deviceId ?? this.currentVideoDeviceId ?? undefined
      await this.forceKeyframeAndRestart(targetDevice)
    }
  }

  async setScreenShareEncodingSettings(settings: VideoEncodingSettings): Promise<void> {
    this.screenShareEncodingSettings = settings
    if (this.videoSource === 'screen' && this.videoWorker) {
      this.currentVideoEncodingInUse = { ...settings }
      this.videoWorker.postMessage({ type: 'encoderConfig', config: settings })
      this.handlers.onScreenShareEncodingApplied?.(settings)
    }
  }

  async setAudioEncodingSettings(settings: AudioEncodingSettings, restartIfActive: boolean = false) {
    this.audioEncodingSettings = settings
    if (this.audioWorker) {
      if (restartIfActive) {
        await this.stopAudio()
        await this.startAudio(this.currentAudioDeviceId ?? undefined)
      } else {
        const desiredChannels = this.audioEncodingSettings.channels
        this.audioWorker.postMessage({
          type: 'config',
          config: {
            codec: this.audioEncodingSettings.codec,
            sampleRate: 48_000,
            numberOfChannels: desiredChannels,
            bitrate: this.audioEncodingSettings.bitrate
          }
        })
      }
    }
  }

  private async forceKeyframeAndRestart(deviceId?: string): Promise<void> {
    try {
      await this.flushPendingEndOfGroup()
      await this.stopVideo()
    } catch (e) {
      console.warn('Failed to stop video before restart', e)
    }
    await this.startVideo(deviceId)
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
    client,
    extraMetadata
  ) => {
    const previousState = this.videoGroupStates.get(trackAlias)
    if (previousState && previousState.groupId !== groupId) {
      const endObjectId = previousState.lastObjectId + 1n
      this.pendingEndOfGroup.set(trackAlias, endObjectId)
      await this.sendEndOfGroup(client, trackAlias, previousState.groupId, endObjectId)
    }

    const payload = serializeChunk(
      {
        type: chunk.type,
        timestamp: chunk.timestamp,
        duration: chunk.duration ?? null,
        byteLength: chunk.byteLength,
        copyTo: (dest) => chunk.copyTo(dest)
      },
      extraMetadata
    )

    await client.sendSubgroupStreamObject(trackAlias, groupId, subgroupId, objectId, undefined, payload)

    this.videoGroupStates.set(trackAlias, { groupId, lastObjectId: objectId })
  }

  private async flushPendingEndOfGroup(): Promise<void> {
    const client = this.client.getRawClient()
    if (!client) return
    for (const [alias, endObjectId] of this.pendingEndOfGroup.entries()) {
      const state = this.videoGroupStates.get(alias)
      const groupId = state?.groupId ?? this.transportState.getVideoGroupId()
      await this.sendEndOfGroup(client, alias, groupId, endObjectId)
    }
    this.pendingEndOfGroup.clear()
  }

  private async sendEndOfGroup(client: MOQTClient, trackAlias: bigint, groupId: bigint, endObjectId: bigint) {
    await client.sendSubgroupStreamObject(trackAlias, groupId, BigInt(0), endObjectId, 3, new Uint8Array(0))
    console.debug(
      `[MediaPublisher] Sent EndOfGroup trackAlias=${trackAlias} groupId=${groupId} subgroupId=0 objectId=${endObjectId}`
    )
  }
}
