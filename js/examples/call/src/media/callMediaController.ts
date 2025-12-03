import { MoqtClientWrapper } from '@moqt/moqtClient'
import { MediaPublisher } from './mediaPublisher'
import { MediaSubscriber } from './mediaSubscriber'

export interface MediaHandlers {
  onLocalVideoStream?: (stream: MediaStream | null) => void
  onLocalAudioStream?: (stream: MediaStream | null) => void
  onLocalVideoBitrate?: (mbps: number) => void
  onLocalAudioBitrate?: (mbps: number) => void
  onRemoteVideoStream?: (userId: string, stream: MediaStream) => void
  onRemoteAudioStream?: (userId: string, stream: MediaStream) => void
  onRemoteVideoBitrate?: (userId: string, mbps: number) => void
  onRemoteAudioBitrate?: (userId: string, mbps: number) => void
  onRemoteVideoLatency?: (userId: string, ms: number) => void
  onRemoteVideoJitter?: (userId: string, ms: number) => void
  onRemoteAudioLatency?: (userId: string, ms: number) => void
  onRemoteAudioJitter?: (userId: string, ms: number) => void
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
      onEncodedAudioBitrate: (mbps) => this.handlers.onLocalAudioBitrate?.(mbps)
    })

    this.subscriber.setHandlers({
      onRemoteVideoStream: (userId, stream) => this.handlers.onRemoteVideoStream?.(userId, stream),
      onRemoteAudioStream: (userId, stream) => this.handlers.onRemoteAudioStream?.(userId, stream),
      onRemoteVideoBitrate: (userId, mbps) => this.handlers.onRemoteVideoBitrate?.(userId, mbps),
      onRemoteAudioBitrate: (userId, mbps) => this.handlers.onRemoteAudioBitrate?.(userId, mbps),
      onRemoteVideoLatency: (userId, ms) => this.handlers.onRemoteVideoLatency?.(userId, ms),
      onRemoteVideoJitter: (userId, ms) => this.handlers.onRemoteVideoJitter?.(userId, ms),
      onRemoteAudioLatency: (userId, ms) => this.handlers.onRemoteAudioLatency?.(userId, ms),
      onRemoteAudioJitter: (userId, ms) => this.handlers.onRemoteAudioJitter?.(userId, ms)
    })
  }

  setHandlers(handlers: MediaHandlers): void {
    this.handlers = handlers
  }

  async startCamera(): Promise<void> {
    await this.publisher.startVideo()
  }

  async stopCamera(): Promise<void> {
    await this.publisher.stopVideo()
  }

  async startMicrophone(): Promise<void> {
    await this.publisher.startAudio()
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
}
