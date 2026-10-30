import type { MoqSession } from '@moqt/session'
import { type Publication, type SourcePublication, publish } from '@publish/publish'
import type { AudioCaptureConstraints, CameraCaptureConstraints } from '../types/captureConstraints'
import { DEFAULT_SCREEN_SHARE_ENCODING_SETTINGS, type VideoEncodingSettings } from '../types/videoEncoding'
import {
  CAMERA_KEYFRAME_INTERVAL,
  CAMERA_RENDITIONS,
  INITIAL_CAMERA_ENCODING_SETTINGS,
  SCREEN_SHARE_RENDITIONS
} from './meetingCatalog'
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

/// The member's media in one `publish()` broadcast: the camera and microphone
/// on its `video` and `audio` tracks, the shared screen as the `screenshare`
/// source.
export class MediaPublisher {
  private handlers: MediaPublisherHandlers = {}
  private publication: Publication | null = null
  private screenShare: SourcePublication | null = null

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
    this.publication = await publish(session, this.trackNamespace, new MediaStream(), {
      videoLabel: 'Camera',
      videoEncoderConfig: this.videoEncodingSettings,
      renditions: CAMERA_RENDITIONS,
      keyframeIntervalFrames: CAMERA_KEYFRAME_INTERVAL,
      offerChat: true,
      onError: (error) => this.handlers.onPublishError?.(error instanceof Error ? error.message : String(error))
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
    const publication = this.started()
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
    publication.replaceTrack('video', track)
    await publication.setMuted('video', false)
  }

  async stopCamera(): Promise<void> {
    releaseStream(this.cameraStream)
    this.cameraStream = null
    this.handlers.onLocalCameraStream?.(null)
    await this.publication?.setMuted('video', true)
  }

  async startScreenShare(): Promise<void> {
    if (this.screenShareStream) {
      return
    }
    const publication = this.started()
    const stream = await navigator.mediaDevices.getDisplayMedia({ video: { frameRate: 30 }, audio: false })
    this.screenShareStream = stream
    this.handlers.onLocalScreenShareStream?.(stream)
    const [track] = stream.getVideoTracks()
    track.onended = () => {
      this.stopScreenShare().catch((error) => console.warn('Failed to stop screen share on track end', error))
    }
    if (this.screenShare) {
      this.screenShare.replaceTrack('video', track)
      await this.screenShare.setMuted('video', false)
      return
    }
    this.screenShare = publication.addSource(SCREEN_SHARE_SOURCE, stream, {
      videoLabel: 'Screen',
      videoEncoderConfig: this.screenShareEncodingSettings,
      renditions: SCREEN_SHARE_RENDITIONS,
      keyframeIntervalFrames: CAMERA_KEYFRAME_INTERVAL
    })
  }

  async stopScreenShare(): Promise<void> {
    releaseStream(this.screenShareStream)
    this.screenShareStream = null
    this.handlers.onLocalScreenShareStream?.(null)
    await this.screenShare?.setMuted('video', true)
  }

  async startAudio(deviceId?: string): Promise<void> {
    if (this.audioStream) {
      return
    }
    this.started()
    await this.attachAudioStream(await this.captureAudio(deviceId))
  }

  async stopAudio(): Promise<void> {
    releaseStream(this.audioStream)
    this.audioStream = null
    this.handlers.onLocalAudioStream?.(null)
    await this.publication?.setMuted('audio', true)
  }

  setVideoEncodingSettings(settings: VideoEncodingSettings): void {
    this.videoEncodingSettings = settings
    this.publication?.setVideoEncoderConfig(settings)
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
    await this.publication?.close()
    this.publication = null
    this.screenShare = null
  }

  private started(): Publication {
    if (!this.publication) {
      throw new Error('the media publisher has not started')
    }
    return this.publication
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
    const publication = this.started()
    publication.replaceTrack('audio', track)
    await publication.setMuted('audio', false)
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
