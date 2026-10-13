import { MoqtClientWrapper, RequestErrorCode } from '@moqt/moqtClient'
import { MediaPublisher, type SubscribedCatalogTrack } from './mediaPublisher'
import { MediaSubscriber, type MediaSubscriberHandlers, type RemotePlaybackStats } from './mediaSubscriber'
import type { PlayoutSettings } from '../types/playout'
import type { VideoSendTiming } from '../types/stats'
import type { VideoEncodingSettings } from '../types/videoEncoding'
import type { AudioEncodingSettings } from '../types/audioEncoding'
import type { AudioCaptureConstraints, CameraCaptureConstraints } from '../types/captureConstraints'
import type { MeetingCatalogTrack, CatalogSubscribeRole, CatalogTrackRole, TrackMediaConfig } from '../types/catalog'
import { isScreenShareTrackName } from '../utils/catalogTrackName'

export interface MediaHandlers extends MediaSubscriberHandlers {
  onLocalVideoStream?: (stream: MediaStream | null, source: 'camera' | 'screenshare') => void
  onLocalAudioStream?: (stream: MediaStream | null) => void
  onLocalVideoBitrate?: (mbps: number) => void
  onLocalAudioBitrate?: (mbps: number) => void
  onLocalVideoSendTiming?: (timing: VideoSendTiming | null, source: 'camera' | 'screenshare') => void
  onVideoEncodeError?: (message: string) => void
  onAudioEncodeError?: (message: string) => void
  onAudioEncodingAdjusted?: (settings: AudioEncodingSettings) => void
  onScreenShareEncodingApplied?: (settings: VideoEncodingSettings) => void
}

export class MeetingMediaController {
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
      onRemotePicture: (userId, source, picture) => this.handlers.onRemotePicture?.(userId, source, picture),
      onRemotePictureClosed: (userId, source) => this.handlers.onRemotePictureClosed?.(userId, source)
    })

    client.setOnIncomingSubscribeHandler(async ({ subscribe, isSuccess, code, respondOk, respondError }) => {
      const trackName = subscribe.trackName ?? ''
      const isLocalTrack = this.isLocalNamespace(subscribe.trackNamespace)
      const isCatalogTrack = this.publisher.isCatalogTrack(trackName)
      console.info('[meeting][moqt] received SUBSCRIBE', { subscribe, isSuccess, code })
      if (!isSuccess) {
        await respondError(BigInt(code), 'Subscription validation failed')
        return
      }
      if (!isLocalTrack) {
        await respondError(404n, 'Unknown namespace')
        return
      }
      if (trackName === 'chat') {
        await respondOk(0n)
        return
      }
      if (isCatalogTrack) {
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
          this.publisher.forceVideoKeyframeForTrack(trackName)
        } else if (role === 'audio') {
          await this.publisher.applyAudioEncodingForTrack(trackName)
        }
      } catch (err) {
        console.error('Failed to kick media pipeline for new subscriber', err)
      }
    })

    client.setOnIncomingFetchHandler(async (context) => {
      const { fetch } = context
      if (this.isLocalNamespace(fetch.trackNamespace) && this.publisher.isCatalogTrack(fetch.trackName)) {
        await this.publisher.answerCatalogFetch(context)
        return
      }
      await context.respondError(RequestErrorCode.NotSupported, 'fetch is served for the catalog only')
    })

    client.setOnIncomingUnsubscribeHandler((subscribeId) => {
      console.info('[meeting][moqt] received UNSUBSCRIBE', { subscribeId: subscribeId.toString() })
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
    config?: TrackMediaConfig
  ): void {
    const resolvedRole = this.resolveSubscribeRole(trackName, role)
    if (resolvedRole === 'video' || resolvedRole === 'screenshare') {
      this.subscriber.registerVideoTrack(userId, trackName, trackAlias, config)
    } else if (resolvedRole === 'audio') {
      this.subscriber.registerAudioTrack(userId, trackName, trackAlias, config)
    }
  }

  unregisterRemoteTrack(trackAlias: bigint): void {
    this.subscriber.unregisterTrack(trackAlias)
  }

  async dispose(): Promise<void> {
    await this.publisher.dispose()
    this.subscriber.dispose()
    this.handlers = {}
  }

  getCatalogTracks(): MeetingCatalogTrack[] {
    return this.publisher.getCatalogTracks()
  }

  getSubscribedCatalogTracks(): SubscribedCatalogTrack[] {
    return this.publisher.getSubscribedCatalogTracks()
  }

  async setCatalogTracks(tracks: MeetingCatalogTrack[]): Promise<void> {
    await this.publisher.setCatalogTracks(tracks)
  }

  resolveTrackRole(trackName: string): CatalogTrackRole | null {
    return this.publisher.resolveTrackRole(trackName)
  }

  setPlayoutSettings(userId: string, settings: PlayoutSettings): void {
    this.subscriber.setPlayoutSettings(userId, settings)
  }

  getRemotePlaybackStats(userId: string): RemotePlaybackStats {
    return this.subscriber.stats(userId)
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

  private isLocalNamespace(trackNamespace: string[]): boolean {
    if (trackNamespace.length !== this.trackNamespace.length) {
      return false
    }
    return trackNamespace.every((value, index) => value === this.trackNamespace[index])
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
