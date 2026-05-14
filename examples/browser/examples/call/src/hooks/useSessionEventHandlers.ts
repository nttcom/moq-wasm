import { Dispatch, SetStateAction, useEffect, useRef } from 'react'
import { PublishNamespaceMessage, RequestErrorMessage, SubscribeOkMessage } from '../../../../pkg/moqt_client_wasm'
import { LocalSession, LocalSessionState } from '../session/localSession'
import { Room } from '../types/room'
import { ChatMessage } from '../types/chat'
import { addOrUpdateRemoteMember, updateSubscriptionState } from '../utils/state/roomState'

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
    }
  }, [roomName, session, setRoom, userName])

  useEffect(() => {
    const handleSubscribeResponse = createSubscribeResponseHandler(session, setRoom)
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

    setRoom((currentRoom) => {
      const update = addOrUpdateRemoteMember(currentRoom, announcedUser, trackNamespace)
      return update.room
    })
  }
}

function createSubscribeResponseHandler(session: LocalSession, setRoom: Dispatch<SetStateAction<Room>>) {
  return (response: SubscribeOkMessage | RequestErrorMessage) => {
    if (session.status !== LocalSessionState.Ready) {
      return
    }
    if ('errorCode' in response) {
      console.error('SUBSCRIBE_ERROR:', response.requestId, response.errorCode, response.reasonPhrase)
      setRoom((currentRoom) =>
        updateSubscriptionState(currentRoom, response.requestId, (track) => ({
          ...track,
          isSubscribing: false,
          isSubscribed: false
        }))
      )
      return
    }

    setRoom((currentRoom) =>
      updateSubscriptionState(currentRoom, response.requestId, (track) => ({
        ...track,
        isSubscribed: true,
        isSubscribing: false
      }))
    )
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
