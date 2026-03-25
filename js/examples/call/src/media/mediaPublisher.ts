import { MoqtClientWrapper } from '@moqt/moqtClient'
import type { MOQTClient } from '../../../../pkg/moqt_client_wasm'
import { MediaTransportState } from '../../../../utils/media/transportState'
import { sendVideoChunkViaMoqt, type VideoChunkSender } from '../../../../utils/media/videoTransport'
import { sendAudioChunkViaMoqt } from '../../../../utils/media/audioTransport'
import { serializeChunk } from '../../../../utils/media/chunk'
import type { LocHeader } from '../../../../utils/media/loc'
import { DEFAULT_VIDEO_ENCODING_SETTINGS, type VideoEncodingSettings } from '../types/videoEncoding'
import { DEFAULT_AUDIO_ENCODING_SETTINGS, type AudioEncodingSettings } from '../types/audioEncoding'
import type { AudioCaptureConstraints, CameraCaptureConstraints } from '../types/captureConstraints'
import { buildCallCatalogJson, getDefaultCallCatalogTracks } from './callCatalog'
import {
  DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS,
  DEFAULT_VIDEO_KEYFRAME_INTERVAL,
  type CallCatalogTrack
} from '../types/catalog'
import { isScreenShareTrackName } from '../utils/catalogTrackName'

type LocalStreamHandler = (stream: MediaStream | null) => void
type VideoSource = 'camera' | 'screenshare'

interface MediaPublisherHandlers {
  onLocalCameraStream?: LocalStreamHandler
  onLocalScreenShareStream?: LocalStreamHandler
  onLocalAudioStream?: LocalStreamHandler
  onEncodedVideoBitrate?: (mbps: number) => void
  onEncodedAudioBitrate?: (mbps: number) => void
  onVideoEncodeError?: (message: string) => void
  onAudioEncodeError?: (message: string) => void
  onAudioEncodingAdjusted?: (settings: AudioEncodingSettings) => void
  onScreenShareEncodingApplied?: (settings: VideoEncodingSettings) => void
  onLocalVideoSendTiming?: (
    timing: {
      captureToEncodeDoneMs: number | null
      encodeQueueSize: number
      queueWaitMs: number
      sendActiveMs: number
      objectSendMs: number
      serializeMs: number
      endOfGroupMs: number
      queueDepth: number
      objectBytes: number
      objectCount: number
      aliasCount: number
      keyframe: boolean
    } | null,
    source: VideoSource
  ) => void
}

type GroupState = { groupId: bigint; lastObjectId: bigint }

type VideoTrackEncoderContext = {
  trackName: string
  source: VideoSource
  track: MediaStreamTrack
  readable: ReadableStream<VideoFrame> | null
  worker: Worker
  encodingStarted: boolean
  config: VideoEncodingSettings
  transportState: MediaTransportState
  sendQueue: Promise<void>
  pendingSendJobs: number
  activeAliases: Set<string>
  videoGroupStates: Map<bigint, GroupState>
}

type VideoSendTimingAccumulator = {
  objectSendMs: number
  serializeMs: number
  endOfGroupMs: number
  objectBytes: number
  objectCount: number
}

type AudioTrackEncoderContext = {
  trackName: string
  track: MediaStreamTrack
  readable: ReadableStream<AudioData> | null
  worker: Worker
  encodingStarted: boolean
  config: AudioEncodingSettings
  streamUpdateMode: 'single' | 'interval'
  streamUpdateIntervalSeconds: number
  streamUpdateTimer: ReturnType<typeof setInterval> | null
  streamUpdateInFlight: boolean
  transportState: MediaTransportState
  sendQueue: Promise<void>
  activeAliases: Set<string>
}

export type SubscribedCatalogTrack = CallCatalogTrack & {
  subscriberCount: number
}

const CATALOG_TRACK_NAME = 'catalog'
const CHAT_TRACK_NAME = 'chat'

export class MediaPublisher {
  private handlers: MediaPublisherHandlers = {}
  private catalogTracks: CallCatalogTrack[] = getDefaultCallCatalogTracks()

  private readonly videoTrackContexts = new Map<string, VideoTrackEncoderContext>()
  private readonly audioTrackContexts = new Map<string, AudioTrackEncoderContext>()

  private readonly videoBitrateByTrackName = new Map<string, number>()
  private readonly audioBitrateByTrackName = new Map<string, number>()

  private cameraStream: MediaStream | null = null
  private screenShareStream: MediaStream | null = null
  private audioStream: MediaStream | null = null

  private videoEncodingSettings: VideoEncodingSettings = DEFAULT_VIDEO_ENCODING_SETTINGS
  private screenShareEncodingSettings: VideoEncodingSettings = {
    codec: 'av01.0.08M.08',
    width: 1920,
    height: 1080,
    bitrate: 1_000_000,
    framerate: 30,
    hardwareAcceleration: 'prefer-software'
  }
  private audioEncodingSettings: AudioEncodingSettings = DEFAULT_AUDIO_ENCODING_SETTINGS

  private currentCameraDeviceId: string | null = null
  private currentAudioDeviceId: string | null = null

  private videoCaptureConstraints: CameraCaptureConstraints = { frameRate: 30 }
  private audioCaptureConstraints: AudioCaptureConstraints = {
    echoCancellation: true,
    noiseSuppression: true,
    autoGainControl: true
  }
  private readonly catalogGroupByAlias = new Map<string, bigint>()

  constructor(
    private readonly client: MoqtClientWrapper,
    private readonly trackNamespace: string[]
  ) {}

  setHandlers(handlers: MediaPublisherHandlers): void {
    this.handlers = handlers
  }

  getCatalogTracks(): CallCatalogTrack[] {
    return this.catalogTracks.map((track) => ({ ...track }))
  }

  getSubscribedCatalogTracks(): SubscribedCatalogTrack[] {
    const client = this.client.getRawClient()
    if (!client) {
      return []
    }
    const tracks: SubscribedCatalogTrack[] = []
    for (const track of this.catalogTracks) {
      const subscriberCount = Array.from(client.getTrackSubscribers(this.trackNamespace, track.name)).length
      if (subscriberCount <= 0) {
        continue
      }
      tracks.push({ ...track, subscriberCount })
    }
    return tracks
  }

  async setCatalogTracks(tracks: CallCatalogTrack[]): Promise<void> {
    this.catalogTracks = this.normalizeCatalogTracks(tracks)
    this.syncVideoTrackContexts('camera')
    this.syncVideoTrackContexts('screenshare')
    this.syncAudioTrackContexts()
    await this.broadcastCatalog()
  }

  resolveTrackRole(trackName: string): CallCatalogTrack['role'] | null {
    const track = this.catalogTracks.find((entry) => entry.name === trackName)
    return track?.role ?? null
  }

  isKnownPublishTrack(trackName: string): boolean {
    return (
      trackName === CHAT_TRACK_NAME || trackName === CATALOG_TRACK_NAME || this.resolveTrackRole(trackName) !== null
    )
  }

  isCatalogTrack(trackName: string): boolean {
    return trackName === CATALOG_TRACK_NAME
  }

  async sendCatalogToAlias(trackAlias: bigint): Promise<void> {
    const client = this.client.getRawClient()
    if (!client) {
      return
    }
    await this.sendCatalogObject(client, trackAlias)
  }

  handleIncomingUnsubscribe(subscribeId: bigint): void {
    this.handleSubscriptionEnded('unsubscribe', subscribeId)
  }

  handleSubscribeDone(subscribeId: bigint): void {
    this.handleSubscriptionEnded('subscribe_done', subscribeId)
  }

  private handleSubscriptionEnded(type: 'unsubscribe' | 'subscribe_done', subscribeId: bigint): void {
    void type
    void subscribeId
    for (const [trackName, context] of this.videoTrackContexts.entries()) {
      if (!context.encodingStarted) {
        continue
      }
      queueMicrotask(() => this.suspendVideoTrackEncodingIfNoSubscribers(trackName, context))
    }
    for (const [trackName, context] of this.audioTrackContexts.entries()) {
      if (!context.encodingStarted) {
        continue
      }
      queueMicrotask(() => this.suspendAudioTrackEncodingIfNoSubscribers(trackName, context))
    }
  }

  setVideoCaptureConstraints(constraints: CameraCaptureConstraints): void {
    this.videoCaptureConstraints = { ...this.videoCaptureConstraints, ...constraints }
  }

  setAudioCaptureConstraints(constraints: AudioCaptureConstraints): void {
    this.audioCaptureConstraints = { ...this.audioCaptureConstraints, ...constraints }
  }

  async startCamera(deviceId?: string, constraints?: CameraCaptureConstraints): Promise<void> {
    if (this.cameraStream) {
      return
    }

    const effectiveConstraints: CameraCaptureConstraints = {
      frameRate: 30,
      ...this.videoCaptureConstraints,
      ...constraints
    }

    const stream = await navigator.mediaDevices.getUserMedia({
      video: {
        width: effectiveConstraints.width ?? this.videoEncodingSettings.width,
        height: effectiveConstraints.height ?? this.videoEncodingSettings.height,
        frameRate: effectiveConstraints.frameRate,
        ...(deviceId ? { deviceId: { exact: deviceId } } : {})
      },
      audio: false
    })

    this.currentCameraDeviceId = deviceId ?? null
    this.cameraStream = stream
    this.handlers.onLocalCameraStream?.(stream)

    const [track] = stream.getVideoTracks()
    track.onended = () => {
      this.stopCamera().catch((error) => console.warn('Failed to stop camera on track end', error))
    }

    this.syncVideoTrackContexts('camera')
    console.log('Camera capture started')
  }

  async stopCamera(): Promise<void> {
    this.stopVideoTrackContextsBySource('camera')
    this.cameraStream?.getTracks().forEach((track) => {
      track.onended = null
      track.stop()
    })
    this.cameraStream = null
    this.currentCameraDeviceId = null
    this.handlers.onLocalCameraStream?.(null)
  }

  async startScreenShare(): Promise<void> {
    if (this.screenShareStream) {
      return
    }

    const stream = await navigator.mediaDevices.getDisplayMedia({
      video: {
        frameRate: 30
      },
      audio: false
    })

    const [track] = stream.getVideoTracks()

    this.screenShareStream = stream
    this.handlers.onLocalScreenShareStream?.(stream)

    track.onended = () => {
      this.stopScreenShare().catch((error) => console.warn('Failed to stop screen share on track end', error))
    }

    this.syncVideoTrackContexts('screenshare')
    this.handlers.onScreenShareEncodingApplied?.(this.screenShareEncodingSettings)
    console.log('Screen share capture started')
  }

  async stopScreenShare(): Promise<void> {
    this.stopVideoTrackContextsBySource('screenshare')
    this.screenShareStream?.getTracks().forEach((track) => {
      track.onended = null
      track.stop()
    })
    this.screenShareStream = null
    this.handlers.onLocalScreenShareStream?.(null)
  }

  async startAudio(deviceId?: string, constraints?: AudioCaptureConstraints): Promise<void> {
    if (this.audioStream) {
      return
    }

    const desiredChannels = this.audioEncodingSettings.channels
    const effectiveAudioConstraints: AudioCaptureConstraints = {
      ...this.audioCaptureConstraints,
      ...constraints
    }

    const stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        echoCancellation: effectiveAudioConstraints.echoCancellation,
        noiseSuppression: effectiveAudioConstraints.noiseSuppression,
        autoGainControl: effectiveAudioConstraints.autoGainControl,
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
    if (desiredChannels > 1 && (!actualChannels || actualChannels < desiredChannels)) {
      const effectiveChannels = actualChannels > 0 ? actualChannels : 1
      this.audioEncodingSettings = { ...this.audioEncodingSettings, channels: effectiveChannels }
      this.handlers.onAudioEncodeError?.('Stereo capture is not available on this device. Falling back to mono.')
      this.handlers.onAudioEncodingAdjusted?.(this.audioEncodingSettings)
    }

    this.syncAudioTrackContexts()
  }

  async stopAudio(): Promise<void> {
    this.stopAllAudioTrackContexts()
    this.audioStream?.getTracks().forEach((track) => track.stop())
    this.audioStream = null
    this.currentAudioDeviceId = null
    this.handlers.onLocalAudioStream?.(null)
  }

  async setVideoEncodingSettings(settings: VideoEncodingSettings, deviceId?: string, restartIfActive: boolean = false) {
    this.videoEncodingSettings = settings
    this.syncVideoTrackContexts('camera')
    if (restartIfActive && this.cameraStream) {
      const targetDevice = deviceId ?? this.currentCameraDeviceId ?? undefined
      await this.forceKeyframeAndRestartCamera(targetDevice)
    }
  }

  async setScreenShareEncodingSettings(settings: VideoEncodingSettings): Promise<void> {
    this.screenShareEncodingSettings = settings
    this.syncVideoTrackContexts('screenshare')
    this.handlers.onScreenShareEncodingApplied?.(settings)
  }

  async setAudioEncodingSettings(settings: AudioEncodingSettings, restartIfActive: boolean = false) {
    this.audioEncodingSettings = settings
    if (restartIfActive && this.audioStream) {
      await this.restartAudioForNewSubscriber()
      return
    }
    this.syncAudioTrackContexts()
  }

  async restartCameraForNewSubscriber(): Promise<void> {
    this.restartVideoTrackContextsBySource('camera')
  }

  async restartScreenShareForNewSubscriber(): Promise<void> {
    this.restartVideoTrackContextsBySource('screenshare')
  }

  async restartAudioForNewSubscriber(): Promise<void> {
    this.restartAllAudioTrackContexts()
  }

  async applyVideoEncodingForTrack(trackName: string): Promise<void> {
    const track = this.catalogTracks.find((entry) => entry.name === trackName && entry.role === 'video')
    if (!track) {
      return
    }
    const source: VideoSource = isScreenShareTrackName(trackName) ? 'screenshare' : 'camera'
    const fallback = source === 'screenshare' ? this.screenShareEncodingSettings : this.videoEncodingSettings
    const settings = this.buildVideoEncodingFromTrack(track, fallback)
    const keyframeInterval = this.normalizeTrackKeyframeInterval(track.keyframeInterval)

    const context = this.videoTrackContexts.get(trackName)
    if (context) {
      if (!this.isSameVideoEncoding(context.config, settings)) {
        context.config = settings
        context.worker.postMessage({ type: 'encoderConfig', config: settings })
      }
      context.worker.postMessage({ type: 'keyframeInterval', keyframeInterval })
      if (!context.encodingStarted) {
        this.startVideoTrackEncoding(context)
        return
      }
      this.restartVideoTrackContext(trackName)
      return
    }

    this.syncVideoTrackContexts(source)
    const created = this.videoTrackContexts.get(trackName)
    if (!created) {
      return
    }
    if (!created.encodingStarted) {
      this.startVideoTrackEncoding(created)
      return
    }
    this.restartVideoTrackContext(trackName)
  }

  async applyAudioEncodingForTrack(trackName: string): Promise<void> {
    const track = this.catalogTracks.find((entry) => entry.name === trackName && entry.role === 'audio')
    if (!track) {
      return
    }
    const settings = this.buildAudioEncodingFromTrack(track, this.audioEncodingSettings)
    const streamUpdateSettings = this.resolveAudioStreamUpdateSettingsForTrack(track)
    const context = this.audioTrackContexts.get(trackName)
    if (context) {
      if (!this.isSameAudioEncoding(context.config, settings)) {
        context.config = settings
        context.worker.postMessage({ type: 'config', config: this.buildAudioEncoderConfig(settings) })
      }
      this.applyAudioStreamUpdateSettingsToContext(context, streamUpdateSettings)
      if (!context.encodingStarted) {
        this.startAudioTrackEncoding(context)
        return
      }
      this.restartAudioTrackContext(trackName)
      return
    }
    this.syncAudioTrackContexts()
    const created = this.audioTrackContexts.get(trackName)
    if (!created) {
      return
    }
    if (!created.encodingStarted) {
      this.startAudioTrackEncoding(created)
      return
    }
    this.restartAudioTrackContext(trackName)
  }

  private forceKeyframeAndRestartCamera(deviceId?: string): Promise<void> {
    const restart = async () => {
      try {
        await this.stopCamera()
      } catch (error) {
        console.warn('Failed to stop camera before restart', error)
      }
      await this.startCamera(deviceId)
    }
    return restart()
  }

  private syncVideoTrackContexts(source: VideoSource): void {
    const desiredTracks = this.getVideoTracksBySource(source)
    const desiredNames = new Set(desiredTracks.map((track) => track.name))

    for (const [trackName, context] of this.videoTrackContexts.entries()) {
      if (context.source === source && !desiredNames.has(trackName)) {
        this.stopVideoTrackContext(trackName)
      }
    }

    const sourceTrack = this.getSourceTrack(source)
    if (!sourceTrack) {
      return
    }

    const fallback = source === 'screenshare' ? this.screenShareEncodingSettings : this.videoEncodingSettings

    for (const track of desiredTracks) {
      const config = this.buildVideoEncodingFromTrack(track, fallback)
      const keyframeInterval = this.normalizeTrackKeyframeInterval(track.keyframeInterval)
      const existing = this.videoTrackContexts.get(track.name)
      if (existing) {
        if (!this.isSameVideoEncoding(existing.config, config)) {
          existing.config = config
          existing.worker.postMessage({ type: 'encoderConfig', config })
        }
        existing.worker.postMessage({ type: 'keyframeInterval', keyframeInterval })
        if (!existing.encodingStarted && this.hasAnySubscriberAliasForTrack(track.name)) {
          this.startVideoTrackEncoding(existing)
        }
        continue
      }
      this.createVideoTrackContext(source, track.name, sourceTrack, config, keyframeInterval)
    }
  }

  private syncAudioTrackContexts(): void {
    const desiredTracks = this.catalogTracks.filter((track) => track.role === 'audio')
    const desiredNames = new Set(desiredTracks.map((track) => track.name))

    for (const trackName of this.audioTrackContexts.keys()) {
      if (!desiredNames.has(trackName)) {
        this.stopAudioTrackContext(trackName)
      }
    }

    const sourceTrack = this.audioStream?.getAudioTracks()[0]
    if (!sourceTrack) {
      return
    }

    for (const track of desiredTracks) {
      const config = this.buildAudioEncodingFromTrack(track, this.audioEncodingSettings)
      const streamUpdateSettings = this.resolveAudioStreamUpdateSettingsForTrack(track)
      const existing = this.audioTrackContexts.get(track.name)
      if (existing) {
        if (!this.isSameAudioEncoding(existing.config, config)) {
          existing.config = config
          existing.worker.postMessage({ type: 'config', config: this.buildAudioEncoderConfig(config) })
        }
        this.applyAudioStreamUpdateSettingsToContext(existing, streamUpdateSettings)
        if (!existing.encodingStarted && this.hasAnySubscriberAliasForTrack(track.name)) {
          this.startAudioTrackEncoding(existing)
        }
        continue
      }
      this.createAudioTrackContext(track.name, sourceTrack, config, streamUpdateSettings)
    }
  }

  private createVideoTrackContext(
    source: VideoSource,
    trackName: string,
    sourceTrack: MediaStreamTrack,
    config: VideoEncodingSettings,
    keyframeInterval: number
  ): void {
    const track = sourceTrack.clone()
    const processor = new MediaStreamTrackProcessor({ track })
    const readable = processor.readable

    const context: VideoTrackEncoderContext = {
      trackName,
      source,
      track,
      readable,
      config,
      worker: new Worker(new URL('../../../../utils/media/encoders/videoEncoder.ts', import.meta.url), {
        type: 'module'
      }),
      encodingStarted: false,
      transportState: new MediaTransportState(),
      sendQueue: Promise.resolve(),
      pendingSendJobs: 0,
      activeAliases: new Set<string>(),
      videoGroupStates: new Map<bigint, GroupState>()
    }

    context.worker.onmessage = async (event: MessageEvent) => {
      const data = event.data as
        | {
            type: 'chunk'
            chunk: EncodedVideoChunk
            metadata: EncodedVideoChunkMetadata | undefined
            captureTimestampMicros?: number
            encodeDoneTimestampMicros?: number
            encodeQueueSize?: number
          }
        | { type: 'bitrate'; kbps: number }
        | { type: 'configError'; reason: string; config: any }
        | {
            chunk: EncodedVideoChunk
            metadata: EncodedVideoChunkMetadata | undefined
            captureTimestampMicros?: number
            encodeDoneTimestampMicros?: number
            encodeQueueSize?: number
          }

      if ('type' in data && data.type === 'bitrate') {
        this.videoBitrateByTrackName.set(trackName, data.kbps)
        this.reportVideoBitrate(source)
        return
      }
      if ('type' in data && data.type === 'configError') {
        const cfg = data.config as { codec?: string; width?: number; height?: number; bitrate?: number }
        const codecText = cfg?.codec ? `codec ${cfg.codec}` : 'selected codec'
        const resText =
          cfg?.width && cfg?.height ? `resolution ${cfg.width}x${cfg.height}` : 'selected resolution/bitrate'
        this.handlers.onVideoEncodeError?.(
          `[${trackName}] Encoder configuration unsupported (${codecText}, ${resText}). Please lower resolution/bitrate or choose another codec.`
        )
        return
      }

      const chunkData = 'type' in data ? data.chunk : data.chunk
      const metadata = 'type' in data ? data.metadata : data.metadata
      const captureTimestampMicros = 'type' in data ? data.captureTimestampMicros : data.captureTimestampMicros
      const encodeDoneTimestampMicros = 'type' in data ? data.encodeDoneTimestampMicros : data.encodeDoneTimestampMicros
      const encodeQueueSize = 'type' in data ? data.encodeQueueSize : data.encodeQueueSize
      this.handleVideoTrackChunk(
        context,
        chunkData,
        metadata,
        captureTimestampMicros,
        encodeDoneTimestampMicros,
        encodeQueueSize
      )
    }

    context.worker.postMessage({ type: 'encoderConfig', config })
    context.worker.postMessage({ type: 'keyframeInterval', keyframeInterval })

    this.videoTrackContexts.set(trackName, context)
    if (this.hasAnySubscriberAliasForTrack(trackName)) {
      this.startVideoTrackEncoding(context)
    }
  }

  private createAudioTrackContext(
    trackName: string,
    sourceTrack: MediaStreamTrack,
    config: AudioEncodingSettings,
    streamUpdateSettings: { mode: 'single' | 'interval'; intervalSeconds: number }
  ): void {
    const track = sourceTrack.clone()
    const processor = new MediaStreamTrackProcessor({ track })
    const readable = processor.readable

    const context: AudioTrackEncoderContext = {
      trackName,
      track,
      readable,
      config,
      worker: new Worker(new URL('../../../../utils/media/encoders/audioEncoder.ts', import.meta.url), {
        type: 'module'
      }),
      encodingStarted: false,
      streamUpdateMode: streamUpdateSettings.mode,
      streamUpdateIntervalSeconds: streamUpdateSettings.intervalSeconds,
      streamUpdateTimer: null,
      streamUpdateInFlight: false,
      transportState: new MediaTransportState(),
      sendQueue: Promise.resolve(),
      activeAliases: new Set<string>()
    }

    context.worker.onmessage = async (event: MessageEvent) => {
      const data = event.data as
        | {
            type: 'chunk'
            chunk: EncodedAudioChunk
            metadata: EncodedAudioChunkMetadata | undefined
            captureTimestampMicros?: number
          }
        | { type: 'bitrate'; media: 'audio'; kbps: number }
        | { type: 'configError'; media: 'audio'; reason: string; config: any }
        | { chunk: EncodedAudioChunk; metadata: EncodedAudioChunkMetadata | undefined; captureTimestampMicros?: number }

      if ('type' in data && data.type === 'bitrate') {
        this.audioBitrateByTrackName.set(trackName, data.kbps)
        this.reportAudioBitrate()
        return
      }

      if ('type' in data && data.type === 'configError') {
        const cfg = data.config as { codec?: string; bitrate?: number }
        const codecText = cfg?.codec ?? 'selected codec'
        const suggestion =
          cfg?.codec && cfg.codec.startsWith('mp4a')
            ? 'Please choose a higher bitrate for AAC.'
            : 'Please choose another codec or adjust bitrate.'
        this.handlers.onAudioEncodeError?.(
          `[${trackName}] Audio encoder configuration unsupported (${codecText}). ${suggestion}`
        )
        return
      }

      const chunkData = 'type' in data ? data.chunk : data.chunk
      const metadata = 'type' in data ? data.metadata : data.metadata
      const captureTimestampMicros = 'type' in data ? data.captureTimestampMicros : data.captureTimestampMicros
      this.handleAudioTrackChunk(context, chunkData, metadata, captureTimestampMicros)
    }

    context.worker.postMessage({ type: 'config', config: this.buildAudioEncoderConfig(config) })
    this.audioTrackContexts.set(trackName, context)
    if (this.hasAnySubscriberAliasForTrack(trackName)) {
      this.startAudioTrackEncoding(context)
    }
  }

  private stopVideoTrackContext(trackName: string): void {
    const context = this.videoTrackContexts.get(trackName)
    if (!context) {
      return
    }
    if (context.readable) {
      void context.readable.cancel().catch(() => {})
      context.readable = null
    }
    context.worker.terminate()
    context.track.stop()
    this.videoTrackContexts.delete(trackName)
    this.videoBitrateByTrackName.delete(trackName)
    this.reportVideoBitrate(context.source)
  }

  private stopAudioTrackContext(trackName: string): void {
    const context = this.audioTrackContexts.get(trackName)
    if (!context) {
      return
    }
    this.stopAudioStreamUpdateTimerForContext(context)
    if (context.readable) {
      void context.readable.cancel().catch(() => {})
      context.readable = null
    }
    context.worker.terminate()
    context.track.stop()
    this.audioTrackContexts.delete(trackName)
    this.audioBitrateByTrackName.delete(trackName)
    this.reportAudioBitrate()
  }

  private stopVideoTrackContextsBySource(source: VideoSource): void {
    const names = Array.from(this.videoTrackContexts.entries())
      .filter(([, context]) => context.source === source)
      .map(([trackName]) => trackName)
    for (const trackName of names) {
      this.stopVideoTrackContext(trackName)
    }
  }

  private stopAllAudioTrackContexts(): void {
    for (const trackName of Array.from(this.audioTrackContexts.keys())) {
      this.stopAudioTrackContext(trackName)
    }
  }

  private restartVideoTrackContextsBySource(source: VideoSource): void {
    const names = Array.from(this.videoTrackContexts.entries())
      .filter(([, context]) => context.source === source)
      .map(([trackName]) => trackName)
    for (const trackName of names) {
      this.restartVideoTrackContext(trackName)
    }
  }

  private restartAllAudioTrackContexts(): void {
    for (const trackName of Array.from(this.audioTrackContexts.keys())) {
      this.restartAudioTrackContext(trackName)
    }
  }

  private restartVideoTrackContext(trackName: string): void {
    const context = this.videoTrackContexts.get(trackName)
    if (!context) {
      return
    }
    const sourceTrack = this.getSourceTrack(context.source)
    if (!sourceTrack) {
      return
    }
    const { source, config, encodingStarted } = context
    const keyframeInterval = this.getKeyframeIntervalForTrackName(trackName)
    this.stopVideoTrackContext(trackName)
    this.createVideoTrackContext(source, trackName, sourceTrack, config, keyframeInterval)
    if (encodingStarted) {
      const next = this.videoTrackContexts.get(trackName)
      if (next) {
        this.startVideoTrackEncoding(next)
      }
    }
  }

  private restartAudioTrackContext(trackName: string): void {
    const context = this.audioTrackContexts.get(trackName)
    if (!context) {
      return
    }
    const sourceTrack = this.audioStream?.getAudioTracks()[0]
    if (!sourceTrack) {
      return
    }
    const { config, streamUpdateMode, streamUpdateIntervalSeconds, encodingStarted } = context
    this.stopAudioTrackContext(trackName)
    this.createAudioTrackContext(trackName, sourceTrack, config, {
      mode: streamUpdateMode,
      intervalSeconds: streamUpdateIntervalSeconds
    })
    if (encodingStarted) {
      const next = this.audioTrackContexts.get(trackName)
      if (next) {
        this.startAudioTrackEncoding(next)
      }
    }
  }

  private handleVideoTrackChunk(
    context: VideoTrackEncoderContext,
    chunk: EncodedVideoChunk,
    metadata: EncodedVideoChunkMetadata | undefined,
    captureTimestampMicros?: number,
    encodeDoneTimestampMicros?: number,
    encodeQueueSize?: number
  ): void {
    const aliases = this.collectAliasesForTrack(context.trackName, context.activeAliases, (alias) => {
      context.transportState.resetAlias(alias)
      context.videoGroupStates.delete(BigInt(alias))
    })
    if (!aliases.length) {
      return
    }

    const enqueueStartedAtMs = performance.now()
    const captureToEncodeDoneMs =
      typeof captureTimestampMicros === 'number' &&
      Number.isFinite(captureTimestampMicros) &&
      typeof encodeDoneTimestampMicros === 'number' &&
      Number.isFinite(encodeDoneTimestampMicros)
        ? Math.max(0, (encodeDoneTimestampMicros - captureTimestampMicros) / 1000)
        : null
    context.pendingSendJobs += 1
    const queueDepthAtEnqueue = context.pendingSendJobs
    context.sendQueue = context.sendQueue
      .then(async () => {
        const sendStartedAtMs = performance.now()
        const client = this.client.getRawClient()
        if (!client) {
          return
        }
        const timingAcc: VideoSendTimingAccumulator = {
          objectSendMs: 0,
          serializeMs: 0,
          endOfGroupMs: 0,
          objectBytes: 0,
          objectCount: 0
        }
        const sender: VideoChunkSender = async (
          trackAlias,
          groupId,
          subgroupId,
          objectId,
          videoChunk,
          rawClient,
          loc
        ) => {
          await this.sendVideoObjectForTrackContext(
            context,
            trackAlias,
            groupId,
            subgroupId,
            objectId,
            videoChunk,
            metadata,
            rawClient,
            loc,
            timingAcc
          )
        }
        await sendVideoChunkViaMoqt({
          chunk,
          metadata,
          captureTimestampMicros,
          trackAliases: aliases,
          publisherPriority: 0,
          client,
          transportState: context.transportState,
          sender
        })
        const sendFinishedAtMs = performance.now()
        this.handlers.onLocalVideoSendTiming?.(
          {
            captureToEncodeDoneMs,
            encodeQueueSize:
              typeof encodeQueueSize === 'number' && Number.isFinite(encodeQueueSize)
                ? Math.max(0, encodeQueueSize)
                : 0,
            queueWaitMs: Math.max(0, sendStartedAtMs - enqueueStartedAtMs),
            sendActiveMs: Math.max(0, sendFinishedAtMs - sendStartedAtMs),
            objectSendMs: timingAcc.objectSendMs,
            serializeMs: timingAcc.serializeMs,
            endOfGroupMs: timingAcc.endOfGroupMs,
            queueDepth: Math.max(0, queueDepthAtEnqueue),
            objectBytes: timingAcc.objectBytes,
            objectCount: timingAcc.objectCount,
            aliasCount: aliases.length,
            keyframe: chunk.type === 'key'
          },
          context.source
        )
      })
      .catch((error) => {
        console.error(`${context.trackName} video send failed:`, error)
      })
      .finally(() => {
        context.pendingSendJobs = Math.max(0, context.pendingSendJobs - 1)
      })
  }

  private handleAudioTrackChunk(
    context: AudioTrackEncoderContext,
    chunk: EncodedAudioChunk,
    metadata: EncodedAudioChunkMetadata | undefined,
    captureTimestampMicros?: number
  ): void {
    const aliases = this.collectAliasesForTrack(context.trackName, context.activeAliases, (alias) => {
      context.transportState.resetAlias(alias)
    })
    if (!aliases.length) {
      if (context.encodingStarted) {
        queueMicrotask(() => this.suspendAudioTrackEncodingIfNoSubscribers(context.trackName, context))
      }
      return
    }

    context.sendQueue = context.sendQueue
      .then(async () => {
        const client = this.client.getRawClient()
        if (!client) {
          return
        }
        await sendAudioChunkViaMoqt({
          chunk,
          metadata,
          captureTimestampMicros,
          trackAliases: aliases,
          client,
          transportState: context.transportState
        })
      })
      .catch((error) => {
        console.error(`${context.trackName} audio send failed:`, error)
      })
  }

  private async sendVideoObjectForTrackContext(
    context: VideoTrackEncoderContext,
    trackAlias: bigint,
    groupId: bigint,
    subgroupId: bigint,
    objectId: bigint,
    chunk: EncodedVideoChunk,
    metadata: EncodedVideoChunkMetadata | undefined,
    client: MOQTClient,
    locHeader?: LocHeader,
    timingAcc?: VideoSendTimingAccumulator
  ): Promise<void> {
    const previousState = context.videoGroupStates.get(trackAlias)
    if (previousState && previousState.groupId !== groupId) {
      const endObjectId = previousState.lastObjectId + 1n
      const eogStartedAtMs = performance.now()
      await this.sendEndOfGroup(client, trackAlias, previousState.groupId, endObjectId)
      if (timingAcc) {
        timingAcc.endOfGroupMs += Math.max(0, performance.now() - eogStartedAtMs)
      }
    }

    const decoderConfig = metadata?.decoderConfig as { codec?: string; avc?: { format?: 'annexb' | 'avc' } } | undefined
    const avcFormat = context.config.codec.startsWith('avc')
      ? ((decoderConfig?.avc?.format as 'annexb' | 'avc' | undefined) ?? 'annexb')
      : undefined
    const codec = decoderConfig?.codec ?? context.config.codec
    const serializeStartedAtMs = performance.now()
    const payload = serializeChunk(
      {
        type: chunk.type,
        timestamp: chunk.timestamp,
        duration: chunk.duration ?? null,
        byteLength: chunk.byteLength,
        copyTo: (dest) => chunk.copyTo(dest)
      },
      {
        codec,
        avcFormat
      }
    )
    if (timingAcc) {
      timingAcc.serializeMs += Math.max(0, performance.now() - serializeStartedAtMs)
      timingAcc.objectBytes += payload.byteLength
      timingAcc.objectCount += 1
    }

    const objectSendStartedAtMs = performance.now()
    await client.sendSubgroupObject(trackAlias, groupId, subgroupId, objectId, undefined, payload, locHeader)
    if (timingAcc) {
      timingAcc.objectSendMs += Math.max(0, performance.now() - objectSendStartedAtMs)
    }
    context.videoGroupStates.set(trackAlias, { groupId, lastObjectId: objectId })
  }

  private collectAliasesForTrack(
    trackName: string,
    activeAliases: Set<string>,
    onAliasRemoved: (alias: string) => void
  ): bigint[] {
    const client = this.client.getRawClient()
    if (!client) {
      return []
    }
    const aliasList = Array.from(client.getTrackSubscribers(this.trackNamespace, trackName), (value) => BigInt(value))
    const latestKeys = new Set(aliasList.map((alias) => alias.toString()))

    for (const key of activeAliases) {
      if (!latestKeys.has(key)) {
        onAliasRemoved(key)
      }
    }

    activeAliases.clear()
    for (const alias of aliasList) {
      activeAliases.add(alias.toString())
    }

    return aliasList
  }

  private hasAnySubscriberAliasForTrack(trackName: string): boolean {
    return this.collectAliasesForTrack(trackName, new Set<string>(), () => {}).length > 0
  }

  private startVideoTrackEncoding(context: VideoTrackEncoderContext): void {
    if (context.encodingStarted || !context.readable) {
      return
    }
    const readable = context.readable
    context.readable = null
    context.encodingStarted = true
    context.worker.postMessage({ type: 'videoStream', videoStream: readable }, [readable])
  }

  private startAudioTrackEncoding(context: AudioTrackEncoderContext): void {
    if (context.encodingStarted || !context.readable) {
      return
    }
    const readable = context.readable
    context.readable = null
    context.encodingStarted = true
    context.worker.postMessage({ audioStream: readable }, [readable])
    this.configureAudioStreamUpdateTimerForContext(context)
  }

  private suspendVideoTrackEncodingIfNoSubscribers(trackName: string, expectedContext: VideoTrackEncoderContext): void {
    const current = this.videoTrackContexts.get(trackName)
    if (!current || current !== expectedContext || !current.encodingStarted) {
      return
    }
    if (this.hasAnySubscriberAliasForTrack(trackName)) {
      return
    }
    const sourceTrack = this.getSourceTrack(current.source)
    if (!sourceTrack) {
      return
    }
    const { source, config } = current
    const keyframeInterval = this.getKeyframeIntervalForTrackName(trackName)
    this.handlers.onLocalVideoSendTiming?.(null, source)
    this.stopVideoTrackContext(trackName)
    this.createVideoTrackContext(source, trackName, sourceTrack, config, keyframeInterval)
  }

  private suspendAudioTrackEncodingIfNoSubscribers(trackName: string, expectedContext: AudioTrackEncoderContext): void {
    const current = this.audioTrackContexts.get(trackName)
    if (!current || current !== expectedContext || !current.encodingStarted) {
      return
    }
    if (this.hasAnySubscriberAliasForTrack(trackName)) {
      return
    }
    const sourceTrack = this.audioStream?.getAudioTracks()[0]
    if (!sourceTrack) {
      return
    }
    const { config, streamUpdateMode, streamUpdateIntervalSeconds } = current
    this.stopAudioTrackContext(trackName)
    this.createAudioTrackContext(trackName, sourceTrack, config, {
      mode: streamUpdateMode,
      intervalSeconds: streamUpdateIntervalSeconds
    })
  }

  private getVideoTracksBySource(source: VideoSource): CallCatalogTrack[] {
    const wantsScreenShare = source === 'screenshare'
    return this.catalogTracks
      .filter((track) => track.role === 'video')
      .filter((track) => isScreenShareTrackName(track.name) === wantsScreenShare)
  }

  private getSourceTrack(source: VideoSource): MediaStreamTrack | null {
    const stream = source === 'camera' ? this.cameraStream : this.screenShareStream
    return stream?.getVideoTracks()[0] ?? null
  }

  private buildVideoEncodingFromTrack(track: CallCatalogTrack, fallback: VideoEncodingSettings): VideoEncodingSettings {
    return {
      codec: this.normalizeNonEmptyString(track.codec) ?? fallback.codec,
      width: this.normalizePositiveNumber(track.width) ?? fallback.width,
      height: this.normalizePositiveNumber(track.height) ?? fallback.height,
      bitrate: this.normalizePositiveNumber(track.bitrate) ?? fallback.bitrate,
      framerate: this.normalizePositiveNumber(track.framerate) ?? fallback.framerate,
      hardwareAcceleration:
        this.normalizeHardwareAcceleration(track.hardwareAcceleration) ?? fallback.hardwareAcceleration
    }
  }

  private buildAudioEncodingFromTrack(track: CallCatalogTrack, fallback: AudioEncodingSettings): AudioEncodingSettings {
    return {
      codec: this.normalizeNonEmptyString(track.codec) ?? fallback.codec,
      bitrate: this.normalizePositiveNumber(track.bitrate) ?? fallback.bitrate,
      channels: this.parseAudioChannels(track.channelConfig) ?? fallback.channels
    }
  }

  private buildAudioEncoderConfig(settings: AudioEncodingSettings): AudioEncoderConfig {
    return {
      codec: settings.codec,
      sampleRate: 48_000,
      numberOfChannels: settings.channels,
      bitrate: settings.bitrate
    }
  }

  private isSameVideoEncoding(left: VideoEncodingSettings, right: VideoEncodingSettings): boolean {
    return (
      left.codec === right.codec &&
      left.width === right.width &&
      left.height === right.height &&
      left.bitrate === right.bitrate &&
      left.framerate === right.framerate &&
      left.hardwareAcceleration === right.hardwareAcceleration
    )
  }

  private isSameAudioEncoding(left: AudioEncodingSettings, right: AudioEncodingSettings): boolean {
    return left.codec === right.codec && left.bitrate === right.bitrate && left.channels === right.channels
  }

  private parseAudioChannels(channelConfig: string | undefined): number | undefined {
    const normalized = this.normalizeNonEmptyString(channelConfig)?.toLowerCase()
    if (!normalized) {
      return undefined
    }
    if (normalized.includes('mono')) {
      return 1
    }
    if (normalized.includes('stereo')) {
      return 2
    }
    const matched = normalized.match(/(\d+)/)
    if (!matched) {
      return undefined
    }
    const parsed = Number(matched[1])
    return Number.isFinite(parsed) && parsed > 0 ? Math.floor(parsed) : undefined
  }

  private normalizePositiveNumber(value: number | undefined): number | undefined {
    if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) {
      return undefined
    }
    return Math.floor(value)
  }

  private normalizeNonEmptyString(value: string | undefined): string | undefined {
    if (typeof value !== 'string') {
      return undefined
    }
    const trimmed = value.trim()
    return trimmed.length > 0 ? trimmed : undefined
  }

  private normalizeHardwareAcceleration(value: HardwareAcceleration | undefined): HardwareAcceleration | undefined {
    return value === 'prefer-hardware' || value === 'prefer-software' || value === 'no-preference' ? value : undefined
  }

  private reportVideoBitrate(source: VideoSource): void {
    const values = Array.from(this.videoTrackContexts.values())
      .filter((context) => context.source === source)
      .map((context) => this.videoBitrateByTrackName.get(context.trackName) ?? 0)

    if (values.length === 0) {
      this.handlers.onEncodedVideoBitrate?.(0)
      return
    }

    const maxKbps = values.reduce((max, value) => Math.max(max, value), 0)
    this.handlers.onEncodedVideoBitrate?.(maxKbps)
  }

  private reportAudioBitrate(): void {
    const values = Array.from(this.audioTrackContexts.values()).map(
      (context) => this.audioBitrateByTrackName.get(context.trackName) ?? 0
    )
    if (values.length === 0) {
      this.handlers.onEncodedAudioBitrate?.(0)
      return
    }
    const maxKbps = values.reduce((max, value) => Math.max(max, value), 0)
    this.handlers.onEncodedAudioBitrate?.(maxKbps)
  }

  private getKeyframeIntervalForTrackName(trackName: string): number {
    const track = this.catalogTracks.find((entry) => entry.name === trackName && entry.role === 'video')
    return this.normalizeTrackKeyframeInterval(track?.keyframeInterval)
  }

  private normalizeTrackKeyframeInterval(value: number | undefined): number {
    return this.normalizePositiveNumber(value) ?? DEFAULT_VIDEO_KEYFRAME_INTERVAL
  }

  private resolveAudioStreamUpdateSettingsForTrack(track: CallCatalogTrack): {
    mode: 'single' | 'interval'
    intervalSeconds: number
  } {
    const mode = track.audioStreamUpdateMode === 'single' ? 'single' : DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS.mode
    const intervalSeconds =
      this.normalizePositiveNumber(track.audioStreamUpdateIntervalSeconds) ??
      DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS.intervalSeconds
    return { mode, intervalSeconds }
  }

  private applyAudioStreamUpdateSettingsToContext(
    context: AudioTrackEncoderContext,
    settings: { mode: 'single' | 'interval'; intervalSeconds: number }
  ): void {
    if (
      context.streamUpdateMode === settings.mode &&
      context.streamUpdateIntervalSeconds === settings.intervalSeconds
    ) {
      return
    }
    context.streamUpdateMode = settings.mode
    context.streamUpdateIntervalSeconds = settings.intervalSeconds
    this.configureAudioStreamUpdateTimerForContext(context)
  }

  private configureAudioStreamUpdateTimerForContext(context: AudioTrackEncoderContext): void {
    this.stopAudioStreamUpdateTimerForContext(context)
    if (!context.encodingStarted) {
      return
    }
    if (context.streamUpdateMode !== 'interval') {
      return
    }
    if (!this.audioStream) {
      return
    }
    const intervalMs = context.streamUpdateIntervalSeconds * 1000
    context.streamUpdateTimer = setInterval(() => {
      void this.rotateAudioStreamGroupForTrackContext(context)
    }, intervalMs)
  }

  private stopAudioStreamUpdateTimerForContext(context: AudioTrackEncoderContext): void {
    if (context.streamUpdateTimer) {
      clearInterval(context.streamUpdateTimer)
      context.streamUpdateTimer = null
    }
  }

  private async rotateAudioStreamGroupForTrackContext(context: AudioTrackEncoderContext): Promise<void> {
    if (context.streamUpdateInFlight) {
      return
    }
    context.streamUpdateInFlight = true
    try {
      context.sendQueue = context.sendQueue
        .then(async () => {
          await this.sendAudioEndOfGroupForTrackContext(context)
          // Advancing the group resets subgroup headers and starts a fresh audio subgroup stream.
          context.transportState.advanceAudioGroup()
        })
        .catch((error) => {
          console.error(`${context.trackName} audio group rotation failed:`, error)
        })
      await context.sendQueue
    } finally {
      context.streamUpdateInFlight = false
    }
  }

  private async sendAudioEndOfGroupForTrackContext(context: AudioTrackEncoderContext): Promise<void> {
    const aliases = this.collectAliasesForTrack(context.trackName, context.activeAliases, (alias) => {
      context.transportState.resetAlias(alias)
    })
    if (!aliases.length) {
      return
    }
    const client = this.client.getRawClient()
    if (!client) {
      return
    }
    const currentGroupId = context.transportState.getAudioGroupId()
    const endObjectId = context.transportState.getAudioObjectId()
    if (endObjectId === 0n) {
      return
    }
    for (const alias of aliases) {
      await client.sendSubgroupObject(alias, currentGroupId, 0n, endObjectId, 3, new Uint8Array(0), undefined)
    }
  }

  private normalizeCatalogTracks(tracks: CallCatalogTrack[]): CallCatalogTrack[] {
    const normalized: CallCatalogTrack[] = []
    for (const track of tracks) {
      const name = track.name.trim()
      if (!name) {
        continue
      }
      const keyframeInterval =
        track.role === 'video' ? this.normalizeTrackKeyframeInterval(track.keyframeInterval) : undefined
      const audioStreamSettings =
        track.role === 'audio' ? this.resolveAudioStreamUpdateSettingsForTrack(track) : undefined
      normalized.push({
        ...track,
        name,
        label: track.label.trim() || name,
        keyframeInterval,
        audioStreamUpdateMode: audioStreamSettings?.mode,
        audioStreamUpdateIntervalSeconds: audioStreamSettings?.intervalSeconds,
        isLive: track.isLive ?? true
      })
    }
    return normalized
  }

  private async broadcastCatalog(): Promise<void> {
    const client = this.client.getRawClient()
    if (!client) {
      return
    }

    const aliases = Array.from(client.getTrackSubscribers(this.trackNamespace, CATALOG_TRACK_NAME), (value) =>
      BigInt(value)
    )
    if (!aliases.length) {
      return
    }
    for (const alias of aliases) {
      await this.sendCatalogObject(client, alias)
    }
  }

  private async sendCatalogObject(client: MOQTClient, trackAlias: bigint): Promise<void> {
    const aliasKey = trackAlias.toString()
    const previousGroup = this.catalogGroupByAlias.get(aliasKey) ?? -1n
    const groupId = previousGroup + 1n
    this.catalogGroupByAlias.set(aliasKey, groupId)

    const payload = new TextEncoder().encode(buildCallCatalogJson(this.trackNamespace, this.catalogTracks))
    await client.sendSubgroupHeader(trackAlias, groupId, 0n, 0)
    await client.sendSubgroupObject(trackAlias, groupId, 0n, 0n, undefined, payload, undefined)
  }

  private async sendEndOfGroup(client: MOQTClient, trackAlias: bigint, groupId: bigint, endObjectId: bigint) {
    await client.sendSubgroupObject(trackAlias, groupId, 0n, endObjectId, 3, new Uint8Array(0), undefined)
  }
}
