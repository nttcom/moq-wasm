import type { MoqSession } from 'moq-wasm'
import { type LocalBroadcast, type LocalSource, createBroadcast } from 'moq-wasm/media'
import type { AudioCaptureConstraints, CameraCaptureConstraints } from '../types/captureConstraints'
import { DEFAULT_SCREEN_SHARE_ENCODING_SETTINGS, type VideoEncodingSettings } from '../types/videoEncoding'
import { CAMERA_RENDITIONS, INITIAL_CAMERA_ENCODING_SETTINGS, SCREEN_SHARE_RENDITIONS } from './meetingCatalog'
import { SCREEN_SHARE_SOURCE } from '../utils/catalogTrackName'
import { captureWithRetry } from './captureRetry'

type LocalStreamHandler = (stream: MediaStream | null) => void

export type MediaPublisherHandlers = {
  onLocalCameraStream?: LocalStreamHandler
  onLocalScreenShareStream?: LocalStreamHandler
  onLocalAudioStream?: LocalStreamHandler
  onPublishError?: (message: string) => void
  /// A new capture of the default device replaced a microphone whose track ended.
  onMicrophoneSwitched?: (track: MediaStreamTrack) => void
  /// No microphone could replace the one whose track ended; the audio is off.
  onMicrophoneLost?: () => void
}

const AUDIO_PUBLISHER_LOG_PREFIX = '[meeting][publisher][audio]'
const MAX_MICROPHONE_RECAPTURE_ATTEMPTS = 3
const MICROPHONE_RECAPTURE_RETRY_DELAY_MS = 1000

/// The member's media in one broadcast: the camera and microphone as the
/// source `camera`, the shared screen as the source `screenshare`.
export class MediaPublisher {
  private handlers: MediaPublisherHandlers = {}
  private broadcast: LocalBroadcast | null = null
  private camera: LocalSource | null = null
  private screenShare: LocalSource | null = null

  private cameraStream: MediaStream | null = null
  private screenShareStream: MediaStream | null = null
  private audioStream: MediaStream | null = null

  private videoEncodingSettings: VideoEncodingSettings = INITIAL_CAMERA_ENCODING_SETTINGS
  private screenShareEncodingSettings: VideoEncodingSettings = DEFAULT_SCREEN_SHARE_ENCODING_SETTINGS

  private videoCaptureConstraints: CameraCaptureConstraints = { frameRate: 30 }
  private audioCaptureConstraints: AudioCaptureConstraints = {
    echoCancellation: true,
    noiseSuppression: true,
    autoGainControl: true
  }

  constructor(private readonly trackNamespace: string[]) {}

  /// Announces the member's namespace on `session`; call before starting a device.
  async start(session: MoqSession): Promise<void> {
    this.broadcast = await createBroadcast(session, this.trackNamespace, {
      offerChat: true,
      onError: (error) => this.handlers.onPublishError?.(error instanceof Error ? error.message : String(error))
    })
    this.camera = await this.broadcast.addSource('camera', {
      label: 'Camera',
      videoEncoderConfig: this.videoEncodingSettings,
      renditions: CAMERA_RENDITIONS
    })
  }

  setHandlers(handlers: MediaPublisherHandlers): void {
    this.handlers = handlers
  }

  setVideoCaptureConstraints(constraints: CameraCaptureConstraints): void {
    this.videoCaptureConstraints = { ...this.videoCaptureConstraints, ...constraints }
  }

  setAudioCaptureConstraints(constraints: AudioCaptureConstraints): void {
    this.audioCaptureConstraints = { ...this.audioCaptureConstraints, ...constraints }
  }

  async startCamera(deviceId?: string): Promise<void> {
    if (this.cameraStream) {
      return
    }
    const camera = this.started()
    const stream = await navigator.mediaDevices.getUserMedia({
      video: {
        width: this.videoCaptureConstraints.width ?? this.videoEncodingSettings.width,
        height: this.videoCaptureConstraints.height ?? this.videoEncodingSettings.height,
        frameRate: this.videoCaptureConstraints.frameRate,
        ...(deviceId ? { deviceId: { exact: deviceId } } : {})
      },
      audio: false
    })
    this.cameraStream = stream
    this.handlers.onLocalCameraStream?.(stream)
    const [track] = stream.getVideoTracks()
    track.onended = () => {
      this.stopCamera().catch((error) => console.warn('Failed to stop camera on track end', error))
    }
    await camera.setVideo(track)
  }

  async stopCamera(): Promise<void> {
    const stream = this.cameraStream
    this.cameraStream = null
    this.handlers.onLocalCameraStream?.(null)
    await this.camera?.setVideo(false)
    releaseStream(stream)
  }

  async startScreenShare(): Promise<void> {
    if (this.screenShareStream) {
      return
    }
    this.started()
    const broadcast = this.broadcast
    if (!broadcast) {
      return
    }
    const stream = await navigator.mediaDevices.getDisplayMedia({ video: { frameRate: 30 }, audio: false })
    this.screenShareStream = stream
    this.handlers.onLocalScreenShareStream?.(stream)
    const [track] = stream.getVideoTracks()
    track.onended = () => {
      this.stopScreenShare().catch((error) => console.warn('Failed to stop screen share on track end', error))
    }
    this.screenShare = await broadcast.addSource(SCREEN_SHARE_SOURCE, {
      label: 'Screen',
      stream,
      videoEncoderConfig: this.screenShareEncodingSettings,
      renditions: SCREEN_SHARE_RENDITIONS
    })
  }

  async stopScreenShare(): Promise<void> {
    const stream = this.screenShareStream
    this.screenShareStream = null
    this.screenShare = null
    this.handlers.onLocalScreenShareStream?.(null)
    await this.broadcast?.removeSource(SCREEN_SHARE_SOURCE)
    releaseStream(stream)
  }

  async startAudio(deviceId?: string): Promise<void> {
    if (this.audioStream) {
      return
    }
    this.started()
    await this.attachAudioStream(await this.captureAudio(deviceId))
  }

  async stopAudio(): Promise<void> {
    const stream = this.audioStream
    this.audioStream = null
    this.handlers.onLocalAudioStream?.(null)
    await this.camera?.setAudio(false)
    releaseStream(stream)
  }

  setVideoEncodingSettings(settings: VideoEncodingSettings): void {
    this.videoEncodingSettings = settings
    this.camera?.setVideoEncoderConfig(settings)
  }

  setScreenShareEncodingSettings(settings: VideoEncodingSettings): void {
    this.screenShareEncodingSettings = settings
    this.screenShare?.setVideoEncoderConfig(settings)
  }

  async dispose(): Promise<void> {
    for (const stream of [this.cameraStream, this.screenShareStream, this.audioStream]) {
      releaseStream(stream)
    }
    this.cameraStream = null
    this.screenShareStream = null
    this.audioStream = null
    await this.broadcast?.close()
    this.broadcast = null
    this.camera = null
    this.screenShare = null
  }

  private started(): LocalSource {
    if (!this.camera) {
      throw new Error('the media publisher has not started')
    }
    return this.camera
  }

  private captureAudio(deviceId?: string): Promise<MediaStream> {
    return navigator.mediaDevices.getUserMedia({
      audio: {
        ...this.audioCaptureConstraints,
        channelCount: { ideal: 1 },
        ...(deviceId ? { deviceId: { exact: deviceId } } : {})
      },
      video: false
    })
  }

  private async attachAudioStream(stream: MediaStream): Promise<void> {
    this.audioStream = stream
    this.handlers.onLocalAudioStream?.(stream)
    const [track] = stream.getAudioTracks()
    track.onended = () => void this.recaptureEndedAudio(stream)
    await this.started().setAudio(track)
  }

  private async recaptureEndedAudio(endedStream: MediaStream): Promise<void> {
    console.warn(`${AUDIO_PUBLISHER_LOG_PREFIX} microphone track ended; capturing the default device`)
    let stream: MediaStream | null
    try {
      stream = await captureWithRetry(() => this.captureAudio(), {
        maxAttempts: MAX_MICROPHONE_RECAPTURE_ATTEMPTS,
        retryDelayMs: MICROPHONE_RECAPTURE_RETRY_DELAY_MS,
        isCancelled: () => this.audioStream !== endedStream
      })
    } catch (error) {
      console.warn(`${AUDIO_PUBLISHER_LOG_PREFIX} no microphone to replace the ended one`, error)
      await this.stopAudio()
      this.handlers.onMicrophoneLost?.()
      return
    }
    if (!stream) {
      return
    }
    releaseStream(this.audioStream)
    await this.attachAudioStream(stream)
    this.handlers.onMicrophoneSwitched?.(stream.getAudioTracks()[0])
  }
}

function releaseStream(stream: MediaStream | null): void {
  stream?.getTracks().forEach((track) => {
    track.onended = null
    track.stop()
  })
}
