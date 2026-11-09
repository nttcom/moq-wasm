import { Dispatch, SetStateAction, useEffect } from 'react'
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

    session.subscribeNamespace().catch((error) => console.error('Failed to subscribe namespace:', error))
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

/// The remote member a namespace in this room announces, once the session is ready. For the
/// transcriber's namespace it sets whether the transcriber is `present` instead.
function announcedRemoteMember(
  { session, roomName, userName, setTranscriberPresent }: PublishNamespaceHandlerOptions,
  trackNamespace: string[],
  present: boolean
) {
  if (session.status !== LocalSessionState.Ready) {
    return
  }
  if (isTranscriberNamespace(trackNamespace, roomName)) {
    setTranscriberPresent(present)
    return
  }
  const announced = parseTrackNamespace(trackNamespace)
  if (!announced || announced.roomName !== roomName || announced.userName === userName) {
    return
  }
  return announced
}

function createPublishNamespaceHandler(options: PublishNamespaceHandlerOptions) {
  const { session, setRoom } = options
  return (trackNamespace: string[]) => {
    const announced = announcedRemoteMember(options, trackNamespace, true)
    if (!announced) {
      return
    }

    // The relay keeps a rejoined member's old subscriptions alive; `add` disposes
    // the old players first, or every object would arrive twice.
    session.getMediaController().players.add(announced.userName, trackNamespace)
    setRoom((currentRoom) => {
      const rejoinedMember = currentRoom.remoteMembers.get(announced.userName)
      if (rejoinedMember) {
        unsubscribeChat(session, rejoinedMember)
      }
      return addOrUpdateRemoteMember(currentRoom, announced.userName, trackNamespace)
    })
  }
}

function createPublishNamespaceDoneHandler(options: PublishNamespaceHandlerOptions) {
  const { session, setRoom } = options
  return (trackNamespace: string[]) => {
    const announced = announcedRemoteMember(options, trackNamespace, false)
    if (!announced) {
      return
    }

    session.getMediaController().players.remove(announced.userName)
    setRoom((currentRoom) => {
      const member = currentRoom.remoteMembers.get(announced.userName)
      if (!member) {
        return currentRoom
      }
      unsubscribeChat(session, member)
      return removeRemoteMember(currentRoom, announced.userName)
    })
  }
}

function unsubscribeChat(session: LocalSession, member: RemoteMember) {
  if (member.chat.subscribeId === undefined) {
    return
  }
  void session.unsubscribe(member.chat.subscribeId).catch((error) => {
    console.warn(`[meeting] failed to unsubscribe the chat of member ${member.id}`, error)
  })
}

function createChatHandler(session: LocalSession, setChatMessages: Dispatch<SetStateAction<ChatMessage[]>>) {
  return (message: ChatMessage) => {
    if (session.status !== LocalSessionState.Ready) {
      return
    }
    setChatMessages((prev) => [...prev, message])
  }
}
