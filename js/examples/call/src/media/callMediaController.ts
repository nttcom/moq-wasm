import { MoqtClientWrapper } from '@moqt/moqtClient'
import { MediaPublisher } from './mediaPublisher'
import { MediaSubscriber } from './mediaSubscriber'
import type { VideoJitterConfig, AudioJitterConfig } from '../types/jitterBuffer'
import type { VideoEncodingSettings } from '../types/videoEncoding'
import type { AudioEncodingSettings } from '../types/audioEncoding'

export interface MediaHandlers {
  onLocalVideoStream?: (stream: MediaStream | null) => void
  onLocalAudioStream?: (stream: MediaStream | null) => void
  onLocalVideoBitrate?: (mbps: number) => void
  onLocalAudioBitrate?: (mbps: number) => void
  onRemoteVideoStream?: (userId: string, stream: MediaStream) => void
  onRemoteAudioStream?: (userId: string, stream: MediaStream) => void
  onRemoteVideoBitrate?: (userId: string, mbps: number) => void
  onRemoteAudioBitrate?: (userId: string, mbps: number) => void
  onRemoteVideoReceiveLatency?: (userId: string, ms: number) => void
  onRemoteVideoRenderingLatency?: (userId: string, ms: number) => void
  onRemoteAudioReceiveLatency?: (userId: string, ms: number) => void
  onRemoteAudioRenderingLatency?: (userId: string, ms: number) => void
  onRemoteVideoConfig?: (userId: string, config: { codec: string; width?: number; height?: number }) => void
  onVideoEncodeError?: (message: string) => void
  onAudioEncodeError?: (message: string) => void
  onAudioEncodingAdjusted?: (settings: AudioEncodingSettings) => void
  onScreenShareEncodingApplied?: (settings: VideoEncodingSettings) => void
}

export class CallMediaController {
  private handlers: MediaHandlers = {}
  private readonly publisher: MediaPublisher
  private readonly subscriber: MediaSubscriber

  constructor(client: MoqtClientWrapper, trackNamespace: string[]) {
    this.publisher = new MediaPublisher(client, trackNamespace)
    this.subscriber = new MediaSubscriber(client)

    this.publisher.setHandlers({
      onLocalVideoStream: (stream) => this.handlers.onLocalVideoStream?.(stream),
      onLocalAudioStream: (stream) => this.handlers.onLocalAudioStream?.(stream),
      onEncodedVideoBitrate: (mbps) => this.handlers.onLocalVideoBitrate?.(mbps),
      onEncodedAudioBitrate: (mbps) => this.handlers.onLocalAudioBitrate?.(mbps),
      onVideoEncodeError: (message) => this.handlers.onVideoEncodeError?.(message),
      onAudioEncodeError: (message) => this.handlers.onAudioEncodeError?.(message),
      onAudioEncodingAdjusted: (settings) => this.handlers.onAudioEncodingAdjusted?.(settings),
      onScreenShareEncodingApplied: (settings) => this.handlers.onScreenShareEncodingApplied?.(settings)
    })

    this.subscriber.setHandlers({
      onRemoteVideoStream: (userId, stream) => this.handlers.onRemoteVideoStream?.(userId, stream),
      onRemoteAudioStream: (userId, stream) => this.handlers.onRemoteAudioStream?.(userId, stream),
      onRemoteVideoBitrate: (userId, mbps) => this.handlers.onRemoteVideoBitrate?.(userId, mbps),
      onRemoteAudioBitrate: (userId, mbps) => this.handlers.onRemoteAudioBitrate?.(userId, mbps),
      onRemoteVideoReceiveLatency: (userId, ms) => this.handlers.onRemoteVideoReceiveLatency?.(userId, ms),
      onRemoteVideoRenderingLatency: (userId, ms) => this.handlers.onRemoteVideoRenderingLatency?.(userId, ms),
      onRemoteAudioReceiveLatency: (userId, ms) => this.handlers.onRemoteAudioReceiveLatency?.(userId, ms),
      onRemoteAudioRenderingLatency: (userId, ms) => this.handlers.onRemoteAudioRenderingLatency?.(userId, ms),
      onRemoteVideoConfig: (userId, config) => this.handlers.onRemoteVideoConfig?.(userId, config)
    })
  }

  setHandlers(handlers: MediaHandlers): void {
    this.handlers = handlers
  }

  async startCamera(deviceId?: string): Promise<void> {
    await this.publisher.startVideo(deviceId)
  }

  async stopCamera(): Promise<void> {
    await this.publisher.stopVideo()
  }

  async startScreenShare(): Promise<void> {
    await this.publisher.startScreenShare()
  }

  async stopScreenShare(): Promise<void> {
    await this.publisher.stopVideo()
  }

  async startMicrophone(deviceId?: string): Promise<void> {
    await this.publisher.startAudio(deviceId)
  }

  async stopMicrophone(): Promise<void> {
    await this.publisher.stopAudio()
  }

  registerRemoteTrack(userId: string, trackName: string, trackAlias: bigint): void {
    if (trackName === 'video') {
      this.subscriber.registerVideoTrack(userId, trackAlias)
    } else if (trackName === 'audio') {
      this.subscriber.registerAudioTrack(userId, trackAlias)
    }
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
}
