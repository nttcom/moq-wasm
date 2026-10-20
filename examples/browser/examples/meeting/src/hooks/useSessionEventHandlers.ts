import { Dispatch, SetStateAction, useEffect, useRef } from 'react'
import { PublishNamespaceDoneMessage, PublishNamespaceMessage } from '../../../../pkg/moqt'
import { LocalSession, LocalSessionState } from '../session/localSession'
import { isTranscriberNamespace, parseTrackNamespace } from '../session/trackNamespace'
import { Room } from '../types/room'
import { ChatMessage } from '../types/chat'
import { RemoteMember } from '../types/member'
import { addOrUpdateRemoteMember, removeRemoteMember } from '../utils/state/roomState'

interface EventHandlerParams {
  session: LocalSession
  roomName: string
  userName: string
  setRoom: Dispatch<SetStateAction<Room>>
  setChatMessages: Dispatch<SetStateAction<ChatMessage[]>>
  /// Whether the transcriber bot's namespace is published in the room.
  setTranscriberPresent: Dispatch<SetStateAction<boolean>>
}

export function useSessionEventHandlers({
  session,
  roomName,
  userName,
  setRoom,
  setChatMessages,
  setTranscriberPresent
}: EventHandlerParams) {
  const subscribedNamespacesRef = useRef<LocalSession | null>(null)

  useEffect(() => {
    const handlePublishNamespace = createPublishNamespaceHandler({
      session,
      roomName,
      userName,
      setRoom,
      setTranscriberPresent
    })
    session.setOnPublishNamespaceHandler(handlePublishNamespace)

    const handlePublishNamespaceDone = createPublishNamespaceDoneHandler({
      session,
      roomName,
      userName,
      setRoom,
      setTranscriberPresent
    })
    session.setOnPublishNamespaceDoneHandler(handlePublishNamespaceDone)

    const ensureSubscribeNamespace = async () => {
      if (subscribedNamespacesRef.current === session) {
        return
      }
      try {
        await session.subscribeNamespace(session.trackNamespacePrefix)
        subscribedNamespacesRef.current = session
      } catch (error) {
        console.error('Failed to subscribe namespace:', error)
      }
    }

    void ensureSubscribeNamespace()
    return () => {
      session.setOnPublishNamespaceHandler(null)
      session.setOnPublishNamespaceDoneHandler(null)
    }
  }, [roomName, session, setRoom, setTranscriberPresent, userName])

  useEffect(() => {
    const handleChat = createChatHandler(session, setChatMessages)
    session.setOnChatMessageHandler(handleChat)
    return () => {
      session.setOnChatMessageHandler(null)
    }
  }, [session, setChatMessages])
}

interface PublishNamespaceHandlerOptions {
  session: LocalSession
  roomName: string
  userName: string
  setRoom: Dispatch<SetStateAction<Room>>
  setTranscriberPresent: Dispatch<SetStateAction<boolean>>
}

function createPublishNamespaceHandler({
  session,
  roomName,
  userName,
  setRoom,
  setTranscriberPresent
}: PublishNamespaceHandlerOptions) {
  return (publishNamespace: PublishNamespaceMessage) => {
    if (session.status !== LocalSessionState.Ready) {
      return
    }
    if (isTranscriberNamespace(publishNamespace.trackNamespace, roomName)) {
      setTranscriberPresent(true)
      return
    }
    const announced = parseTrackNamespace(publishNamespace.trackNamespace)
    if (!announced || announced.roomName !== roomName || announced.userName === userName) {
      return
    }

    setRoom((currentRoom) => addOrUpdateRemoteMember(currentRoom, announced.userName, publishNamespace.trackNamespace))
  }
}

function createPublishNamespaceDoneHandler({
  session,
  roomName,
  userName,
  setRoom,
  setTranscriberPresent
}: PublishNamespaceHandlerOptions) {
  return (message: PublishNamespaceDoneMessage) => {
    if (session.status !== LocalSessionState.Ready) {
      return
    }
    if (isTranscriberNamespace(message.trackNamespace, roomName)) {
      setTranscriberPresent(false)
      return
    }
    const announced = parseTrackNamespace(message.trackNamespace)
    if (!announced || announced.roomName !== roomName || announced.userName === userName) {
      return
    }

    setRoom((currentRoom) => {
      const member = currentRoom.remoteMembers.get(announced.userName)
      if (!member) {
        return currentRoom
      }
      // Release local media contexts immediately for the departed member.
      unsubscribeMemberTracks(session, member)
      return removeRemoteMember(currentRoom, announced.userName)
    })
  }
}

function unsubscribeMemberTracks(session: LocalSession, member: RemoteMember) {
  const tracks = member.subscribedTracks
  const entries = [
    ['chat', tracks.chat],
    ['video', tracks.video],
    ['screenshare', tracks.screenshare],
    ['audio', tracks.audio]
  ] as const
  for (const [role, track] of entries) {
    if (track.subscribeId === undefined) {
      continue
    }
    void session.unsubscribe(track.subscribeId).catch((error) => {
      console.warn(`[meeting] failed to unsubscribe departed member ${member.id} (${role})`, error)
    })
  }
}

function createChatHandler(session: LocalSession, setChatMessages: Dispatch<SetStateAction<ChatMessage[]>>) {
  return (message: ChatMessage) => {
    if (session.status !== LocalSessionState.Ready) {
      return
    }
    setChatMessages((prev) => [...prev, message])
  }
}
