import { FormEvent, useEffect, useState } from 'react'
import { LocalSession } from '../session/localSession'
import { Room } from '../types/room'
import { ChatMessage } from '../types/chat'
import { ChatSidebar } from './ChatSidebar'
import { MemberGrid } from './MemberGrid'
import { RoomHeader } from './RoomHeader'
import { useSessionEventHandlers } from '../hooks/useSessionEventHandlers'
import { useCallMedia } from '../hooks/useCallMedia'

interface CallRoomProps {
  session: LocalSession
  onLeave: () => void
}

export function CallRoom({ session, onLeave }: CallRoomProps) {
  const roomName = session.roomName
  const userName = session.localMember.name
  const [chatMessage, setChatMessage] = useState('')
  const [chatMessages, setChatMessages] = useState<ChatMessage[]>([])
  const {
    cameraEnabled,
    screenShareEnabled,
    microphoneEnabled,
    cameraBusy,
    microphoneBusy,
    localVideoStream,
    localAudioBitrate,
    localVideoBitrate,
    remoteMedia,
    toggleCamera,
    toggleScreenShare,
    toggleMicrophone,
    videoJitterConfigs,
    setVideoJitterBufferConfig,
    audioJitterConfigs,
    setAudioJitterBufferConfig,
    videoCodecOptions,
    videoResolutionOptions,
    videoBitrateOptions,
    selectedVideoEncoding,
    selectVideoEncoding,
    selectedScreenShareEncoding,
    selectScreenShareEncoding,
    videoEncoderError,
    audioCodecOptions,
    audioBitrateOptions,
    audioChannelOptions,
    selectedAudioEncoding,
    selectAudioEncoding,
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
  } = useCallMedia(session)

  const [room, setRoom] = useState<Room>(() => ({
    name: roomName,
    localMember: {
      ...session.localMember,
      publishedTracks: { ...session.localMember.publishedTracks }
    },
    remoteMembers: new Map()
  }))

  const handleSendChatMessage = async (event?: FormEvent) => {
    event?.preventDefault()
    const trimmed = chatMessage.trim()
    if (!trimmed) {
      return
    }
    try {
      await session.sendChatMessage(trimmed)
      const timestamp = Date.now()
      const namespace = [...session.trackNamespace]
      const localMessage: ChatMessage = {
        sender: userName,
        trackNamespace: namespace,
        groupId: BigInt(timestamp),
        text: trimmed,
        timestamp,
        isLocal: true
      }
      setChatMessages((prev) => [...prev, localMessage])
      setChatMessage('')
    } catch (error) {
      console.error('Failed to send chat message:', error)
    }
  }

  useEffect(() => {
    setRoom({
      name: session.roomName,
      localMember: {
        ...session.localMember,
        publishedTracks: { ...session.localMember.publishedTracks }
      },
      remoteMembers: new Map()
    })
  }, [session])

  useSessionEventHandlers({
    session,
    roomName,
    userName,
    setRoom,
    setChatMessages
  })

  const handleToggleCamera = async () => {
    const enabled = await toggleCamera()
    session.localMember.publishedTracks.video = enabled || screenShareEnabled
    setRoom((current) => ({
      ...current,
      localMember: {
        ...current.localMember,
        publishedTracks: {
          ...current.localMember.publishedTracks,
          video: enabled || screenShareEnabled
        }
      }
    }))
  }

  const handleToggleMicrophone = async () => {
    const enabled = await toggleMicrophone()
    session.localMember.publishedTracks.audio = enabled
    setRoom((current) => ({
      ...current,
      localMember: {
        ...current.localMember,
        publishedTracks: {
          ...current.localMember.publishedTracks,
          audio: enabled
        }
      }
    }))
  }

  return (
    <div className="min-h-screen bg-gradient-to-br from-gray-900 via-blue-900 to-gray-900 text-white">
      <div className="flex min-h-screen flex-col lg:flex-row">
        <main className="flex-1 px-6 py-10 lg:px-10 lg:py-12">
          <RoomHeader roomName={roomName} onLeave={onLeave} />
          <MemberGrid
            localMember={room.localMember}
            remoteMembers={Array.from(room.remoteMembers.values())}
            localVideoStream={localVideoStream}
            localVideoBitrate={localVideoBitrate}
            localAudioBitrate={localAudioBitrate}
            remoteMedia={remoteMedia}
            videoJitterConfigs={videoJitterConfigs}
            onChangeVideoJitterConfig={setVideoJitterBufferConfig}
            audioJitterConfigs={audioJitterConfigs}
            onChangeAudioJitterConfig={setAudioJitterBufferConfig}
            onToggleCamera={handleToggleCamera}
            onToggleScreenShare={async () => {
              const enabled = await toggleScreenShare()
              session.localMember.publishedTracks.video = enabled || cameraEnabled
              setRoom((current) => ({
                ...current,
                localMember: {
                  ...current.localMember,
                  publishedTracks: {
                    ...current.localMember.publishedTracks,
                    video: enabled || cameraEnabled
                  }
                }
              }))
            }}
            onToggleMicrophone={handleToggleMicrophone}
            cameraEnabled={cameraEnabled}
            screenShareEnabled={screenShareEnabled}
            microphoneEnabled={microphoneEnabled}
            cameraBusy={cameraBusy}
            microphoneBusy={microphoneBusy}
            videoDevices={videoDevices}
            audioDevices={audioDevices}
            selectedVideoDeviceId={selectedVideoDeviceId}
            selectedAudioDeviceId={selectedAudioDeviceId}
            onSelectVideoDevice={selectVideoDevice}
            onSelectAudioDevice={selectAudioDevice}
            videoEncodingOptions={{
              codecOptions: videoCodecOptions,
              resolutionOptions: videoResolutionOptions,
              bitrateOptions: videoBitrateOptions
            }}
            selectedVideoEncoding={selectedVideoEncoding}
            onSelectVideoEncoding={selectVideoEncoding}
            selectedScreenShareEncoding={selectedScreenShareEncoding}
            onSelectScreenShareEncoding={selectScreenShareEncoding}
            videoEncoderError={videoEncoderError}
            audioEncodingOptions={{
              codecOptions: audioCodecOptions,
              bitrateOptions: audioBitrateOptions,
              channelOptions: audioChannelOptions
            }}
            selectedAudioEncoding={selectedAudioEncoding}
            onSelectAudioEncoding={selectAudioEncoding}
            audioEncoderError={audioEncoderError}
            captureSettings={captureSettings}
            onChangeCaptureSettings={updateCaptureSettings}
            onApplyCaptureSettings={applyCaptureSettings}
          />
        </main>

        <ChatSidebar
          messages={chatMessages}
          chatMessage={chatMessage}
          onMessageChange={setChatMessage}
          onSend={handleSendChatMessage}
        />
      </div>
    </div>
  )
}
