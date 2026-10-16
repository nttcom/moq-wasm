import { useCallback, useEffect, useState } from 'react'
import { LocalSession } from '../session/localSession'
import type { RemotePictures } from '../media/mediaSubscriber'
import type { PlayoutSettings } from '../types/playout'
import {
  DEFAULT_SCREEN_SHARE_ENCODING_SETTINGS,
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
import type { EditableMeetingCatalogTrack, MeetingCatalogTrack } from '../types/catalog'
import { isScreenShareTrackName } from '../utils/catalogTrackName'
import {
  E2E_CAMERA_SETTINGS,
  appendCatalogTracks,
  createCatalogTrackId,
  getAudioCatalogTracks,
  getCameraCatalogTracks,
  getScreenShareCatalogTracks,
  pickHighestBitrateTrack,
  removeCatalogTracksByNames,
  toCatalogTracks,
  toEditableCatalogTracks,
  trackAudioEncoding,
  trackVideoEncoding
} from '../media/meetingCatalog'
import type { SubscribedCatalogTrack } from '../media/mediaPublisher'
import type { VideoSendTiming } from '../types/stats'
import { DominantSpeaker } from '../media/dominantSpeaker'

const SPEAKER_UPDATE_INTERVAL_MS = 250

function sameMembers(a: Set<string>, b: Set<string>): boolean {
  return a.size === b.size && [...a].every((member) => b.has(member))
}

interface UseMeetingMediaResult {
  /// The remote member to focus, from the LOC Audio Level of their audio.
  activeSpeakerId: string | undefined
  speakingMemberIds: Set<string>
  cameraEnabled: boolean
  screenShareEnabled: boolean
  microphoneEnabled: boolean
  cameraBusy: boolean
  microphoneBusy: boolean
  localVideoStream: MediaStream | null
  localScreenShareStream: MediaStream | null
  localAudioStream: MediaStream | null
  localCameraBitrate: number | null
  localScreenShareBitrate: number | null
  localAudioBitrate: number | null
  localCameraVideoSendTiming: VideoSendTiming | null
  localScreenShareVideoSendTiming: VideoSendTiming | null
  remotePictures: Map<string, RemotePictures>
  toggleCamera: () => Promise<boolean>
  toggleScreenShare: () => Promise<boolean>
  toggleMicrophone: () => Promise<boolean>
  playoutSettings: Map<string, PlayoutSettings>
  setPlayoutSettings: (userId: string, settings: PlayoutSettings) => void
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
  catalogTracks: EditableMeetingCatalogTrack[]
  subscribedCatalogTracks: SubscribedCatalogTrack[]
  addCatalogTrack: (track: Omit<EditableMeetingCatalogTrack, 'id'>) => void
  updateCatalogTrack: (id: string, patch: Partial<EditableMeetingCatalogTrack>) => void
  removeCatalogTrack: (id: string) => void
}

const INITIAL_CAMERA_ENCODING_SETTINGS: VideoEncodingSettings = E2E_CAMERA_SETTINGS
  ? {
      ...DEFAULT_VIDEO_ENCODING_SETTINGS,
      width: E2E_CAMERA_SETTINGS.width,
      height: E2E_CAMERA_SETTINGS.height,
      framerate: E2E_CAMERA_SETTINGS.framerate,
      bitrate: E2E_CAMERA_SETTINGS.bitrate,
      ...(E2E_CAMERA_SETTINGS.codec ? { codec: E2E_CAMERA_SETTINGS.codec } : {})
    }
  : DEFAULT_VIDEO_ENCODING_SETTINGS

type CatalogPresetSource = 'camera' | 'screenshare' | 'audio'
const CATALOG_PRESETS: Record<CatalogPresetSource, () => MeetingCatalogTrack[]> = {
  camera: getCameraCatalogTracks,
  screenshare: getScreenShareCatalogTracks,
  audio: getAudioCatalogTracks
}

export function useMeetingMedia(session: LocalSession | null): UseMeetingMediaResult {
  const [cameraEnabled, setCameraEnabled] = useState(false)
  const [screenShareEnabled, setScreenShareEnabled] = useState(false)
  const [microphoneEnabled, setMicrophoneEnabled] = useState(false)
  const [cameraBusy, setCameraBusy] = useState(false)
  const [microphoneBusy, setMicrophoneBusy] = useState(false)
  const [localVideoStream, setLocalVideoStream] = useState<MediaStream | null>(null)
  const [localScreenShareStream, setLocalScreenShareStream] = useState<MediaStream | null>(null)
  const [localAudioStream, setLocalAudioStream] = useState<MediaStream | null>(null)
  const [localCameraBitrate, setLocalCameraBitrate] = useState<number | null>(null)
  const [localScreenShareBitrate, setLocalScreenShareBitrate] = useState<number | null>(null)
  const [localAudioBitrate, setLocalAudioBitrate] = useState<number | null>(null)
  const [localCameraVideoSendTiming, setLocalCameraVideoSendTiming] = useState<VideoSendTiming | null>(null)
  const [localScreenShareVideoSendTiming, setLocalScreenShareVideoSendTiming] = useState<VideoSendTiming | null>(null)
  const [remotePictures, setRemotePictures] = useState<Map<string, RemotePictures>>(new Map())
  const [activeSpeakerId, setActiveSpeakerId] = useState<string | undefined>(undefined)
  const [speakingMemberIds, setSpeakingMemberIds] = useState<Set<string>>(new Set())
  const [playoutSettings, setPlayoutSettingsState] = useState<Map<string, PlayoutSettings>>(new Map())
  const [videoDevices, setVideoDevices] = useState<MediaDeviceInfo[]>([])
  const [audioDevices, setAudioDevices] = useState<MediaDeviceInfo[]>([])
  const [selectedVideoDeviceId, setSelectedVideoDeviceId] = useState<string | null>(null)
  const [selectedAudioDeviceId, setSelectedAudioDeviceId] = useState<string | null>(null)
  const [selectedVideoEncoding, setSelectedVideoEncoding] = useState<VideoEncodingSettings>(
    INITIAL_CAMERA_ENCODING_SETTINGS
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
    width: E2E_CAMERA_SETTINGS?.width ?? DEFAULT_VIDEO_ENCODING_SETTINGS.width,
    height: E2E_CAMERA_SETTINGS?.height ?? DEFAULT_VIDEO_ENCODING_SETTINGS.height,
    frameRate: E2E_CAMERA_SETTINGS?.framerate ?? 30,
    echoCancellation: true,
    noiseSuppression: true,
    autoGainControl: true
  })
  const [catalogTracks, setCatalogTracks] = useState<EditableMeetingCatalogTrack[]>([])
  const [subscribedCatalogTracks, setSubscribedCatalogTracks] = useState<SubscribedCatalogTrack[]>([])

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
      setRemotePictures(new Map())
      setCameraEnabled(false)
      setMicrophoneEnabled(false)
      setScreenShareEnabled(false)
      setPlayoutSettingsState(new Map())
      setVideoEncoderError(null)
      setAudioEncoderError(null)
      setCatalogTracks([])
      setSubscribedCatalogTracks([])
      return
    }

    const { publisher, subscriber } = session.getMediaController()
    const initialCatalogTracks = publisher.getCatalogTracks()
    setSubscribedCatalogTracks(publisher.getSubscribedCatalogTracks())
    setCatalogTracks(toEditableCatalogTracks(initialCatalogTracks))
    const initialVideoEncoding = deriveVideoEncoding(initialCatalogTracks, INITIAL_CAMERA_ENCODING_SETTINGS, 'camera')
    const initialScreenShareEncoding = deriveVideoEncoding(
      initialCatalogTracks,
      DEFAULT_SCREEN_SHARE_ENCODING_SETTINGS,
      'screenshare'
    )
    const initialAudioEncoding = deriveAudioEncoding(initialCatalogTracks, DEFAULT_AUDIO_ENCODING_SETTINGS)
    setSelectedVideoEncoding(initialVideoEncoding)
    setSelectedScreenShareEncoding(initialScreenShareEncoding)
    setSelectedAudioEncoding(initialAudioEncoding)
    void publisher.setVideoEncodingSettings(initialVideoEncoding, undefined, false).catch((error) => {
      console.error('Failed to apply video encoding from catalog:', error)
    })
    void publisher.setScreenShareEncodingSettings(initialScreenShareEncoding).catch((error) => {
      console.error('Failed to apply screen share encoding from catalog:', error)
    })
    void publisher.setAudioEncodingSettings(initialAudioEncoding, false).catch((error) => {
      console.error('Failed to apply audio encoding from catalog:', error)
    })
    publisher.setHandlers({
      onLocalCameraStream: (stream) => {
        setLocalVideoStream(stream)
        setCameraEnabled(Boolean(stream))
      },
      onLocalScreenShareStream: (stream) => {
        setLocalScreenShareStream(stream)
        setScreenShareEnabled(Boolean(stream))
      },
      onLocalAudioStream: (stream) => {
        setLocalAudioStream(stream)
        setMicrophoneEnabled(!!stream)
      },
      onEncodedVideoBitrate: (kbps, source) => {
        if (source === 'screenshare') {
          setLocalScreenShareBitrate(kbps)
          return
        }
        setLocalCameraBitrate(kbps)
      },
      onEncodedAudioBitrate: (kbps) => setLocalAudioBitrate(kbps),
      onLocalVideoSendTiming: (timing, source) => {
        if (source === 'screenshare') {
          setLocalScreenShareVideoSendTiming(timing)
          return
        }
        setLocalCameraVideoSendTiming(timing)
      },
      onVideoEncodeError: (message) => setVideoEncoderError(message),
      onAudioEncodeError: (message) => setAudioEncoderError(message),
      onAudioEncodingAdjusted: (settings) => setSelectedAudioEncoding(settings),
      onScreenShareEncodingApplied: (settings) => setSelectedScreenShareEncoding(settings)
    })
    const dominantSpeaker = new DominantSpeaker()
    subscriber.setHandlers({
      onRemotePicture: (userId, source, picture) =>
        setRemotePictures((prev) => {
          const updated = new Map(prev)
          updated.set(userId, { ...updated.get(userId), [source]: picture })
          return updated
        }),
      onRemotePictureClosed: (userId, source) =>
        setRemotePictures((prev) => {
          const current = prev.get(userId)
          if (!current?.[source]) {
            return prev
          }
          const updated = new Map(prev)
          const { [source]: _closed, ...rest } = current
          updated.set(userId, rest)
          return updated
        }),
      onAudioLevel: (userId, audioLevel) => dominantSpeaker.observe(userId, audioLevel, performance.now())
    })
    const speakerTimer = window.setInterval(() => {
      const nowMs = performance.now()
      const current = dominantSpeaker.current(nowMs)
      setActiveSpeakerId((previous) => (previous === current ? previous : current))
      const speaking = dominantSpeaker.speaking(nowMs)
      setSpeakingMemberIds((previous) => (sameMembers(previous, speaking) ? previous : speaking))
    }, SPEAKER_UPDATE_INTERVAL_MS)

    return () => {
      window.clearInterval(speakerTimer)
      publisher.setHandlers({})
      subscriber.setHandlers({})
      setRemotePictures(new Map())
      setLocalCameraBitrate(null)
      setLocalScreenShareBitrate(null)
      setLocalAudioBitrate(null)
      setLocalCameraVideoSendTiming(null)
      setLocalScreenShareVideoSendTiming(null)
      setLocalVideoStream(null)
      setLocalScreenShareStream(null)
      setLocalAudioStream(null)
      setCameraEnabled(false)
      setScreenShareEnabled(false)
      setMicrophoneEnabled(false)
      setPlayoutSettingsState(new Map())
      setSubscribedCatalogTracks([])
    }
  }, [session])

  useEffect(() => {
    if (!session) {
      setSubscribedCatalogTracks([])
      return
    }
    const { publisher } = session.getMediaController()
    const syncSubscribedCatalogTracks = () => {
      const nextTracks = publisher.getSubscribedCatalogTracks()
      setSubscribedCatalogTracks((prev) => (JSON.stringify(prev) === JSON.stringify(nextTracks) ? prev : nextTracks))
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
    const { publisher } = session.getMediaController()
    publisher
      .setVideoEncodingSettings(selectedVideoEncoding, selectedVideoDeviceId ?? undefined, false)
      .catch((err) => console.error('Failed to apply initial video encoding config', err))
  }, [selectedVideoEncoding, selectedVideoDeviceId, session])

  useEffect(() => {
    if (!session) {
      return
    }
    const { publisher } = session.getMediaController()
    publisher
      .setAudioEncodingSettings(selectedAudioEncoding, false)
      .catch((err) => console.error('Failed to apply initial audio encoding config', err))
  }, [selectedAudioEncoding, session])

  useEffect(() => {
    if (!session) {
      return
    }
    const { publisher } = session.getMediaController()
    publisher.setVideoCaptureConstraints({
      frameRate: captureSettings.frameRate,
      width: captureSettings.width,
      height: captureSettings.height
    })
    publisher.setAudioCaptureConstraints({
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
    async (nextTracks: EditableMeetingCatalogTrack[]) => {
      if (!session) {
        return
      }
      const { publisher } = session.getMediaController()
      const nextCatalogTracks = toCatalogTracks(nextTracks)
      await publisher.setCatalogTracks(nextCatalogTracks)
      const nextVideoEncoding = deriveVideoEncoding(nextCatalogTracks, selectedVideoEncoding, 'camera')
      const nextScreenShareEncoding = deriveVideoEncoding(nextCatalogTracks, selectedScreenShareEncoding, 'screenshare')
      const nextAudioEncoding = deriveAudioEncoding(nextCatalogTracks, selectedAudioEncoding)
      setSelectedVideoEncoding(nextVideoEncoding)
      setSelectedScreenShareEncoding(nextScreenShareEncoding)
      setSelectedAudioEncoding(nextAudioEncoding)
      await publisher.setVideoEncodingSettings(nextVideoEncoding, selectedVideoDeviceId ?? undefined, false)
      await publisher.setScreenShareEncodingSettings(nextScreenShareEncoding)
      await publisher.setAudioEncodingSettings(nextAudioEncoding, false)
    },
    [selectedAudioEncoding, selectedScreenShareEncoding, selectedVideoDeviceId, selectedVideoEncoding, session]
  )

  const updateCatalogTracks = useCallback(
    (updater: (prev: EditableMeetingCatalogTrack[]) => EditableMeetingCatalogTrack[], actionLabel: string) => {
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
      updateCatalogTracks((prev) => {
        if (hasCatalogTrackForSource(prev, source)) {
          return prev
        }
        return appendCatalogTracks(prev, CATALOG_PRESETS[source]())
      }, `ensure ${source} catalog tracks`)
    },
    [updateCatalogTracks]
  )

  const removeCatalogPresetTracks = useCallback(
    (source: CatalogPresetSource) => {
      updateCatalogTracks(
        (prev) =>
          removeCatalogTracksByNames(
            prev,
            CATALOG_PRESETS[source]().map((track) => track.name)
          ),
        `remove ${source} catalog tracks`
      )
    },
    [updateCatalogTracks]
  )

  const toggleCamera = useCallback(async () => {
    if (!session || cameraBusy) {
      return cameraEnabled
    }
    const { publisher } = session.getMediaController()
    return withBusy(setCameraBusy, 'Failed to toggle camera:', cameraEnabled, async () => {
      if (cameraEnabled) {
        await publisher.stopCamera()
        removeCatalogPresetTracks('camera')
      } else {
        await publisher.startCamera(selectedVideoDeviceId ?? undefined)
        ensureCatalogPresetTracks('camera')
      }
      setCameraEnabled(!cameraEnabled)
      return !cameraEnabled
    })
  }, [cameraBusy, cameraEnabled, ensureCatalogPresetTracks, removeCatalogPresetTracks, session, selectedVideoDeviceId])

  const toggleScreenShare = useCallback(async () => {
    if (!session || cameraBusy) {
      return screenShareEnabled
    }
    const { publisher } = session.getMediaController()
    return withBusy(setCameraBusy, 'Failed to toggle screen share:', screenShareEnabled, async () => {
      if (screenShareEnabled) {
        await publisher.stopScreenShare()
        removeCatalogPresetTracks('screenshare')
      } else {
        await publisher.startScreenShare()
        ensureCatalogPresetTracks('screenshare')
      }
      setScreenShareEnabled(!screenShareEnabled)
      return !screenShareEnabled
    })
  }, [cameraBusy, ensureCatalogPresetTracks, removeCatalogPresetTracks, screenShareEnabled, session])

  const toggleMicrophone = useCallback(async () => {
    if (!session || microphoneBusy) {
      return microphoneEnabled
    }
    const { publisher } = session.getMediaController()
    return withBusy(setMicrophoneBusy, 'Failed to toggle microphone:', microphoneEnabled, async () => {
      if (microphoneEnabled) {
        await publisher.stopAudio()
        removeCatalogPresetTracks('audio')
      } else {
        await publisher.startAudio(selectedAudioDeviceId ?? undefined)
        ensureCatalogPresetTracks('audio')
      }
      setMicrophoneEnabled(!microphoneEnabled)
      return !microphoneEnabled
    })
  }, [
    ensureCatalogPresetTracks,
    removeCatalogPresetTracks,
    microphoneBusy,
    microphoneEnabled,
    selectedAudioDeviceId,
    session
  ])

  const setPlayoutSettings = useCallback(
    (userId: string, settings: PlayoutSettings) => {
      if (!session) {
        return
      }
      session.getMediaController().subscriber.setPlayoutSettings(userId, settings)
      setPlayoutSettingsState((prev) => {
        const updated = new Map(prev)
        updated.set(userId, settings)
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
      const { publisher } = session.getMediaController()
      await withBusy(setCameraBusy, 'Failed to switch camera device', undefined, async () => {
        await publisher.stopCamera()
        await publisher.startCamera(deviceId)
      })
    },
    [cameraEnabled, session]
  )

  const selectAudioDevice = useCallback(
    async (deviceId: string) => {
      setSelectedAudioDeviceId(deviceId)
      if (!session || !microphoneEnabled) {
        return
      }
      const { publisher } = session.getMediaController()
      await withBusy(setMicrophoneBusy, 'Failed to switch audio device', undefined, async () => {
        await publisher.stopAudio()
        await publisher.startAudio(deviceId)
      })
    },
    [microphoneEnabled, session]
  )

  const selectVideoEncoding = useCallback(
    async (settings: Partial<VideoEncodingSettings>) => {
      const next = { ...selectedVideoEncoding, ...settings }
      setSelectedVideoEncoding(next)
      setVideoEncoderError(null)
      await session
        ?.getMediaController()
        .publisher.setVideoEncodingSettings(next, selectedVideoDeviceId ?? undefined, cameraEnabled)
    },
    [cameraEnabled, selectedVideoDeviceId, selectedVideoEncoding, session]
  )

  const selectScreenShareEncoding = useCallback(
    async (settings: Partial<VideoEncodingSettings>) => {
      const next = { ...selectedScreenShareEncoding, ...settings }
      setSelectedScreenShareEncoding(next)
      setVideoEncoderError(null)
      await session?.getMediaController().publisher.setScreenShareEncodingSettings(next)
    },
    [selectedScreenShareEncoding, session]
  )

  const selectAudioEncoding = useCallback(
    async (settings: Partial<AudioEncodingSettings>) => {
      const next = { ...selectedAudioEncoding, ...settings }
      setSelectedAudioEncoding(next)
      setAudioEncoderError(null)
      await session?.getMediaController().publisher.setAudioEncodingSettings(next, microphoneEnabled)
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
    const { publisher } = session.getMediaController()
    const setCaptureBusy = (busy: boolean) => {
      setCameraBusy(busy)
      setMicrophoneBusy(busy)
    }
    await withBusy(setCaptureBusy, 'Failed to apply getUserMedia settings', undefined, async () => {
      if (captureSettings.videoEnabled) {
        await publisher.stopCamera()
        await publisher.startCamera(selectedVideoDeviceId ?? undefined)
        await publisher.setVideoEncodingSettings(selectedVideoEncoding, selectedVideoDeviceId ?? undefined, true)
        if (!cameraEnabled) {
          ensureCatalogPresetTracks('camera')
        }
        setCameraEnabled(true)
      } else if (cameraEnabled) {
        await publisher.stopCamera()
        removeCatalogPresetTracks('camera')
        setCameraEnabled(false)
      }

      if (captureSettings.audioEnabled) {
        await publisher.startAudio(selectedAudioDeviceId ?? undefined)
        await publisher.setAudioEncodingSettings(selectedAudioEncoding, true)
        if (!microphoneEnabled) {
          ensureCatalogPresetTracks('audio')
        }
        setMicrophoneEnabled(true)
      } else if (microphoneEnabled) {
        await publisher.stopAudio()
        removeCatalogPresetTracks('audio')
        setMicrophoneEnabled(false)
      }
    })
  }, [
    cameraEnabled,
    captureSettings.audioEnabled,
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
    (track: Omit<EditableMeetingCatalogTrack, 'id'>) => {
      updateCatalogTracks((prev) => [...prev, { ...track, id: createCatalogTrackId() }], 'add catalog track')
    },
    [updateCatalogTracks]
  )

  const updateCatalogTrack = useCallback(
    (id: string, patch: Partial<EditableMeetingCatalogTrack>) => {
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
    activeSpeakerId,
    speakingMemberIds,
    cameraEnabled,
    screenShareEnabled,
    microphoneEnabled,
    cameraBusy,
    microphoneBusy,
    localVideoStream,
    localScreenShareStream,
    localAudioStream,
    localCameraBitrate,
    localScreenShareBitrate,
    localAudioBitrate,
    localCameraVideoSendTiming,
    localScreenShareVideoSendTiming,
    remotePictures,
    toggleCamera,
    toggleScreenShare,
    toggleMicrophone,
    playoutSettings,
    setPlayoutSettings,
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

async function withBusy<T>(
  setBusy: (busy: boolean) => void,
  failureMessage: string,
  fallback: T,
  action: () => Promise<T>
): Promise<T> {
  setBusy(true)
  try {
    return await action()
  } catch (error) {
    console.error(failureMessage, error)
    return fallback
  } finally {
    setBusy(false)
  }
}

function hasCatalogTrackForSource(tracks: EditableMeetingCatalogTrack[], source: CatalogPresetSource): boolean {
  if (source === 'audio') {
    return tracks.some((track) => track.role === 'audio')
  }
  const isScreenshare = source === 'screenshare'
  return tracks.some((track) => track.role === 'video' && isScreenShareTrackName(track.name) === isScreenshare)
}

function deriveVideoEncoding(
  tracks: MeetingCatalogTrack[],
  fallback: VideoEncodingSettings,
  source: 'camera' | 'screenshare'
): VideoEncodingSettings {
  const videoTrack = pickHighestBitrateTrack(
    tracks.filter(
      (track) => track.role === 'video' && isScreenShareTrackName(track.name) === (source === 'screenshare')
    )
  )
  return videoTrack ? trackVideoEncoding(videoTrack, fallback) : fallback
}

function deriveAudioEncoding(tracks: MeetingCatalogTrack[], fallback: AudioEncodingSettings): AudioEncodingSettings {
  const audioTrack = tracks.find((track) => track.role === 'audio')
  return audioTrack ? trackAudioEncoding(audioTrack, fallback) : fallback
}
