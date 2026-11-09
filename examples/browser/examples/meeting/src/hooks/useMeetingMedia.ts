import { useCallback, useEffect, useState } from 'react'
import { LocalSession } from '../session/localSession'
import type { RemotePictures } from '../media/mediaSubscriber'
import type { PlayoutSettings } from '../types/playout'
import {
  DEFAULT_SCREEN_SHARE_ENCODING_SETTINGS,
  DEFAULT_VIDEO_ENCODING_SETTINGS,
  VIDEO_HARDWARE_ACCELERATION_OPTIONS,
  VIDEO_RESOLUTION_OPTIONS,
  type VideoEncodingSettings
} from '../types/videoEncoding'
import type { CaptureSettingsState } from '../types/captureConstraints'
import { E2E_CAMERA_SETTINGS, INITIAL_CAMERA_ENCODING_SETTINGS } from '../media/meetingCatalog'
import { DominantSpeaker } from '../media/dominantSpeaker'

const SPEAKER_UPDATE_INTERVAL_MS = 250
const NOTICE_DURATION_MS = 4000

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
  remotePictures: Map<string, RemotePictures>
  toggleCamera: () => Promise<boolean>
  toggleScreenShare: () => Promise<boolean>
  toggleMicrophone: () => Promise<boolean>
  playoutSettings: Map<string, PlayoutSettings>
  setPlayoutSettings: (userId: string, settings: PlayoutSettings) => void
  videoResolutionOptions: typeof VIDEO_RESOLUTION_OPTIONS
  videoHardwareAccelerationOptions: typeof VIDEO_HARDWARE_ACCELERATION_OPTIONS
  selectedVideoEncoding: VideoEncodingSettings
  selectVideoEncoding: (settings: Partial<VideoEncodingSettings>) => Promise<void>
  selectedScreenShareEncoding: VideoEncodingSettings
  selectScreenShareEncoding: (settings: Partial<VideoEncodingSettings>) => Promise<void>
  videoDevices: MediaDeviceInfo[]
  audioDevices: MediaDeviceInfo[]
  selectedVideoDeviceId: string | null
  selectedAudioDeviceId: string | null
  selectVideoDevice: (deviceId: string) => Promise<void>
  selectAudioDevice: (deviceId: string) => Promise<void>
  captureSettings: CaptureSettingsState
  updateCaptureSettings: (settings: Partial<CaptureSettingsState>) => void
  applyCaptureSettings: () => Promise<void>
  /// A transient message for the user.
  notice: string | null
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
  const [notice, setNotice] = useState<string | null>(null)

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
      setRemotePictures(new Map())
      setCameraEnabled(false)
      setMicrophoneEnabled(false)
      setScreenShareEnabled(false)
      setPlayoutSettingsState(new Map())
      return
    }

    const { publisher, subscriber } = session.getMediaController()
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
      onPublishError: (message) => setNotice(message),
      onMicrophoneSwitched: (track) => {
        setSelectedAudioDeviceId(track.getSettings().deviceId ?? null)
        setNotice(`Microphone: ${track.label}`)
      },
      onMicrophoneLost: () => setNotice('Microphone unavailable')
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
      setLocalVideoStream(null)
      setLocalScreenShareStream(null)
      setLocalAudioStream(null)
      setCameraEnabled(false)
      setScreenShareEnabled(false)
      setMicrophoneEnabled(false)
      setPlayoutSettingsState(new Map())
    }
  }, [session])

  useEffect(() => {
    if (!notice) {
      return
    }
    const timerId = window.setTimeout(() => setNotice(null), NOTICE_DURATION_MS)
    return () => window.clearTimeout(timerId)
  }, [notice])

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
    session.getMediaController().publisher.setVideoEncodingSettings(selectedVideoEncoding)
  }, [selectedVideoEncoding, session])

  useEffect(() => {
    if (!session) {
      return
    }
    session.getMediaController().publisher.setScreenShareEncodingSettings(selectedScreenShareEncoding)
  }, [selectedScreenShareEncoding, session])

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

  const toggleCamera = useCallback(async () => {
    if (!session || cameraBusy) {
      return cameraEnabled
    }
    const { publisher } = session.getMediaController()
    return withBusy(setCameraBusy, 'Failed to toggle camera:', cameraEnabled, async () => {
      if (cameraEnabled) {
        await publisher.stopCamera()
      } else {
        await publisher.startCamera(selectedVideoDeviceId ?? undefined)
      }
      setCameraEnabled(!cameraEnabled)
      return !cameraEnabled
    })
  }, [cameraBusy, cameraEnabled, session, selectedVideoDeviceId])

  const toggleScreenShare = useCallback(async () => {
    if (!session || cameraBusy) {
      return screenShareEnabled
    }
    const { publisher } = session.getMediaController()
    return withBusy(setCameraBusy, 'Failed to toggle screen share:', screenShareEnabled, async () => {
      if (screenShareEnabled) {
        await publisher.stopScreenShare()
      } else {
        await publisher.startScreenShare()
      }
      setScreenShareEnabled(!screenShareEnabled)
      return !screenShareEnabled
    })
  }, [cameraBusy, screenShareEnabled, session])

  const toggleMicrophone = useCallback(async () => {
    if (!session || microphoneBusy) {
      return microphoneEnabled
    }
    const { publisher } = session.getMediaController()
    return withBusy(setMicrophoneBusy, 'Failed to toggle microphone:', microphoneEnabled, async () => {
      if (microphoneEnabled) {
        await publisher.stopAudio()
      } else {
        await publisher.startAudio(selectedAudioDeviceId ?? undefined)
      }
      setMicrophoneEnabled(!microphoneEnabled)
      return !microphoneEnabled
    })
  }, [microphoneBusy, microphoneEnabled, selectedAudioDeviceId, session])

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
    },
    [selectedVideoEncoding]
  )

  const selectScreenShareEncoding = useCallback(
    async (settings: Partial<VideoEncodingSettings>) => {
      const next = { ...selectedScreenShareEncoding, ...settings }
      setSelectedScreenShareEncoding(next)
    },
    [selectedScreenShareEncoding]
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
        setCameraEnabled(true)
      } else if (cameraEnabled) {
        await publisher.stopCamera()
        setCameraEnabled(false)
      }

      if (captureSettings.audioEnabled) {
        await publisher.startAudio(selectedAudioDeviceId ?? undefined)
        setMicrophoneEnabled(true)
      } else if (microphoneEnabled) {
        await publisher.stopAudio()
        setMicrophoneEnabled(false)
      }
    })
  }, [
    cameraEnabled,
    captureSettings.audioEnabled,
    captureSettings.videoEnabled,
    microphoneEnabled,
    selectedAudioDeviceId,
    selectedVideoDeviceId,
    session
  ])

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
    remotePictures,
    toggleCamera,
    toggleScreenShare,
    toggleMicrophone,
    playoutSettings,
    setPlayoutSettings,
    videoResolutionOptions: VIDEO_RESOLUTION_OPTIONS,
    videoHardwareAccelerationOptions: VIDEO_HARDWARE_ACCELERATION_OPTIONS,
    selectedVideoEncoding,
    selectVideoEncoding,
    selectedScreenShareEncoding,
    selectScreenShareEncoding,
    videoDevices,
    audioDevices,
    selectedVideoDeviceId,
    selectedAudioDeviceId,
    selectVideoDevice,
    selectAudioDevice,
    captureSettings,
    updateCaptureSettings,
    applyCaptureSettings,
    notice
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
