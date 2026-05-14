import { useCallback, useEffect, useRef, useState } from 'react'
import { LocalSession } from '../session/localSession'
import type { JitterBufferEvent, JitterBufferSnapshot, RemoteMediaStreams } from '../types/media'
import {
  DEFAULT_VIDEO_JITTER_CONFIG,
  normalizeVideoJitterConfig,
  type VideoJitterConfig,
  DEFAULT_AUDIO_JITTER_CONFIG,
  normalizeAudioJitterConfig,
  type AudioJitterConfig
} from '../types/jitterBuffer'
import {
  DEFAULT_VIDEO_ENCODING_SETTINGS,
  VIDEO_BITRATE_OPTIONS,
  VIDEO_CODEC_OPTIONS,
  VIDEO_HARDWARE_ACCELERATION_OPTIONS,
  VIDEO_RESOLUTION_OPTIONS,
  type VideoEncodingSettings
} from '../types/videoEncoding'
import {
  AUDIO_BITRATE_OPTIONS,
  AUDIO_CODEC_OPTIONS,
  AUDIO_CHANNEL_OPTIONS,
  DEFAULT_AUDIO_ENCODING_SETTINGS,
  type AudioEncodingSettings
} from '../types/audioEncoding'
import type { CaptureSettingsState } from '../types/captureConstraints'
import type { EditableCallCatalogTrack } from '../types/catalog'
import { isScreenShareTrackName } from '../utils/catalogTrackName'
import {
  appendCatalogTracks,
  createCatalogTrackId,
  getAudioCatalogTrackNames,
  getAudioCatalogTracks,
  getCameraCatalogTrackNames,
  getCameraCatalogTracks,
  getScreenShareCatalogTrackNames,
  getScreenShareCatalogTracks,
  removeCatalogTracksByNames,
  toCatalogTracks,
  toEditableCatalogTracks
} from '../media/callCatalog'
import type { SubscribedCatalogTrack } from '../media/mediaPublisher'

interface UseCallMediaResult {
  cameraEnabled: boolean
  screenShareEnabled: boolean
  microphoneEnabled: boolean
  cameraBusy: boolean
  microphoneBusy: boolean
  localVideoStream: MediaStream | null
  localScreenShareStream: MediaStream | null
  localAudioStream: MediaStream | null
  localVideoBitrate: number | null
  localAudioBitrate: number | null
  localCameraVideoSendTiming: LocalVideoSendTiming | null
  localScreenShareVideoSendTiming: LocalVideoSendTiming | null
  remoteMedia: Map<string, RemoteMediaStreams>
  toggleCamera: () => Promise<boolean>
  toggleScreenShare: () => Promise<boolean>
  toggleMicrophone: () => Promise<boolean>
  videoJitterConfigs: Map<string, VideoJitterConfig>
  setVideoJitterBufferConfig: (userId: string, config: Partial<VideoJitterConfig>) => void
  audioJitterConfigs: Map<string, AudioJitterConfig>
  setAudioJitterBufferConfig: (userId: string, config: Partial<AudioJitterConfig>) => void
  videoCodecOptions: typeof VIDEO_CODEC_OPTIONS
  videoResolutionOptions: typeof VIDEO_RESOLUTION_OPTIONS
  videoBitrateOptions: typeof VIDEO_BITRATE_OPTIONS
  videoHardwareAccelerationOptions: typeof VIDEO_HARDWARE_ACCELERATION_OPTIONS
  selectedVideoEncoding: VideoEncodingSettings
  selectVideoEncoding: (settings: Partial<VideoEncodingSettings>) => Promise<void>
  selectedScreenShareEncoding: VideoEncodingSettings
  selectScreenShareEncoding: (settings: Partial<VideoEncodingSettings>) => Promise<void>
  videoEncoderError: string | null
  audioCodecOptions: typeof AUDIO_CODEC_OPTIONS
  audioBitrateOptions: typeof AUDIO_BITRATE_OPTIONS
  audioChannelOptions: typeof AUDIO_CHANNEL_OPTIONS
  selectedAudioEncoding: AudioEncodingSettings
  selectAudioEncoding: (settings: Partial<AudioEncodingSettings>) => Promise<void>
  audioEncoderError: string | null
  videoDevices: MediaDeviceInfo[]
  audioDevices: MediaDeviceInfo[]
  selectedVideoDeviceId: string | null
  selectedAudioDeviceId: string | null
  selectVideoDevice: (deviceId: string) => Promise<void>
  selectAudioDevice: (deviceId: string) => Promise<void>
  captureSettings: CaptureSettingsState
  updateCaptureSettings: (settings: Partial<CaptureSettingsState>) => void
  applyCaptureSettings: () => Promise<void>
  catalogTracks: EditableCallCatalogTrack[]
  subscribedCatalogTracks: SubscribedCatalogTrack[]
  addCatalogTrack: (track: Omit<EditableCallCatalogTrack, 'id'>) => void
  updateCatalogTrack: (id: string, patch: Partial<EditableCallCatalogTrack>) => void
  removeCatalogTrack: (id: string) => void
}

type LocalVideoSendTiming = {
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
}

const DEFAULT_SCREEN_SHARE_ENCODING_SETTINGS: VideoEncodingSettings = {
  codec: VIDEO_CODEC_OPTIONS.find((c) => c.id.startsWith('av1'))?.codec ?? VIDEO_CODEC_OPTIONS[3].codec,
  width: 1920,
  height: 1080,
  bitrate: VIDEO_BITRATE_OPTIONS.find((b) => b.id === '1mbps')?.bitrate ?? VIDEO_BITRATE_OPTIONS[2].bitrate,
  framerate: 30,
  hardwareAcceleration: 'prefer-software'
}

type CatalogPresetSource = 'camera' | 'screenshare' | 'audio'
type CatalogPreset = {
  label: string
  append: () => ReturnType<typeof getCameraCatalogTracks>
  names: () => string[]
}

const CATALOG_PRESETS: Record<CatalogPresetSource, CatalogPreset> = {
  camera: {
    label: 'camera',
    append: getCameraCatalogTracks,
    names: getCameraCatalogTrackNames
  },
  screenshare: {
    label: 'screenshare',
    append: getScreenShareCatalogTracks,
    names: getScreenShareCatalogTrackNames
  },
  audio: {
    label: 'audio',
    append: getAudioCatalogTracks,
    names: getAudioCatalogTrackNames
  }
}

type RenderingRateState = {
  lastEventAtMs: number
  smoothedFps: number
}

const RENDERING_RATE_SMOOTHING_FACTOR = 0.2
const MIN_RENDERING_INTERVAL_MS = 1
const MAX_RENDERING_FPS = 120

export function useCallMedia(session: LocalSession | null): UseCallMediaResult {
  const [cameraEnabled, setCameraEnabled] = useState(false)
  const [screenShareEnabled, setScreenShareEnabled] = useState(false)
  const [microphoneEnabled, setMicrophoneEnabled] = useState(false)
  const [cameraBusy, setCameraBusy] = useState(false)
  const [microphoneBusy, setMicrophoneBusy] = useState(false)
  const [localVideoStream, setLocalVideoStream] = useState<MediaStream | null>(null)
  const [localScreenShareStream, setLocalScreenShareStream] = useState<MediaStream | null>(null)
  const [localAudioStream, setLocalAudioStream] = useState<MediaStream | null>(null)
  const [localVideoBitrate, setLocalVideoBitrate] = useState<number | null>(null)
  const [localAudioBitrate, setLocalAudioBitrate] = useState<number | null>(null)
  const [localCameraVideoSendTiming, setLocalCameraVideoSendTiming] = useState<LocalVideoSendTiming | null>(null)
  const [localScreenShareVideoSendTiming, setLocalScreenShareVideoSendTiming] = useState<LocalVideoSendTiming | null>(
    null
  )
  const [remoteMedia, setRemoteMedia] = useState<Map<string, RemoteMediaStreams>>(new Map())
  const [videoJitterConfigs, setVideoJitterConfigs] = useState<Map<string, VideoJitterConfig>>(new Map())
  const [audioJitterConfigs, setAudioJitterConfigs] = useState<Map<string, AudioJitterConfig>>(new Map())
  const [videoDevices, setVideoDevices] = useState<MediaDeviceInfo[]>([])
  const [audioDevices, setAudioDevices] = useState<MediaDeviceInfo[]>([])
  const [selectedVideoDeviceId, setSelectedVideoDeviceId] = useState<string | null>(null)
  const [selectedAudioDeviceId, setSelectedAudioDeviceId] = useState<string | null>(null)
  const [selectedVideoEncoding, setSelectedVideoEncoding] = useState<VideoEncodingSettings>(
    DEFAULT_VIDEO_ENCODING_SETTINGS
  )
  const [selectedScreenShareEncoding, setSelectedScreenShareEncoding] = useState<VideoEncodingSettings>(
    DEFAULT_SCREEN_SHARE_ENCODING_SETTINGS
  )
  const [videoEncoderError, setVideoEncoderError] = useState<string | null>(null)
  const [selectedAudioEncoding, setSelectedAudioEncoding] = useState<AudioEncodingSettings>(
    DEFAULT_AUDIO_ENCODING_SETTINGS
  )
  const [audioEncoderError, setAudioEncoderError] = useState<string | null>(null)
  const [captureSettings, setCaptureSettings] = useState<CaptureSettingsState>({
    videoEnabled: true,
    audioEnabled: true,
    width: DEFAULT_VIDEO_ENCODING_SETTINGS.width,
    height: DEFAULT_VIDEO_ENCODING_SETTINGS.height,
    frameRate: 30,
    echoCancellation: true,
    noiseSuppression: true,
    autoGainControl: true
  })
  const [catalogTracks, setCatalogTracks] = useState<EditableCallCatalogTrack[]>([])
  const [subscribedCatalogTracks, setSubscribedCatalogTracks] = useState<SubscribedCatalogTrack[]>([])
  const videoRenderingRateStateRef = useRef<Map<string, RenderingRateState>>(new Map())
  const audioRenderingRateStateRef = useRef<Map<string, RenderingRateState>>(new Map())

  const refreshDevices = useCallback(async () => {
    if (!navigator.mediaDevices?.enumerateDevices) {
      return
    }
    const devices = await navigator.mediaDevices.enumerateDevices()
    const videos = devices.filter((d) => d.kind === 'videoinput')
    const audios = devices.filter((d) => d.kind === 'audioinput')
    setVideoDevices(videos)
    setAudioDevices(audios)
    if (!selectedVideoDeviceId && videos[0]) {
      setSelectedVideoDeviceId(videos[0].deviceId)
    }
    if (!selectedAudioDeviceId && audios[0]) {
      setSelectedAudioDeviceId(audios[0].deviceId)
    }
  }, [selectedAudioDeviceId, selectedVideoDeviceId])

  useEffect(() => {
    if (!session) {
      setLocalVideoStream(null)
      setLocalScreenShareStream(null)
      setLocalAudioStream(null)
      setLocalCameraVideoSendTiming(null)
      setLocalScreenShareVideoSendTiming(null)
      setRemoteMedia(new Map())
      setCameraEnabled(false)
      setMicrophoneEnabled(false)
      setScreenShareEnabled(false)
      setVideoJitterConfigs(new Map())
      setAudioJitterConfigs(new Map())
      setVideoEncoderError(null)
      setAudioEncoderError(null)
      setCatalogTracks([])
      setSubscribedCatalogTracks([])
      videoRenderingRateStateRef.current.clear()
      audioRenderingRateStateRef.current.clear()
      return
    }

    const controller = session.getMediaController()
    const initialCatalogTracks = toEditableCatalogTracks(controller.getCatalogTracks())
    setSubscribedCatalogTracks(controller.getSubscribedCatalogTracks())
    setCatalogTracks(initialCatalogTracks)
    const initialVideoEncoding = deriveCameraEncodingFromCatalogTracks(
      initialCatalogTracks,
      DEFAULT_VIDEO_ENCODING_SETTINGS
    )
    const initialScreenShareEncoding = deriveScreenShareEncodingFromCatalogTracks(
      initialCatalogTracks,
      DEFAULT_SCREEN_SHARE_ENCODING_SETTINGS
    )
    const initialAudioEncoding = deriveAudioEncodingFromCatalogTracks(
      initialCatalogTracks,
      DEFAULT_AUDIO_ENCODING_SETTINGS
    )
    setSelectedVideoEncoding(initialVideoEncoding)
    setSelectedScreenShareEncoding(initialScreenShareEncoding)
    setSelectedAudioEncoding(initialAudioEncoding)
    void controller.setVideoEncodingSettings(initialVideoEncoding, undefined, false).catch((error) => {
      console.error('Failed to apply video encoding from catalog:', error)
    })
    void controller.setScreenShareEncodingSettings(initialScreenShareEncoding).catch((error) => {
      console.error('Failed to apply screen share encoding from catalog:', error)
    })
    void controller.setAudioEncodingSettings(initialAudioEncoding, false).catch((error) => {
      console.error('Failed to apply audio encoding from catalog:', error)
    })
    controller.setHandlers({
      onLocalVideoStream: (stream, source) => {
        if (source === 'camera') {
          setLocalVideoStream(stream)
          setCameraEnabled(Boolean(stream))
          return
        }
        setLocalScreenShareStream(stream)
        setScreenShareEnabled(Boolean(stream))
      },
      onLocalAudioStream: (stream) => {
        setLocalAudioStream(stream)
        setMicrophoneEnabled(!!stream)
      },
      onRemoteVideoStream: (userId, stream, source) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          if (source === 'screenshare') {
            updated.set(userId, { ...current, screenShareStream: stream })
            return updated
          }
          updated.set(userId, { ...current, videoStream: stream })
          return updated
        }),
      onRemoteAudioStream: (userId, stream) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          updated.set(userId, { ...current, audioStream: stream, audioPlaybackQueueMs: 0 })
          return updated
        }),
      onRemoteAudioStreamClosed: (userId) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId)
          if (!current) {
            return prev
          }
          updated.set(userId, {
            ...current,
            audioStream: null,
            audioPlaybackQueueMs: 0
          })
          return updated
        }),
      onLocalVideoBitrate: (kbps) => setLocalVideoBitrate(kbps),
      onLocalAudioBitrate: (kbps) => setLocalAudioBitrate(kbps),
      onLocalVideoSendTiming: (timing, source) => {
        if (source === 'screenshare') {
          setLocalScreenShareVideoSendTiming(timing)
          return
        }
        setLocalCameraVideoSendTiming(timing)
      },
      onRemoteVideoBitrate: (userId, kbps, source) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          if (source === 'screenshare') {
            updated.set(userId, { ...current, screenShareBitrateKbps: kbps })
            return updated
          }
          updated.set(userId, { ...current, videoBitrateKbps: kbps })
          return updated
        }),
      onRemoteVideoKeyframeInterval: (userId, frames, source) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          if (source === 'screenshare') {
            updated.set(userId, { ...current, screenShareKeyframeIntervalFrames: frames })
            return updated
          }
          updated.set(userId, { ...current, videoKeyframeIntervalFrames: frames })
          return updated
        }),
      onRemoteAudioBitrate: (userId, kbps) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          updated.set(userId, { ...current, audioBitrateKbps: kbps })
          return updated
        }),
      onRemoteVideoReceiveLatency: (userId, ms, source) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          if (source === 'screenshare') {
            updated.set(userId, {
              ...current,
              screenShareLatencyReceiveMs: ms
            })
            return updated
          }
          updated.set(userId, { ...current, videoLatencyReceiveMs: ms })
          return updated
        }),
      onRemoteVideoRenderingLatency: (userId, ms, source) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          const renderingRate = updateRenderingRate(
            videoRenderingRateStateRef.current,
            buildVideoRenderingRateKey(userId, source)
          )
          if (source === 'screenshare') {
            updated.set(userId, {
              ...current,
              screenShareLatencyRenderMs: ms,
              screenShareRenderingRateFps: renderingRate
            })
            return updated
          }
          updated.set(userId, {
            ...current,
            videoLatencyRenderMs: ms,
            videoRenderingRateFps: renderingRate
          })
          return updated
        }),
      onRemoteVideoTiming: (userId, timing, source) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          if (source === 'screenshare') {
            updated.set(userId, {
              ...current,
              screenShareReceiveToDecodeMs: timing.receiveToDecodeMs,
              screenShareReceiveToRenderMs: timing.receiveToRenderMs
            })
            return updated
          }
          updated.set(userId, {
            ...current,
            videoReceiveToDecodeMs: timing.receiveToDecodeMs,
            videoReceiveToRenderMs: timing.receiveToRenderMs
          })
          return updated
        }),
      onRemoteVideoDecodingObject: (userId, decoding, source) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          if (source === 'screenshare') {
            updated.set(userId, {
              ...current,
              screenShareDecodingGroupId: decoding.groupId,
              screenShareDecodingObjectId: decoding.objectId,
              screenShareDecodingChunkType: decoding.chunkType,
              screenShareDecodingPhase: decoding.phase
            })
            return updated
          }
          updated.set(userId, {
            ...current,
            videoDecodingGroupId: decoding.groupId,
            videoDecodingObjectId: decoding.objectId,
            videoDecodingChunkType: decoding.chunkType,
            videoDecodingPhase: decoding.phase
          })
          return updated
        }),
      onRemoteVideoPacing: (userId, pacing, source) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          if (source === 'screenshare') {
            updated.set(userId, {
              ...current,
              screenSharePacingIntervalMs: pacing.intervalMs,
              screenSharePacingEffectiveIntervalMs: pacing.effectiveIntervalMs,
              screenSharePacingBufferedFrames: pacing.bufferedFrames,
              screenShareDecodeQueueSize: pacing.decodeQueueSize,
              screenSharePacingTargetFrames: pacing.targetFrames
            })
            return updated
          }
          updated.set(userId, {
            ...current,
            videoPacingIntervalMs: pacing.intervalMs,
            videoPacingEffectiveIntervalMs: pacing.effectiveIntervalMs,
            videoPacingBufferedFrames: pacing.bufferedFrames,
            videoDecodeQueueSize: pacing.decodeQueueSize,
            videoPacingTargetFrames: pacing.targetFrames
          })
          return updated
        }),
      onRemoteAudioReceiveLatency: (userId, ms) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          updated.set(userId, {
            ...current,
            audioLatencyReceiveMs: ms
          })
          return updated
        }),
      onRemoteAudioRenderingLatency: (userId, ms) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          const renderingRate = updateRenderingRate(audioRenderingRateStateRef.current, userId)
          updated.set(userId, {
            ...current,
            audioLatencyRenderMs: ms,
            audioRenderingRateFps: renderingRate
          })
          return updated
        }),
      onRemoteAudioPlaybackQueue: (userId, queuedMs) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          updated.set(userId, {
            ...current,
            audioPlaybackQueueMs: queuedMs
          })
          return updated
        }),
      onRemoteVideoJitterBufferActivity: (userId, activity, source) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          const nextSnapshot = createJitterBufferSnapshot(
            source === 'screenshare' ? current.screenShareJitterBuffer : current.videoJitterBuffer,
            activity.event,
            activity.bufferedFrames,
            activity.capacityFrames
          )
          if (source === 'screenshare') {
            updated.set(userId, {
              ...current,
              screenShareJitterBuffer: nextSnapshot
            })
            return updated
          }
          updated.set(userId, {
            ...current,
            videoJitterBuffer: nextSnapshot
          })
          return updated
        }),
      onRemoteAudioJitterBufferActivity: (userId, activity) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          updated.set(userId, {
            ...current,
            audioJitterBuffer: createJitterBufferSnapshot(
              current.audioJitterBuffer,
              activity.event,
              activity.bufferedFrames,
              activity.capacityFrames
            )
          })
          return updated
        }),
      onRemoteVideoConfig: (userId, config, source) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          if (source === 'screenshare') {
            updated.set(userId, {
              ...current,
              screenShareCodec: config.codec ?? current.screenShareCodec,
              screenShareWidth: config.width ?? current.screenShareWidth,
              screenShareHeight: config.height ?? current.screenShareHeight,
              screenShareDecoderDescriptionLength:
                config.descriptionLength ?? current.screenShareDecoderDescriptionLength,
              screenShareDecoderAvcFormat: config.avcFormat ?? current.screenShareDecoderAvcFormat,
              screenShareDecoderHardwareAcceleration:
                config.hardwareAcceleration ?? current.screenShareDecoderHardwareAcceleration,
              screenShareDecoderOptimizeForLatency:
                config.optimizeForLatency ?? current.screenShareDecoderOptimizeForLatency
            })
            return updated
          }
          updated.set(userId, {
            ...current,
            videoCodec: config.codec ?? current.videoCodec,
            videoWidth: config.width ?? current.videoWidth,
            videoHeight: config.height ?? current.videoHeight,
            videoDecoderDescriptionLength: config.descriptionLength ?? current.videoDecoderDescriptionLength,
            videoDecoderAvcFormat: config.avcFormat ?? current.videoDecoderAvcFormat,
            videoDecoderHardwareAcceleration: config.hardwareAcceleration ?? current.videoDecoderHardwareAcceleration,
            videoDecoderOptimizeForLatency: config.optimizeForLatency ?? current.videoDecoderOptimizeForLatency
          })
          return updated
        }),
      onVideoEncodeError: (message) => setVideoEncoderError(message),
      onAudioEncodeError: (message) => setAudioEncoderError(message),
      onAudioEncodingAdjusted: (settings) => setSelectedAudioEncoding(settings),
      onScreenShareEncodingApplied: (settings) => setSelectedScreenShareEncoding(settings)
    })

    return () => {
      controller.setHandlers({})
      setRemoteMedia(new Map())
      setLocalVideoBitrate(null)
      setLocalAudioBitrate(null)
      setLocalCameraVideoSendTiming(null)
      setLocalScreenShareVideoSendTiming(null)
      setLocalVideoStream(null)
      setLocalScreenShareStream(null)
      setLocalAudioStream(null)
      setCameraEnabled(false)
      setScreenShareEnabled(false)
      setMicrophoneEnabled(session.localMember.publishedTracks.audio)
      setVideoJitterConfigs(new Map())
      setAudioJitterConfigs(new Map())
      setSubscribedCatalogTracks([])
      videoRenderingRateStateRef.current.clear()
      audioRenderingRateStateRef.current.clear()
    }
  }, [session])

  useEffect(() => {
    if (!session) {
      setSubscribedCatalogTracks([])
      return
    }
    const controller = session.getMediaController()
    const syncSubscribedCatalogTracks = () => {
      const nextTracks = controller.getSubscribedCatalogTracks()
      setSubscribedCatalogTracks((prev) => (isSameSubscribedCatalogTracks(prev, nextTracks) ? prev : nextTracks))
    }

    syncSubscribedCatalogTracks()
    const timerId = window.setInterval(syncSubscribedCatalogTracks, 500)
    return () => window.clearInterval(timerId)
  }, [session])

  useEffect(() => {
    refreshDevices().catch((err) => console.error('Failed to enumerate devices', err))
    const handler = () => refreshDevices()
    navigator.mediaDevices?.addEventListener('devicechange', handler)
    return () => {
      navigator.mediaDevices?.removeEventListener('devicechange', handler)
    }
  }, [refreshDevices])

  useEffect(() => {
    if (!session) {
      return
    }
    const controller = session.getMediaController()
    controller
      .setVideoEncodingSettings(selectedVideoEncoding, selectedVideoDeviceId ?? undefined, false)
      .catch((err) => console.error('Failed to apply initial video encoding config', err))
  }, [selectedVideoEncoding, selectedVideoDeviceId, session])

  useEffect(() => {
    if (!session) {
      return
    }
    const controller = session.getMediaController()
    controller
      .setAudioEncodingSettings(selectedAudioEncoding, false)
      .catch((err) => console.error('Failed to apply initial audio encoding config', err))
  }, [selectedAudioEncoding, session])

  useEffect(() => {
    if (!session) {
      return
    }
    const controller = session.getMediaController()
    controller.setVideoCaptureConstraints({
      frameRate: captureSettings.frameRate,
      width: captureSettings.width,
      height: captureSettings.height
    })
    controller.setAudioCaptureConstraints({
      echoCancellation: captureSettings.echoCancellation,
      noiseSuppression: captureSettings.noiseSuppression,
      autoGainControl: captureSettings.autoGainControl
    })
  }, [
    captureSettings.autoGainControl,
    captureSettings.echoCancellation,
    captureSettings.frameRate,
    captureSettings.height,
    captureSettings.noiseSuppression,
    captureSettings.width,
    session
  ])

  const persistCatalogTracks = useCallback(
    async (nextTracks: EditableCallCatalogTrack[]) => {
      if (!session) {
        return
      }
      const controller = session.getMediaController()
      await controller.setCatalogTracks(toCatalogTracks(nextTracks))
      const nextVideoEncoding = deriveCameraEncodingFromCatalogTracks(nextTracks, selectedVideoEncoding)
      const nextScreenShareEncoding = deriveScreenShareEncodingFromCatalogTracks(
        nextTracks,
        selectedScreenShareEncoding
      )
      const nextAudioEncoding = deriveAudioEncodingFromCatalogTracks(nextTracks, selectedAudioEncoding)
      setSelectedVideoEncoding(nextVideoEncoding)
      setSelectedScreenShareEncoding(nextScreenShareEncoding)
      setSelectedAudioEncoding(nextAudioEncoding)
      await controller.setVideoEncodingSettings(nextVideoEncoding, selectedVideoDeviceId ?? undefined, false)
      await controller.setScreenShareEncodingSettings(nextScreenShareEncoding)
      await controller.setAudioEncodingSettings(nextAudioEncoding, false)
    },
    [selectedAudioEncoding, selectedScreenShareEncoding, selectedVideoDeviceId, selectedVideoEncoding, session]
  )

  const updateCatalogTracks = useCallback(
    (updater: (prev: EditableCallCatalogTrack[]) => EditableCallCatalogTrack[], actionLabel: string) => {
      setCatalogTracks((prev) => {
        const next = updater(prev)
        if (next === prev) {
          return prev
        }
        void persistCatalogTracks(next).catch((error) => {
          console.error(`Failed to ${actionLabel}:`, error)
        })
        return next
      })
    },
    [persistCatalogTracks]
  )

  const ensureCatalogPresetTracks = useCallback(
    (source: CatalogPresetSource) => {
      const preset = CATALOG_PRESETS[source]
      updateCatalogTracks((prev) => {
        if (hasCatalogTrackForSource(prev, source)) {
          return prev
        }
        return appendCatalogTracks(prev, preset.append())
      }, `ensure ${preset.label} catalog tracks`)
    },
    [updateCatalogTracks]
  )

  const removeCatalogPresetTracks = useCallback(
    (source: CatalogPresetSource) => {
      const preset = CATALOG_PRESETS[source]
      updateCatalogTracks(
        (prev) => removeCatalogTracksByNames(prev, preset.names()),
        `remove ${preset.label} catalog tracks`
      )
    },
    [updateCatalogTracks]
  )

  const toggleCamera = useCallback(async () => {
    if (!session || cameraBusy) {
      return cameraEnabled
    }
    const controller = session.getMediaController()
    setCameraBusy(true)
    const nextState = !cameraEnabled
    try {
      if (nextState) {
        await controller.startCamera(selectedVideoDeviceId ?? undefined, {
          frameRate: captureSettings.frameRate,
          width: captureSettings.width,
          height: captureSettings.height
        })
        ensureCatalogPresetTracks('camera')
      } else {
        await controller.stopCamera()
        removeCatalogPresetTracks('camera')
      }
      setCameraEnabled(nextState)
      return nextState
    } catch (error) {
      console.error('Failed to toggle camera:', error)
      return cameraEnabled
    } finally {
      setCameraBusy(false)
    }
  }, [
    cameraBusy,
    cameraEnabled,
    captureSettings.frameRate,
    captureSettings.height,
    captureSettings.width,
    ensureCatalogPresetTracks,
    removeCatalogPresetTracks,
    session,
    selectedVideoDeviceId
  ])

  const toggleScreenShare = useCallback(async () => {
    if (!session || cameraBusy) {
      return screenShareEnabled
    }
    const controller = session.getMediaController()
    setCameraBusy(true)
    const nextState = !screenShareEnabled
    try {
      if (nextState) {
        await controller.startScreenShare()
        ensureCatalogPresetTracks('screenshare')
      } else {
        await controller.stopScreenShare()
        removeCatalogPresetTracks('screenshare')
      }
      setScreenShareEnabled(nextState)
      return nextState
    } catch (error) {
      console.error('Failed to toggle screen share:', error)
      return screenShareEnabled
    } finally {
      setCameraBusy(false)
    }
  }, [cameraBusy, ensureCatalogPresetTracks, removeCatalogPresetTracks, screenShareEnabled, session])

  const toggleMicrophone = useCallback(async () => {
    if (!session || microphoneBusy) {
      return microphoneEnabled
    }
    const controller = session.getMediaController()
    setMicrophoneBusy(true)
    const nextState = !microphoneEnabled
    try {
      if (nextState) {
        await controller.startMicrophone(selectedAudioDeviceId ?? undefined, {
          echoCancellation: captureSettings.echoCancellation,
          noiseSuppression: captureSettings.noiseSuppression,
          autoGainControl: captureSettings.autoGainControl
        })
        ensureCatalogPresetTracks('audio')
      } else {
        await controller.stopMicrophone()
        removeCatalogPresetTracks('audio')
      }
      setMicrophoneEnabled(nextState)
      return nextState
    } catch (error) {
      console.error('Failed to toggle microphone:', error)
      return microphoneEnabled
    } finally {
      setMicrophoneBusy(false)
    }
  }, [
    captureSettings.autoGainControl,
    captureSettings.echoCancellation,
    captureSettings.noiseSuppression,
    ensureCatalogPresetTracks,
    removeCatalogPresetTracks,
    microphoneBusy,
    microphoneEnabled,
    selectedAudioDeviceId,
    session
  ])

  const setVideoJitterBufferConfig = useCallback(
    (userId: string, config: Partial<VideoJitterConfig>) => {
      if (!session) {
        return
      }
      const controller = session.getMediaController()
      setVideoJitterConfigs((prev) => {
        const current = prev.get(userId) ?? DEFAULT_VIDEO_JITTER_CONFIG
        const next = normalizeVideoJitterConfig({ ...current, ...config })
        const updated = new Map(prev)
        updated.set(userId, next)
        controller.setVideoJitterBufferConfig(userId, next)
        return updated
      })
    },
    [session]
  )

  const setAudioJitterBufferConfig = useCallback(
    (userId: string, config: Partial<AudioJitterConfig>) => {
      if (!session) {
        return
      }
      const controller = session.getMediaController()
      setAudioJitterConfigs((prev) => {
        const current = prev.get(userId) ?? DEFAULT_AUDIO_JITTER_CONFIG
        const next = normalizeAudioJitterConfig({ ...current, ...config })
        const updated = new Map(prev)
        updated.set(userId, next)
        controller.setAudioJitterBufferConfig(userId, next)
        return updated
      })
    },
    [session]
  )

  const selectVideoDevice = useCallback(
    async (deviceId: string) => {
      setSelectedVideoDeviceId(deviceId)
      if (!session || !cameraEnabled) {
        return
      }
      const controller = session.getMediaController()
      setCameraBusy(true)
      try {
        await controller.stopCamera()
        await controller.startCamera(deviceId, {
          frameRate: captureSettings.frameRate,
          width: captureSettings.width,
          height: captureSettings.height
        })
        setCameraEnabled(true)
      } catch (err) {
        console.error('Failed to switch camera device', err)
      } finally {
        setCameraBusy(false)
      }
    },
    [cameraEnabled, captureSettings.frameRate, captureSettings.height, captureSettings.width, session]
  )

  const selectAudioDevice = useCallback(
    async (deviceId: string) => {
      setSelectedAudioDeviceId(deviceId)
      if (!session || !microphoneEnabled) {
        return
      }
      const controller = session.getMediaController()
      setMicrophoneBusy(true)
      try {
        await controller.stopMicrophone()
        await controller.startMicrophone(deviceId, {
          echoCancellation: captureSettings.echoCancellation,
          noiseSuppression: captureSettings.noiseSuppression,
          autoGainControl: captureSettings.autoGainControl
        })
        setMicrophoneEnabled(true)
      } catch (err) {
        console.error('Failed to switch audio device', err)
      } finally {
        setMicrophoneBusy(false)
      }
    },
    [
      captureSettings.autoGainControl,
      captureSettings.echoCancellation,
      captureSettings.noiseSuppression,
      microphoneEnabled,
      session
    ]
  )

  const selectVideoEncoding = useCallback(
    async (settings: Partial<VideoEncodingSettings>) => {
      const next: VideoEncodingSettings = {
        codec: settings.codec ?? selectedVideoEncoding.codec,
        width: settings.width ?? selectedVideoEncoding.width,
        height: settings.height ?? selectedVideoEncoding.height,
        bitrate: settings.bitrate ?? selectedVideoEncoding.bitrate,
        framerate: settings.framerate ?? selectedVideoEncoding.framerate,
        hardwareAcceleration: settings.hardwareAcceleration ?? selectedVideoEncoding.hardwareAcceleration
      }
      setSelectedVideoEncoding(next)
      setVideoEncoderError(null)
      if (!session) {
        return
      }
      const controller = session.getMediaController()
      await controller.setVideoEncodingSettings(next, selectedVideoDeviceId ?? undefined, cameraEnabled)
      if (cameraEnabled) {
        setCameraEnabled(true)
      }
    },
    [cameraEnabled, selectedVideoDeviceId, selectedVideoEncoding, session]
  )

  const selectScreenShareEncoding = useCallback(
    async (settings: Partial<VideoEncodingSettings>) => {
      const next: VideoEncodingSettings = {
        codec: settings.codec ?? selectedScreenShareEncoding.codec,
        width: settings.width ?? selectedScreenShareEncoding.width,
        height: settings.height ?? selectedScreenShareEncoding.height,
        bitrate: settings.bitrate ?? selectedScreenShareEncoding.bitrate,
        framerate: settings.framerate ?? selectedScreenShareEncoding.framerate,
        hardwareAcceleration: settings.hardwareAcceleration ?? selectedScreenShareEncoding.hardwareAcceleration
      }
      setSelectedScreenShareEncoding(next)
      setVideoEncoderError(null)
      if (!session) {
        return
      }
      const controller = session.getMediaController()
      await controller.setScreenShareEncodingSettings(next)
    },
    [selectedScreenShareEncoding, session]
  )

  const selectAudioEncoding = useCallback(
    async (settings: Partial<AudioEncodingSettings>) => {
      const next: AudioEncodingSettings = {
        codec: settings.codec ?? selectedAudioEncoding.codec,
        bitrate: settings.bitrate ?? selectedAudioEncoding.bitrate,
        channels: settings.channels ?? selectedAudioEncoding.channels
      }
      setSelectedAudioEncoding(next)
      setAudioEncoderError(null)
      if (!session) {
        return
      }
      const controller = session.getMediaController()
      await controller.setAudioEncodingSettings(next, microphoneEnabled)
      if (microphoneEnabled) {
        setMicrophoneEnabled(true)
      }
    },
    [microphoneEnabled, selectedAudioEncoding, session]
  )

  const updateCaptureSettings = useCallback((settings: Partial<CaptureSettingsState>) => {
    setCaptureSettings((prev) => ({ ...prev, ...settings }))
  }, [])

  const applyCaptureSettings = useCallback(async () => {
    if (!session) {
      return
    }
    const controller = session.getMediaController()
    setCameraBusy(true)
    setMicrophoneBusy(true)
    try {
      if (captureSettings.videoEnabled) {
        const isCameraEnabling = !cameraEnabled
        await controller.stopCamera()
        await controller.startCamera(selectedVideoDeviceId ?? undefined, {
          frameRate: captureSettings.frameRate,
          width: captureSettings.width,
          height: captureSettings.height
        })
        await controller.setVideoEncodingSettings(selectedVideoEncoding, selectedVideoDeviceId ?? undefined, true)
        if (isCameraEnabling) {
          ensureCatalogPresetTracks('camera')
        }
        setCameraEnabled(true)
      } else if (cameraEnabled) {
        await controller.stopCamera()
        removeCatalogPresetTracks('camera')
        setCameraEnabled(false)
      }

      if (captureSettings.audioEnabled) {
        const isMicrophoneEnabling = !microphoneEnabled
        await controller.startMicrophone(selectedAudioDeviceId ?? undefined, {
          echoCancellation: captureSettings.echoCancellation,
          noiseSuppression: captureSettings.noiseSuppression,
          autoGainControl: captureSettings.autoGainControl
        })
        await controller.setAudioEncodingSettings(selectedAudioEncoding, true)
        if (isMicrophoneEnabling) {
          ensureCatalogPresetTracks('audio')
        }
        setMicrophoneEnabled(true)
      } else if (microphoneEnabled) {
        await controller.stopMicrophone()
        removeCatalogPresetTracks('audio')
        setMicrophoneEnabled(false)
      }
    } catch (err) {
      console.error('Failed to apply getUserMedia settings', err)
    } finally {
      setCameraBusy(false)
      setMicrophoneBusy(false)
    }
  }, [
    cameraEnabled,
    captureSettings.audioEnabled,
    captureSettings.autoGainControl,
    captureSettings.echoCancellation,
    captureSettings.frameRate,
    captureSettings.noiseSuppression,
    captureSettings.videoEnabled,
    ensureCatalogPresetTracks,
    removeCatalogPresetTracks,
    microphoneEnabled,
    selectedAudioEncoding,
    selectedAudioDeviceId,
    selectedVideoEncoding,
    selectedVideoDeviceId,
    session
  ])

  const addCatalogTrack = useCallback(
    (track: Omit<EditableCallCatalogTrack, 'id'>) => {
      updateCatalogTracks((prev) => [...prev, { ...track, id: createCatalogTrackId() }], 'add catalog track')
    },
    [updateCatalogTracks]
  )

  const updateCatalogTrack = useCallback(
    (id: string, patch: Partial<EditableCallCatalogTrack>) => {
      updateCatalogTracks(
        (prev) => prev.map((track) => (track.id === id ? { ...track, ...patch } : track)),
        'update catalog track'
      )
    },
    [updateCatalogTracks]
  )

  const removeCatalogTrack = useCallback(
    (id: string) => {
      updateCatalogTracks((prev) => prev.filter((track) => track.id !== id), 'remove catalog track')
    },
    [updateCatalogTracks]
  )

  return {
    cameraEnabled,
    screenShareEnabled,
    microphoneEnabled,
    cameraBusy,
    microphoneBusy,
    localVideoStream,
    localScreenShareStream,
    localAudioStream,
    localVideoBitrate,
    localAudioBitrate,
    localCameraVideoSendTiming,
    localScreenShareVideoSendTiming,
    remoteMedia,
    toggleCamera,
    toggleScreenShare,
    toggleMicrophone,
    videoJitterConfigs,
    setVideoJitterBufferConfig,
    audioJitterConfigs,
    setAudioJitterBufferConfig,
    videoCodecOptions: VIDEO_CODEC_OPTIONS,
    videoResolutionOptions: VIDEO_RESOLUTION_OPTIONS,
    videoBitrateOptions: VIDEO_BITRATE_OPTIONS,
    videoHardwareAccelerationOptions: VIDEO_HARDWARE_ACCELERATION_OPTIONS,
    selectedVideoEncoding,
    selectVideoEncoding,
    selectedScreenShareEncoding,
    selectScreenShareEncoding,
    audioCodecOptions: AUDIO_CODEC_OPTIONS,
    audioBitrateOptions: AUDIO_BITRATE_OPTIONS,
    audioChannelOptions: AUDIO_CHANNEL_OPTIONS,
    selectedAudioEncoding,
    selectAudioEncoding,
    videoEncoderError,
    audioEncoderError,
    videoDevices,
    audioDevices,
    selectedVideoDeviceId,
    selectedAudioDeviceId,
    selectVideoDevice,
    selectAudioDevice,
    captureSettings,
    updateCaptureSettings,
    applyCaptureSettings,
    catalogTracks,
    subscribedCatalogTracks,
    addCatalogTrack,
    updateCatalogTrack,
    removeCatalogTrack
  }
}

function isSameSubscribedCatalogTracks(left: SubscribedCatalogTrack[], right: SubscribedCatalogTrack[]): boolean {
  if (left.length !== right.length) {
    return false
  }
  for (let index = 0; index < left.length; index += 1) {
    const l = left[index]
    const r = right[index]
    if (
      l.name !== r.name ||
      l.subscriberCount !== r.subscriberCount ||
      l.label !== r.label ||
      l.role !== r.role ||
      l.codec !== r.codec ||
      l.bitrate !== r.bitrate ||
      l.width !== r.width ||
      l.height !== r.height ||
      l.framerate !== r.framerate ||
      l.hardwareAcceleration !== r.hardwareAcceleration ||
      l.keyframeInterval !== r.keyframeInterval ||
      l.samplerate !== r.samplerate ||
      l.channelConfig !== r.channelConfig ||
      l.audioStreamUpdateMode !== r.audioStreamUpdateMode ||
      l.audioStreamUpdateIntervalSeconds !== r.audioStreamUpdateIntervalSeconds ||
      l.isLive !== r.isLive
    ) {
      return false
    }
  }
  return true
}

function buildVideoRenderingRateKey(userId: string, source: 'camera' | 'screenshare'): string {
  return `${userId}:${source}`
}

function updateRenderingRate(states: Map<string, RenderingRateState>, key: string): number | undefined {
  const now = performance.now()
  const previous = states.get(key)
  if (!previous) {
    states.set(key, { lastEventAtMs: now, smoothedFps: 0 })
    return undefined
  }

  const intervalMs = now - previous.lastEventAtMs
  if (!Number.isFinite(intervalMs) || intervalMs < MIN_RENDERING_INTERVAL_MS) {
    states.set(key, { ...previous, lastEventAtMs: now })
    return previous.smoothedFps > 0 ? previous.smoothedFps : undefined
  }

  const instantaneousFps = Math.min(MAX_RENDERING_FPS, 1000 / intervalMs)
  const nextFps =
    previous.smoothedFps > 0
      ? previous.smoothedFps * (1 - RENDERING_RATE_SMOOTHING_FACTOR) +
        instantaneousFps * RENDERING_RATE_SMOOTHING_FACTOR
      : instantaneousFps
  states.set(key, { lastEventAtMs: now, smoothedFps: nextFps })
  return nextFps
}

function createJitterBufferSnapshot(
  current: JitterBufferSnapshot | undefined,
  event: JitterBufferEvent,
  bufferedFrames: number,
  capacityFrames: number
): JitterBufferSnapshot {
  return {
    bufferedFrames: Math.max(0, Math.floor(bufferedFrames)),
    capacityFrames: Math.max(1, Math.floor(capacityFrames)),
    lastEvent: event,
    sequence: (current?.sequence ?? 0) + 1,
    updatedAtMs: Date.now()
  }
}

function deriveCameraEncodingFromCatalogTracks(
  tracks: EditableCallCatalogTrack[],
  fallback: VideoEncodingSettings
): VideoEncodingSettings {
  return deriveVideoEncodingFromCatalogTracks(tracks, fallback, 'camera')
}

function deriveScreenShareEncodingFromCatalogTracks(
  tracks: EditableCallCatalogTrack[],
  fallback: VideoEncodingSettings
): VideoEncodingSettings {
  return deriveVideoEncodingFromCatalogTracks(tracks, fallback, 'screenshare')
}

function hasCatalogTrackForSource(tracks: EditableCallCatalogTrack[], source: CatalogPresetSource): boolean {
  if (source === 'audio') {
    return tracks.some((track) => track.role === 'audio')
  }
  const isScreenshare = source === 'screenshare'
  return tracks.some((track) => track.role === 'video' && isScreenShareTrackName(track.name) === isScreenshare)
}

function deriveVideoEncodingFromCatalogTracks(
  tracks: EditableCallCatalogTrack[],
  fallback: VideoEncodingSettings,
  source: 'camera' | 'screenshare'
): VideoEncodingSettings {
  const videoTracks = tracks.filter(
    (track) => track.role === 'video' && isScreenShareTrackName(track.name) === (source === 'screenshare')
  )
  const videoTrack = pickVideoTrackByPriority(videoTracks)
  if (!videoTrack) {
    return fallback
  }
  return {
    codec: normalizeNonEmptyString(videoTrack.codec) ?? fallback.codec,
    width: normalizePositiveNumber(videoTrack.width) ?? fallback.width,
    height: normalizePositiveNumber(videoTrack.height) ?? fallback.height,
    bitrate: normalizePositiveNumber(videoTrack.bitrate) ?? fallback.bitrate,
    framerate: normalizePositiveNumber(videoTrack.framerate) ?? fallback.framerate,
    hardwareAcceleration:
      normalizeHardwareAcceleration(videoTrack.hardwareAcceleration) ?? fallback.hardwareAcceleration
  }
}

function pickVideoTrackByPriority(tracks: EditableCallCatalogTrack[]): EditableCallCatalogTrack | undefined {
  const tracksWithBitrate = tracks
    .filter((track) => typeof track.bitrate === 'number')
    .sort((a, b) => (b.bitrate ?? 0) - (a.bitrate ?? 0))
  return tracksWithBitrate[0] ?? tracks[0]
}

function deriveAudioEncodingFromCatalogTracks(
  tracks: EditableCallCatalogTrack[],
  fallback: AudioEncodingSettings
): AudioEncodingSettings {
  const audioTrack = tracks.find((track) => track.role === 'audio')
  if (!audioTrack) {
    return fallback
  }
  return {
    codec: normalizeNonEmptyString(audioTrack.codec) ?? fallback.codec,
    bitrate: normalizePositiveNumber(audioTrack.bitrate) ?? fallback.bitrate,
    channels: parseAudioChannels(audioTrack.channelConfig) ?? fallback.channels
  }
}

function normalizePositiveNumber(value: number | undefined): number | undefined {
  if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) {
    return undefined
  }
  return Math.floor(value)
}

function normalizeNonEmptyString(value: string | undefined): string | undefined {
  if (typeof value !== 'string') {
    return undefined
  }
  const trimmed = value.trim()
  return trimmed.length > 0 ? trimmed : undefined
}

function normalizeHardwareAcceleration(value: HardwareAcceleration | undefined): HardwareAcceleration | undefined {
  return value === 'prefer-hardware' || value === 'prefer-software' || value === 'no-preference' ? value : undefined
}

function parseAudioChannels(channelConfig: string | undefined): number | undefined {
  const normalized = normalizeNonEmptyString(channelConfig)?.toLowerCase()
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
