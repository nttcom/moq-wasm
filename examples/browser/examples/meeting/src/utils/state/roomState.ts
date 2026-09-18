import { RemoteMember, SubscriptionState } from '../../types/member'
import { Room } from '../../types/room'

export function addOrUpdateRemoteMember(room: Room, announcedUser: string, trackNamespace: string[]): Room {
  const existingMember = room.remoteMembers.get(announcedUser)
  const updatedMember = buildRemoteMember({ existingMember, announcedUser, trackNamespace })

  const updatedMembers = new Map(room.remoteMembers)
  updatedMembers.set(announcedUser, updatedMember)

  return { ...room, remoteMembers: updatedMembers }
}

export function removeRemoteMember(room: Room, userId: string): Room {
  if (!room.remoteMembers.has(userId)) {
    return room
  }
  const updatedMembers = new Map(room.remoteMembers)
  updatedMembers.delete(userId)
  return { ...room, remoteMembers: updatedMembers }
}

export function updateSubscriptionState(
  room: Room,
  subscribeId: bigint,
  updater: (track: SubscriptionState) => SubscriptionState
): Room {
  const updatedMembers = new Map(room.remoteMembers)
  let updated = false

  for (const [memberId, member] of updatedMembers.entries()) {
    const trackEntries = Object.entries(member.subscribedTracks) as Array<
      [keyof RemoteMember['subscribedTracks'], SubscriptionState]
    >

    for (const [trackKey, trackState] of trackEntries) {
      if (trackState.subscribeId === subscribeId) {
        const updatedMember: RemoteMember = {
          ...member,
          subscribedTracks: {
            ...member.subscribedTracks,
            [trackKey]: updater(trackState)
          }
        }
        updatedMembers.set(memberId, updatedMember)
        updated = true
        break
      }
    }

    if (updated) {
      break
    }
  }

  return updated ? { ...room, remoteMembers: updatedMembers } : room
}

interface BuildRemoteMemberOptions {
  existingMember?: RemoteMember
  announcedUser: string
  trackNamespace: string[]
}

function createFreshSubscribedTracks(): RemoteMember['subscribedTracks'] {
  return {
    chat: { isSubscribing: false, isSubscribed: false },
    audio: { isSubscribing: false, isSubscribed: false },
    video: { isSubscribing: false, isSubscribed: false },
    screenshare: { isSubscribing: false, isSubscribed: false }
  }
}

function buildRemoteMember({ existingMember, announcedUser, trackNamespace }: BuildRemoteMemberOptions): RemoteMember {
  if (existingMember) {
    return {
      ...existingMember,
      announcedTracks: {
        chat: { isAnnounced: true, trackNamespace },
        video: { isAnnounced: true, trackNamespace },
        screenshare: { isAnnounced: true, trackNamespace },
        audio: { isAnnounced: true, trackNamespace }
      },
      // Same-name rejoin is a fresh generation; do not carry stale subscription state forward.
      subscribedTracks: createFreshSubscribedTracks()
    }
  }

  return {
    id: announcedUser,
    name: announcedUser,
    announcedTracks: {
      chat: { isAnnounced: true, trackNamespace },
      video: { isAnnounced: true, trackNamespace },
      screenshare: { isAnnounced: true, trackNamespace },
      audio: { isAnnounced: true, trackNamespace }
    },
    // subscribeId is left undefined until the track is actually subscribed; the
    // id is then taken from subscribe()'s return value (issued by moqtClient).
    subscribedTracks: createFreshSubscribedTracks()
  }
}
