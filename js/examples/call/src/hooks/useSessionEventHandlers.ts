import { Dispatch, SetStateAction, useEffect } from 'react'
import { AnnounceMessage, SubscribeErrorMessage, SubscribeOkMessage } from '../../../../pkg/moqt_client_sample'
import { LocalSession, LocalSessionState } from '../session/localSession'
import { Room } from '../types/room'
import { ChatMessage } from '../types/chat'
import {
  addOrUpdateRemoteMember,
  alreadySubscribing,
  resetSubscriptionsOnError,
  updateSubscriptionState
} from '../utils/state/roomState'

interface EventHandlerParams {
  session: LocalSession
  roomName: string
  userName: string
  setRoom: Dispatch<SetStateAction<Room>>
  setChatMessages: Dispatch<SetStateAction<ChatMessage[]>>
}

export function useSessionEventHandlers({ session, roomName, userName, setRoom, setChatMessages }: EventHandlerParams) {
  useEffect(() => {
    const handleAnnounce = createAnnounceHandler({ session, roomName, userName, setRoom })
    session.setOnAnnounceHandler(handleAnnounce)
    return () => {
      session.setOnAnnounceHandler(() => {})
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

interface AnnounceHandlerOptions {
  session: LocalSession
  roomName: string
  userName: string
  setRoom: Dispatch<SetStateAction<Room>>
}

function createAnnounceHandler({ session, roomName, userName, setRoom }: AnnounceHandlerOptions) {
  return (announce: AnnounceMessage) => {
    if (session.status !== LocalSessionState.Ready) {
      return
    }
    const trackNamespace = announce.trackNamespace
    if (!trackNamespace || trackNamespace.length < 2) {
      return
    }

    const [announcedRoom, announcedUser] = trackNamespace
    if (announcedRoom !== roomName || announcedUser === userName) {
      return
    }

    setRoom((currentRoom) => {
      const existingMember = currentRoom.remoteMembers.get(announcedUser)
      if (alreadySubscribing(existingMember)) {
        return currentRoom
      }

      const update = addOrUpdateRemoteMember(currentRoom, announcedUser, trackNamespace)

      subscribeTracks({
        session,
        announcedUser,
        trackNamespace,
        chatSubscribeId: update.chatSubscribeId,
        audioSubscribeId: update.audioSubscribeId,
        videoSubscribeId: update.videoSubscribeId,
        setRoom
      })

      return update.room
    })
  }
}

interface SubscribeTracksOptions {
  session: LocalSession
  announcedUser: string
  trackNamespace: string[]
  chatSubscribeId: bigint
  audioSubscribeId: bigint
  videoSubscribeId: bigint
  setRoom: Dispatch<SetStateAction<Room>>
}

function subscribeTracks({
  session,
  announcedUser,
  trackNamespace,
  chatSubscribeId,
  audioSubscribeId,
  videoSubscribeId,
  setRoom
}: SubscribeTracksOptions) {
  const trackConfigs = [
    { name: 'chat', subscribeId: chatSubscribeId },
    { name: 'audio', subscribeId: audioSubscribeId },
    { name: 'video', subscribeId: videoSubscribeId }
  ]

  ;(async () => {
    try {
      for (const { name, subscribeId } of trackConfigs) {
        const trackAlias = subscribeId
        await session.subscribe(subscribeId, trackAlias, trackNamespace, name)
      }
    } catch (error) {
      console.error(`Failed to subscribe to ${announcedUser}'s tracks:`, error)
      setRoom((currentRoom) => resetSubscriptionsOnError(currentRoom, announcedUser))
    }
  })()
}

function createSubscribeResponseHandler(session: LocalSession, setRoom: Dispatch<SetStateAction<Room>>) {
  return (response: SubscribeOkMessage | SubscribeErrorMessage) => {
    if (session.status !== LocalSessionState.Ready) {
      return
    }
    if ('errorCode' in response) {
      console.error('SUBSCRIBE_ERROR:', response.subscribeId, response.errorCode, response.reasonPhrase)
      setRoom((currentRoom) =>
        updateSubscriptionState(currentRoom, response.subscribeId, (track) => ({
          ...track,
          isSubscribing: false,
          isSubscribed: false
        }))
      )
      return
    }

    setRoom((currentRoom) =>
      updateSubscriptionState(currentRoom, response.subscribeId, (track) => ({
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
