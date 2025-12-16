import { useCallback, useEffect, useState } from 'react'
import { LocalSession } from '../session/localSession'
import { RemoteMediaStreams } from '../types/media'
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

interface UseCallMediaResult {
  cameraEnabled: boolean
  screenShareEnabled: boolean
  microphoneEnabled: boolean
  cameraBusy: boolean
  microphoneBusy: boolean
  localVideoStream: MediaStream | null
  localAudioStream: MediaStream | null
  localVideoBitrate: number | null
  localAudioBitrate: number | null
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
}

export function useCallMedia(session: LocalSession | null): UseCallMediaResult {
  const [cameraEnabled, setCameraEnabled] = useState(false)
  const [screenShareEnabled, setScreenShareEnabled] = useState(false)
  const [microphoneEnabled, setMicrophoneEnabled] = useState(false)
  const [cameraBusy, setCameraBusy] = useState(false)
  const [microphoneBusy, setMicrophoneBusy] = useState(false)
  const [localVideoStream, setLocalVideoStream] = useState<MediaStream | null>(null)
  const [localAudioStream, setLocalAudioStream] = useState<MediaStream | null>(null)
  const [localVideoBitrate, setLocalVideoBitrate] = useState<number | null>(null)
  const [localAudioBitrate, setLocalAudioBitrate] = useState<number | null>(null)
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
  const [selectedScreenShareEncoding, setSelectedScreenShareEncoding] = useState<VideoEncodingSettings>({
    codec: VIDEO_CODEC_OPTIONS.find((c) => c.id.startsWith('av1'))?.codec ?? VIDEO_CODEC_OPTIONS[3].codec,
    width: 1920,
    height: 1080,
    bitrate: VIDEO_BITRATE_OPTIONS.find((b) => b.id === '1mbps')?.bitrate ?? VIDEO_BITRATE_OPTIONS[2].bitrate
  })
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
      setLocalAudioStream(null)
      setRemoteMedia(new Map())
      setCameraEnabled(false)
      setMicrophoneEnabled(false)
      setScreenShareEnabled(false)
      setVideoJitterConfigs(new Map())
      setAudioJitterConfigs(new Map())
      setVideoEncoderError(null)
      setAudioEncoderError(null)
      return
    }

    const controller = session.getMediaController()
    controller.setHandlers({
      onLocalVideoStream: (stream) => {
        setLocalVideoStream(stream)
        if (stream) {
          setCameraEnabled(true)
        } else {
          setCameraEnabled(false)
          setScreenShareEnabled(false)
        }
      },
      onLocalAudioStream: (stream) => {
        setLocalAudioStream(stream)
        setMicrophoneEnabled(!!stream)
      },
      onRemoteVideoStream: (userId, stream) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          updated.set(userId, { ...current, videoStream: stream })
          return updated
        }),
      onRemoteAudioStream: (userId, stream) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          updated.set(userId, { ...current, audioStream: stream })
          return updated
        }),
      onLocalVideoBitrate: (kbps) => setLocalVideoBitrate(kbps),
      onLocalAudioBitrate: (kbps) => setLocalAudioBitrate(kbps),
      onRemoteVideoBitrate: (userId, kbps) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          updated.set(userId, { ...current, videoBitrateKbps: kbps })
          return updated
        }),
      onRemoteAudioBitrate: (userId, kbps) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          updated.set(userId, { ...current, audioBitrateKbps: kbps })
          return updated
        }),
      onRemoteVideoReceiveLatency: (userId, ms) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          updated.set(userId, {
            ...current,
            videoLatencyReceiveMs: ms
          })
          return updated
        }),
      onRemoteVideoRenderingLatency: (userId, ms) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          updated.set(userId, {
            ...current,
            videoLatencyRenderMs: ms
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
          updated.set(userId, {
            ...current,
            audioLatencyRenderMs: ms
          })
          return updated
        }),
      onRemoteVideoConfig: (userId, config) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          updated.set(userId, {
            ...current,
            videoCodec: config.codec,
            videoWidth: config.width,
            videoHeight: config.height
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
      setLocalVideoStream(null)
      setLocalAudioStream(null)
      setCameraEnabled(session.localMember.publishedTracks.video)
      setMicrophoneEnabled(session.localMember.publishedTracks.audio)
      setVideoJitterConfigs(new Map())
      setAudioJitterConfigs(new Map())
    }
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

  const toggleCamera = useCallback(async () => {
    if (!session || cameraBusy) {
      return cameraEnabled
    }
    const controller = session.getMediaController()
    setCameraBusy(true)
    const nextState = !cameraEnabled
    try {
      if (nextState) {
        if (screenShareEnabled) {
          await controller.stopScreenShare()
          setScreenShareEnabled(false)
        }
        await controller.startCamera(selectedVideoDeviceId ?? undefined, {
          frameRate: captureSettings.frameRate,
          width: captureSettings.width,
          height: captureSettings.height
        })
      } else {
        await controller.stopCamera()
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
    screenShareEnabled,
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
        if (cameraEnabled) {
          await controller.stopCamera()
          setCameraEnabled(false)
        }
        await controller.startScreenShare()
      } else {
        await controller.stopScreenShare()
      }
      setScreenShareEnabled(nextState)
      return nextState
    } catch (error) {
      console.error('Failed to toggle screen share:', error)
      return screenShareEnabled
    } finally {
      setCameraBusy(false)
    }
  }, [cameraBusy, screenShareEnabled, session])

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
      } else {
        await controller.stopMicrophone()
      }
      setMicrophoneEnabled(nextState)
      return nextState
    } catch (error) {
      console.error('Failed to toggle microphone:', error)
      return microphoneEnabled
    } finally {
      setMicrophoneBusy(false)
    }
  }, [microphoneBusy, microphoneEnabled, session, selectedAudioDeviceId])

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
      const next = {
        codec: settings.codec ?? selectedVideoEncoding.codec,
        width: settings.width ?? selectedVideoEncoding.width,
        height: settings.height ?? selectedVideoEncoding.height,
        bitrate: settings.bitrate ?? selectedVideoEncoding.bitrate
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
      const next = {
        codec: settings.codec ?? selectedScreenShareEncoding.codec,
        width: settings.width ?? selectedScreenShareEncoding.width,
        height: settings.height ?? selectedScreenShareEncoding.height,
        bitrate: settings.bitrate ?? selectedScreenShareEncoding.bitrate
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
        if (screenShareEnabled) {
          await controller.stopScreenShare()
          setScreenShareEnabled(false)
        }
        await controller.stopCamera()
        await controller.startCamera(selectedVideoDeviceId ?? undefined, {
          frameRate: captureSettings.frameRate,
          width: captureSettings.width,
          height: captureSettings.height
        })
        await controller.setVideoEncodingSettings(selectedVideoEncoding, selectedVideoDeviceId ?? undefined, true)
        setCameraEnabled(true)
      } else if (cameraEnabled || screenShareEnabled) {
        if (screenShareEnabled) {
          await controller.stopScreenShare()
          setScreenShareEnabled(false)
        } else {
          await controller.stopCamera()
        }
        setCameraEnabled(false)
      }

      if (captureSettings.audioEnabled) {
        await controller.startMicrophone(selectedAudioDeviceId ?? undefined, {
          echoCancellation: captureSettings.echoCancellation,
          noiseSuppression: captureSettings.noiseSuppression,
          autoGainControl: captureSettings.autoGainControl
        })
        await controller.setAudioEncodingSettings(selectedAudioEncoding, true)
        setMicrophoneEnabled(true)
      } else if (microphoneEnabled) {
        await controller.stopMicrophone()
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
    microphoneEnabled,
    screenShareEnabled,
    selectedAudioEncoding,
    selectedAudioDeviceId,
    selectedVideoEncoding,
    selectedVideoDeviceId,
    session
  ])

  return {
    cameraEnabled,
    screenShareEnabled,
    microphoneEnabled,
    cameraBusy,
    microphoneBusy,
    localVideoStream,
    localAudioStream,
    localVideoBitrate,
    localAudioBitrate,
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
    applyCaptureSettings
  }
}
