import { FormEvent, useEffect, useState } from 'react'
import { LocalSession } from '../session/localSession'
import { Room } from '../types/room'
import { ChatMessage } from '../types/chat'
import { ChatSidebar } from './ChatSidebar'
import { MemberGrid } from './MemberGrid'
import { RoomHeader } from './RoomHeader'
import { MediaControlBar } from './MediaControlBar'
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
    microphoneEnabled,
    cameraBusy,
    microphoneBusy,
    localVideoStream,
    localAudioBitrate,
    localVideoBitrate,
    remoteMedia,
    toggleCamera,
    toggleMicrophone
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
    session.localMember.publishedTracks.video = enabled
    setRoom((current) => ({
      ...current,
      localMember: {
        ...current.localMember,
        publishedTracks: {
          ...current.localMember.publishedTracks,
          video: enabled
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
          <MediaControlBar
            cameraEnabled={cameraEnabled}
            microphoneEnabled={microphoneEnabled}
            cameraBusy={cameraBusy}
            microphoneBusy={microphoneBusy}
            onToggleCamera={handleToggleCamera}
            onToggleMicrophone={handleToggleMicrophone}
          />
          <MemberGrid
            localMember={room.localMember}
            remoteMembers={Array.from(room.remoteMembers.values())}
            localVideoStream={localVideoStream}
            localVideoBitrate={localVideoBitrate}
            localAudioBitrate={localAudioBitrate}
            remoteMedia={remoteMedia}
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
