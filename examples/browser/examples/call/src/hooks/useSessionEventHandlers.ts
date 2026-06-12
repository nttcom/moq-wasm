import { Dispatch, SetStateAction, useEffect, useRef } from 'react'
import { PublishNamespaceDoneMessage, PublishNamespaceMessage, RequestErrorMessage, SubscribeOkMessage } from '../../../../pkg/moqt_client_wasm'
import { LocalSession, LocalSessionState } from '../session/localSession'
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
}

export function useSessionEventHandlers({ session, roomName, userName, setRoom, setChatMessages }: EventHandlerParams) {
  const subscribedNamespacesRef = useRef<LocalSession | null>(null)

  useEffect(() => {
    const handlePublishNamespace = createPublishNamespaceHandler({ session, roomName, userName, setRoom })
    session.setOnPublishNamespaceHandler(handlePublishNamespace)

    const handlePublishNamespaceDone = createPublishNamespaceDoneHandler({ session, roomName, userName, setRoom })
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
  }, [roomName, session, setRoom, userName])

  useEffect(() => {
    const handleSubscribeResponse = createSubscribeResponseHandler(session)
    session.setOnSubscribeResponseHandler(handleSubscribeResponse)
    return () => {
      session.setOnSubscribeResponseHandler(() => {})
    }
  }, [session, setRoom])

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
}

function createPublishNamespaceHandler({ session, roomName, userName, setRoom }: PublishNamespaceHandlerOptions) {
  return (publishNamespace: PublishNamespaceMessage) => {
    if (session.status !== LocalSessionState.Ready) {
      return
    }
    const trackNamespace = publishNamespace.trackNamespace
    if (!trackNamespace || trackNamespace.length < 2) {
      return
    }

    const [announcedRoom, announcedUser] = trackNamespace
    if (announcedRoom !== roomName || announcedUser === userName) {
      return
    }

    setRoom((currentRoom) => addOrUpdateRemoteMember(currentRoom, announcedUser, trackNamespace))
  }
}

function createPublishNamespaceDoneHandler({ session, roomName, userName, setRoom }: PublishNamespaceHandlerOptions) {
  return (message: PublishNamespaceDoneMessage) => {
    if (session.status !== LocalSessionState.Ready) {
      return
    }
    const trackNamespace = message.trackNamespace
    if (!trackNamespace || trackNamespace.length < 2) {
      return
    }

    const [announcedRoom, announcedUser] = trackNamespace
    if (announcedRoom !== roomName || announcedUser === userName) {
      return
    }

    setRoom((currentRoom) => {
      const member = currentRoom.remoteMembers.get(announcedUser)
      if (!member) {
        return currentRoom
      }
      // Release the dangling subscriptions outside the state updater.
      queueMicrotask(() => unsubscribeMemberTracks(session, member))
      return removeRemoteMember(currentRoom, announcedUser)
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
    void session.unsubscribe(track.subscribeId, role).catch((error) => {
      console.warn(`[call] failed to unsubscribe departed member ${member.id} (${role})`, error)
    })
  }
}

// Subscription state (isSubscribing/isSubscribed/subscribeId) is now driven by
// the awaited result of session.subscribe() in CallRoom, so this handler only
// surfaces errors for diagnostics.
function createSubscribeResponseHandler(session: LocalSession) {
  return (response: SubscribeOkMessage | RequestErrorMessage) => {
    if (session.status !== LocalSessionState.Ready) {
      return
    }
    if ('errorCode' in response) {
      console.error('SUBSCRIBE_ERROR:', response.requestId, response.errorCode, response.reasonPhrase)
    }
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
