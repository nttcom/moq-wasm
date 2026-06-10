import { RemoteMember, SubscriptionState } from '../../types/member'
import { Room } from '../../types/room'

export function addOrUpdateRemoteMember(room: Room, announcedUser: string, trackNamespace: string[]): Room {
  const existingMember = room.remoteMembers.get(announcedUser)
  const updatedMember = buildRemoteMember({ existingMember, announcedUser, trackNamespace })

  const updatedMembers = new Map(room.remoteMembers)
  updatedMembers.set(announcedUser, updatedMember)

  return { ...room, remoteMembers: updatedMembers }
}

export function resetSubscriptionsOnError(room: Room, userId: string): Room {
  const member = room.remoteMembers.get(userId)
  if (!member) {
    return room
  }
  const updatedMember: RemoteMember = {
    ...member,
    subscribedTracks: {
      chat: { ...member.subscribedTracks.chat, isSubscribing: false },
      audio: { ...member.subscribedTracks.audio, isSubscribing: false },
      video: { ...member.subscribedTracks.video, isSubscribing: false },
      screenshare: { ...member.subscribedTracks.screenshare, isSubscribing: false }
    }
  }
  const updatedMembers = new Map(room.remoteMembers)
  updatedMembers.set(userId, updatedMember)
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

export function alreadySubscribing(existingMember?: RemoteMember): boolean {
  if (!existingMember) {
    return false
  }
  return (
    existingMember.subscribedTracks.chat.isSubscribing ||
    existingMember.subscribedTracks.video.isSubscribing ||
    existingMember.subscribedTracks.screenshare.isSubscribing ||
    existingMember.subscribedTracks.audio.isSubscribing
  )
}

interface BuildRemoteMemberOptions {
  existingMember?: RemoteMember
  announcedUser: string
  trackNamespace: string[]
}

export function buildRemoteMember({
  existingMember,
  announcedUser,
  trackNamespace
}: BuildRemoteMemberOptions): RemoteMember {
  if (existingMember) {
    return {
      ...existingMember,
      announcedTracks: {
        chat: { isAnnounced: true, trackNamespace },
        video: { isAnnounced: true, trackNamespace },
        screenshare: { isAnnounced: true, trackNamespace },
        audio: { isAnnounced: true, trackNamespace }
      },
      subscribedTracks: {
        chat: {
          isSubscribing: existingMember.subscribedTracks.chat.isSubscribing,
          isSubscribed: existingMember.subscribedTracks.chat.isSubscribed,
          subscribeId: existingMember.subscribedTracks.chat.subscribeId
        },
        audio: {
          isSubscribing: existingMember.subscribedTracks.audio.isSubscribing,
          isSubscribed: existingMember.subscribedTracks.audio.isSubscribed,
          subscribeId: existingMember.subscribedTracks.audio.subscribeId
        },
        video: {
          isSubscribing: existingMember.subscribedTracks.video.isSubscribing,
          isSubscribed: existingMember.subscribedTracks.video.isSubscribed,
          subscribeId: existingMember.subscribedTracks.video.subscribeId
        },
        screenshare: {
          isSubscribing: existingMember.subscribedTracks.screenshare.isSubscribing,
          isSubscribed: existingMember.subscribedTracks.screenshare.isSubscribed,
          subscribeId: existingMember.subscribedTracks.screenshare.subscribeId
        }
      }
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
    subscribedTracks: {
      chat: { isSubscribing: false, isSubscribed: false },
      audio: { isSubscribing: false, isSubscribed: false },
      video: { isSubscribing: false, isSubscribed: false },
      screenshare: { isSubscribing: false, isSubscribed: false }
    }
  }
}
