import { useCallback, useEffect, useState } from 'react'
import { LocalSession } from '../session/localSession'
import { RemoteMediaStreams } from '../types/media'

interface UseCallMediaResult {
  cameraEnabled: boolean
  microphoneEnabled: boolean
  cameraBusy: boolean
  microphoneBusy: boolean
  localVideoStream: MediaStream | null
  localAudioStream: MediaStream | null
  localVideoBitrate: number | null
  localAudioBitrate: number | null
  remoteMedia: Map<string, RemoteMediaStreams>
  toggleCamera: () => Promise<boolean>
  toggleMicrophone: () => Promise<boolean>
}

export function useCallMedia(session: LocalSession | null): UseCallMediaResult {
  const [cameraEnabled, setCameraEnabled] = useState(false)
  const [microphoneEnabled, setMicrophoneEnabled] = useState(false)
  const [cameraBusy, setCameraBusy] = useState(false)
  const [microphoneBusy, setMicrophoneBusy] = useState(false)
  const [localVideoStream, setLocalVideoStream] = useState<MediaStream | null>(null)
  const [localAudioStream, setLocalAudioStream] = useState<MediaStream | null>(null)
  const [localVideoBitrate, setLocalVideoBitrate] = useState<number | null>(null)
  const [localAudioBitrate, setLocalAudioBitrate] = useState<number | null>(null)
  const [remoteMedia, setRemoteMedia] = useState<Map<string, RemoteMediaStreams>>(new Map())

  useEffect(() => {
    if (!session) {
      setLocalVideoStream(null)
      setLocalAudioStream(null)
      setRemoteMedia(new Map())
      setCameraEnabled(false)
      setMicrophoneEnabled(false)
      return
    }

    const controller = session.getMediaController()
    controller.setHandlers({
      onLocalVideoStream: (stream) => setLocalVideoStream(stream),
      onLocalAudioStream: (stream) => setLocalAudioStream(stream),
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
      onRemoteVideoLatency: (userId, ms) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          updated.set(userId, { ...current, videoLatencyMs: ms })
          return updated
        }),
      onRemoteAudioLatency: (userId, ms) =>
        setRemoteMedia((prev) => {
          const updated = new Map(prev)
          const current = updated.get(userId) ?? {}
          updated.set(userId, { ...current, audioLatencyMs: ms })
          return updated
        })
    })

    return () => {
      controller.setHandlers({})
      setRemoteMedia(new Map())
      setLocalVideoBitrate(null)
      setLocalAudioBitrate(null)
      setLocalVideoStream(null)
      setLocalAudioStream(null)
      setCameraEnabled(false)
      setMicrophoneEnabled(false)
    }
  }, [session])

  const toggleCamera = useCallback(async () => {
    if (!session || cameraBusy) {
      return cameraEnabled
    }
    const controller = session.getMediaController()
    setCameraBusy(true)
    const nextState = !cameraEnabled
    try {
      if (nextState) {
        await controller.startCamera()
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
  }, [cameraBusy, cameraEnabled, session])

  const toggleMicrophone = useCallback(async () => {
    if (!session || microphoneBusy) {
      return microphoneEnabled
    }
    const controller = session.getMediaController()
    setMicrophoneBusy(true)
    const nextState = !microphoneEnabled
    try {
      if (nextState) {
        await controller.startMicrophone()
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
  }, [microphoneBusy, microphoneEnabled, session])

  return {
    cameraEnabled,
    microphoneEnabled,
    cameraBusy,
    microphoneBusy,
    localVideoStream,
    localAudioStream,
    localVideoBitrate,
    localAudioBitrate,
    remoteMedia,
    toggleCamera,
    toggleMicrophone
  }
}
