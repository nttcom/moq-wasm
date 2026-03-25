import { MoqtClientWrapper } from '@moqt/moqtClient'
import { MediaPublisher, type SubscribedCatalogTrack } from './mediaPublisher'
import { MediaSubscriber } from './mediaSubscriber'
import type { VideoJitterConfig, AudioJitterConfig } from '../types/jitterBuffer'
import type { VideoEncodingSettings } from '../types/videoEncoding'
import type { AudioEncodingSettings } from '../types/audioEncoding'
import type { AudioCaptureConstraints, CameraCaptureConstraints } from '../types/captureConstraints'
import type { SubscribeMessage } from '../../../../pkg/moqt_client_wasm'
import type { CallCatalogTrack, CatalogSubscribeRole, CatalogTrackRole } from '../types/catalog'
import type { JitterBufferEvent } from '../types/media'
import { isScreenShareTrackName } from '../utils/catalogTrackName'

export interface MediaHandlers {
  onLocalVideoStream?: (stream: MediaStream | null, source: 'camera' | 'screenshare') => void
  onLocalAudioStream?: (stream: MediaStream | null) => void
  onLocalVideoBitrate?: (mbps: number) => void
  onLocalAudioBitrate?: (mbps: number) => void
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
    source: 'camera' | 'screenshare'
  ) => void
  onRemoteVideoStream?: (userId: string, stream: MediaStream, source: 'camera' | 'screenshare') => void
  onRemoteAudioStream?: (userId: string, stream: MediaStream) => void
  onRemoteAudioStreamClosed?: (userId: string) => void
  onRemoteVideoBitrate?: (userId: string, mbps: number, source: 'camera' | 'screenshare') => void
  onRemoteVideoKeyframeInterval?: (userId: string, frames: number, source: 'camera' | 'screenshare') => void
  onRemoteAudioBitrate?: (userId: string, mbps: number) => void
  onRemoteVideoReceiveLatency?: (userId: string, ms: number, source: 'camera' | 'screenshare') => void
  onRemoteVideoRenderingLatency?: (userId: string, ms: number, source: 'camera' | 'screenshare') => void
  onRemoteVideoTiming?: (
    userId: string,
    timing: {
      receiveToDecodeMs: number | null
      receiveToRenderMs: number | null
    },
    source: 'camera' | 'screenshare'
  ) => void
  onRemoteVideoDecodingObject?: (
    userId: string,
    decoding: {
      phase: 'submit' | 'output' | 'error'
      groupId: string
      objectId: string
      chunkType: string
      codec?: string
    },
    source: 'camera' | 'screenshare'
  ) => void
  onRemoteVideoPacing?: (
    userId: string,
    pacing: {
      intervalMs: number
      effectiveIntervalMs: number
      bufferedFrames: number
      decodeQueueSize: number
      targetFrames: number
      lastReason?: string
      action?: string
      detailMs?: number
    },
    source: 'camera' | 'screenshare'
  ) => void
  onRemoteAudioReceiveLatency?: (userId: string, ms: number) => void
  onRemoteAudioRenderingLatency?: (userId: string, ms: number) => void
  onRemoteAudioPlaybackQueue?: (userId: string, queuedMs: number) => void
  onRemoteVideoJitterBufferActivity?: (
    userId: string,
    activity: { event: JitterBufferEvent; bufferedFrames: number; capacityFrames: number },
    source: 'camera' | 'screenshare'
  ) => void
  onRemoteAudioJitterBufferActivity?: (
    userId: string,
    activity: { event: JitterBufferEvent; bufferedFrames: number; capacityFrames: number }
  ) => void
  onRemoteVideoConfig?: (
    userId: string,
    config: {
      codec: string
      width?: number
      height?: number
      descriptionLength?: number
      avcFormat?: 'annexb' | 'avc'
      hardwareAcceleration?: HardwareAcceleration
      optimizeForLatency?: boolean
    },
    source: 'camera' | 'screenshare'
  ) => void
  onVideoEncodeError?: (message: string) => void
  onAudioEncodeError?: (message: string) => void
  onAudioEncodingAdjusted?: (settings: AudioEncodingSettings) => void
  onScreenShareEncodingApplied?: (settings: VideoEncodingSettings) => void
}

export class CallMediaController {
  private handlers: MediaHandlers = {}
  private readonly publisher: MediaPublisher
  private readonly subscriber: MediaSubscriber
  private readonly trackNamespace: string[]

  constructor(client: MoqtClientWrapper, trackNamespace: string[]) {
    this.publisher = new MediaPublisher(client, trackNamespace)
    this.subscriber = new MediaSubscriber(client)
    this.trackNamespace = trackNamespace

    this.publisher.setHandlers({
      onLocalCameraStream: (stream) => this.handlers.onLocalVideoStream?.(stream, 'camera'),
      onLocalScreenShareStream: (stream) => this.handlers.onLocalVideoStream?.(stream, 'screenshare'),
      onLocalAudioStream: (stream) => this.handlers.onLocalAudioStream?.(stream),
      onEncodedVideoBitrate: (mbps) => this.handlers.onLocalVideoBitrate?.(mbps),
      onEncodedAudioBitrate: (mbps) => this.handlers.onLocalAudioBitrate?.(mbps),
      onLocalVideoSendTiming: (timing, source) => this.handlers.onLocalVideoSendTiming?.(timing, source),
      onVideoEncodeError: (message) => this.handlers.onVideoEncodeError?.(message),
      onAudioEncodeError: (message) => this.handlers.onAudioEncodeError?.(message),
      onAudioEncodingAdjusted: (settings) => this.handlers.onAudioEncodingAdjusted?.(settings),
      onScreenShareEncodingApplied: (settings) => this.handlers.onScreenShareEncodingApplied?.(settings)
    })

    this.subscriber.setHandlers({
      onRemoteVideoStream: (userId, stream, source) => this.handlers.onRemoteVideoStream?.(userId, stream, source),
      onRemoteAudioStream: (userId, stream) => this.handlers.onRemoteAudioStream?.(userId, stream),
      onRemoteAudioStreamClosed: (userId) => this.handlers.onRemoteAudioStreamClosed?.(userId),
      onRemoteVideoBitrate: (userId, mbps, source) => this.handlers.onRemoteVideoBitrate?.(userId, mbps, source),
      onRemoteVideoKeyframeInterval: (userId, frames, source) =>
        this.handlers.onRemoteVideoKeyframeInterval?.(userId, frames, source),
      onRemoteAudioBitrate: (userId, mbps) => this.handlers.onRemoteAudioBitrate?.(userId, mbps),
      onRemoteVideoReceiveLatency: (userId, ms, source) =>
        this.handlers.onRemoteVideoReceiveLatency?.(userId, ms, source),
      onRemoteVideoRenderingLatency: (userId, ms, source) =>
        this.handlers.onRemoteVideoRenderingLatency?.(userId, ms, source),
      onRemoteVideoTiming: (userId, timing, source) => this.handlers.onRemoteVideoTiming?.(userId, timing, source),
      onRemoteVideoDecodingObject: (userId, decoding, source) =>
        this.handlers.onRemoteVideoDecodingObject?.(userId, decoding, source),
      onRemoteVideoPacing: (userId, pacing, source) => this.handlers.onRemoteVideoPacing?.(userId, pacing, source),
      onRemoteAudioReceiveLatency: (userId, ms) => this.handlers.onRemoteAudioReceiveLatency?.(userId, ms),
      onRemoteAudioRenderingLatency: (userId, ms) => this.handlers.onRemoteAudioRenderingLatency?.(userId, ms),
      onRemoteAudioPlaybackQueue: (userId, queuedMs) => this.handlers.onRemoteAudioPlaybackQueue?.(userId, queuedMs),
      onRemoteVideoJitterBufferActivity: (userId, activity, source) =>
        this.handlers.onRemoteVideoJitterBufferActivity?.(userId, activity, source),
      onRemoteAudioJitterBufferActivity: (userId, activity) =>
        this.handlers.onRemoteAudioJitterBufferActivity?.(userId, activity),
      onRemoteVideoConfig: (userId, config, source) => this.handlers.onRemoteVideoConfig?.(userId, config, source)
    })

    client.setOnIncomingSubscribeHandler(async ({ subscribe, isSuccess, code, respondOk, respondError }) => {
      if (!isSuccess) {
        await respondError(BigInt(code), 'Subscription validation failed')
        return
      }
      if (!this.isLocalTrack(subscribe)) {
        await respondError(404n, 'Unknown namespace')
        return
      }
      const trackName = subscribe.trackName ?? ''
      if (trackName === 'chat') {
        await respondOk(0n)
        return
      }
      if (this.publisher.isCatalogTrack(trackName)) {
        const trackAlias = await respondOk(0n)
        await this.publisher.sendCatalogToAlias(trackAlias)
        return
      }
      const role = this.publisher.resolveTrackRole(trackName)
      if (!role) {
        await respondError(404n, 'Unknown track')
        return
      }
      await respondOk(0n)
      try {
        if (role === 'video') {
          await this.publisher.applyVideoEncodingForTrack(trackName)
        } else if (role === 'audio') {
          await this.publisher.applyAudioEncodingForTrack(trackName)
        }
      } catch (err) {
        console.error('Failed to kick media pipeline for new subscriber', err)
      }
    })

    client.setOnIncomingUnsubscribeHandler((subscribeId) => {
      this.publisher.handleIncomingUnsubscribe(subscribeId)
    })
  }

  setHandlers(handlers: MediaHandlers): void {
    this.handlers = handlers
  }

  async startCamera(deviceId?: string, constraints?: CameraCaptureConstraints): Promise<void> {
    await this.publisher.startCamera(deviceId, constraints)
  }

  async stopCamera(): Promise<void> {
    await this.publisher.stopCamera()
  }

  async startScreenShare(): Promise<void> {
    await this.publisher.startScreenShare()
  }

  async stopScreenShare(): Promise<void> {
    await this.publisher.stopScreenShare()
  }

  async startMicrophone(deviceId?: string, constraints?: AudioCaptureConstraints): Promise<void> {
    await this.publisher.startAudio(deviceId, constraints)
  }

  async stopMicrophone(): Promise<void> {
    await this.publisher.stopAudio()
  }

  registerRemoteTrack(
    userId: string,
    trackName: string,
    trackAlias: bigint,
    role?: CatalogSubscribeRole,
    codec?: string
  ): void {
    const resolvedRole = this.resolveSubscribeRole(trackName, role)
    if (resolvedRole === 'video' || resolvedRole === 'screenshare') {
      this.subscriber.registerVideoTrack(userId, trackName, trackAlias, codec)
    } else if (resolvedRole === 'audio') {
      this.subscriber.registerAudioTrack(userId, trackAlias)
    }
  }

  unregisterRemoteTrack(trackAlias: bigint, role?: CatalogSubscribeRole): void {
    if (role === 'video' || role === 'screenshare') {
      this.subscriber.unregisterVideoTrack(trackAlias)
      return
    }
    if (role === 'audio') {
      this.subscriber.unregisterAudioTrack(trackAlias)
      return
    }
    this.subscriber.unregisterVideoTrack(trackAlias)
    this.subscriber.unregisterAudioTrack(trackAlias)
  }

  getCatalogTracks(): CallCatalogTrack[] {
    return this.publisher.getCatalogTracks()
  }

  getSubscribedCatalogTracks(): SubscribedCatalogTrack[] {
    return this.publisher.getSubscribedCatalogTracks()
  }

  async setCatalogTracks(tracks: CallCatalogTrack[]): Promise<void> {
    await this.publisher.setCatalogTracks(tracks)
  }

  resolveTrackRole(trackName: string): CatalogTrackRole | null {
    return this.publisher.resolveTrackRole(trackName)
  }

  setVideoJitterBufferConfig(userId: string, config: VideoJitterConfig): void {
    this.subscriber.setVideoJitterBufferConfig(userId, config)
  }

  setAudioJitterBufferConfig(userId: string, config: AudioJitterConfig): void {
    this.subscriber.setAudioJitterBufferConfig(userId, config)
  }

  async setVideoEncodingSettings(settings: VideoEncodingSettings, deviceId?: string, restartIfActive: boolean = false) {
    await this.publisher.setVideoEncodingSettings(settings, deviceId, restartIfActive)
  }

  async setScreenShareEncodingSettings(settings: VideoEncodingSettings) {
    await this.publisher.setScreenShareEncodingSettings(settings)
  }

  async setAudioEncodingSettings(settings: AudioEncodingSettings, restartIfActive: boolean = false) {
    await this.publisher.setAudioEncodingSettings(settings, restartIfActive)
  }

  setVideoCaptureConstraints(constraints: CameraCaptureConstraints): void {
    this.publisher.setVideoCaptureConstraints(constraints)
  }

  setAudioCaptureConstraints(constraints: AudioCaptureConstraints): void {
    this.publisher.setAudioCaptureConstraints(constraints)
  }

  private isLocalTrack(subscribe: SubscribeMessage): boolean {
    if (subscribe.trackNamespace.length !== this.trackNamespace.length) {
      return false
    }
    return subscribe.trackNamespace.every((value, index) => value === this.trackNamespace[index])
  }

  private resolveSubscribeRole(trackName: string, role?: CatalogSubscribeRole): CatalogSubscribeRole | null {
    if (role) {
      return role
    }
    const resolved = this.publisher.resolveTrackRole(trackName)
    if (resolved === 'video') {
      return isScreenShareTrackName(trackName) ? 'screenshare' : 'video'
    }
    return resolved
  }
}
