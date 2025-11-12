import { RemoteMember, SubscriptionState } from '../../types/member'
import { Room } from '../../types/room'

export interface RemoteMemberUpdateResult {
  room: Room
  chatSubscribeId: bigint
  audioSubscribeId: bigint
  videoSubscribeId: bigint
  targetMember: RemoteMember
}

export function addOrUpdateRemoteMember(
  room: Room,
  announcedUser: string,
  trackNamespace: string[]
): RemoteMemberUpdateResult {
  const existingMember = room.remoteMembers.get(announcedUser)
  const { chatSubscribeId, audioSubscribeId, videoSubscribeId } = computeSubscribeIds(
    room,
    announcedUser,
    existingMember
  )

  const updatedMember = buildRemoteMember({
    existingMember,
    announcedUser,
    trackNamespace,
    chatSubscribeId,
    audioSubscribeId,
    videoSubscribeId
  })

  const updatedMembers = new Map(room.remoteMembers)
  updatedMembers.set(announcedUser, updatedMember)

  return {
    room: { ...room, remoteMembers: updatedMembers },
    chatSubscribeId,
    audioSubscribeId,
    videoSubscribeId,
    targetMember: updatedMember
  }
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
      video: { ...member.subscribedTracks.video, isSubscribing: false }
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
    existingMember.subscribedTracks.audio.isSubscribing
  )
}

export function computeSubscribeIds(room: Room, announcedUser: string, existingMember?: RemoteMember) {
  const memberIndex = existingMember
    ? Array.from(room.remoteMembers.keys()).indexOf(announcedUser)
    : room.remoteMembers.size
  const baseSubscribeId = memberIndex * 3
  return {
    chatSubscribeId: BigInt(baseSubscribeId),
    audioSubscribeId: BigInt(baseSubscribeId + 1),
    videoSubscribeId: BigInt(baseSubscribeId + 2)
  }
}

interface BuildRemoteMemberOptions {
  existingMember?: RemoteMember
  announcedUser: string
  trackNamespace: string[]
  chatSubscribeId: bigint
  audioSubscribeId: bigint
  videoSubscribeId: bigint
}

export function buildRemoteMember({
  existingMember,
  announcedUser,
  trackNamespace,
  chatSubscribeId,
  audioSubscribeId,
  videoSubscribeId
}: BuildRemoteMemberOptions): RemoteMember {
  if (existingMember) {
    return {
      ...existingMember,
      announcedTracks: {
        chat: { isAnnounced: true, trackNamespace },
        video: { isAnnounced: true, trackNamespace },
        audio: { isAnnounced: true, trackNamespace }
      },
      subscribedTracks: {
        chat: {
          isSubscribing: true,
          isSubscribed: existingMember.subscribedTracks.chat.isSubscribed,
          subscribeId: chatSubscribeId
        },
        audio: {
          isSubscribing: true,
          isSubscribed: existingMember.subscribedTracks.audio.isSubscribed,
          subscribeId: audioSubscribeId
        },
        video: {
          isSubscribing: true,
          isSubscribed: existingMember.subscribedTracks.video.isSubscribed,
          subscribeId: videoSubscribeId
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
      audio: { isAnnounced: true, trackNamespace }
    },
    subscribedTracks: {
      chat: { isSubscribing: true, isSubscribed: false, subscribeId: chatSubscribeId },
      audio: { isSubscribing: true, isSubscribed: false, subscribeId: audioSubscribeId },
      video: { isSubscribing: true, isSubscribed: false, subscribeId: videoSubscribeId }
    }
  }
}
