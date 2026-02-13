import { RemoteMember, SubscriptionState } from '../../types/member'
import { Room } from '../../types/room'

export interface RemoteMemberUpdateResult {
  room: Room
  chatSubscribeId: bigint
  catalogSubscribeId: bigint
  audioSubscribeId: bigint
  videoSubscribeId: bigint
  screenshareSubscribeId: bigint
  targetMember: RemoteMember
}

export function addOrUpdateRemoteMember(
  room: Room,
  announcedUser: string,
  trackNamespace: string[]
): RemoteMemberUpdateResult {
  const existingMember = room.remoteMembers.get(announcedUser)
  const { chatSubscribeId, catalogSubscribeId, audioSubscribeId, videoSubscribeId, screenshareSubscribeId } =
    computeSubscribeIds(room, announcedUser, existingMember)

  const updatedMember = buildRemoteMember({
    existingMember,
    announcedUser,
    trackNamespace,
    chatSubscribeId,
    audioSubscribeId,
    videoSubscribeId,
    screenshareSubscribeId
  })

  const updatedMembers = new Map(room.remoteMembers)
  updatedMembers.set(announcedUser, updatedMember)

  return {
    room: { ...room, remoteMembers: updatedMembers },
    chatSubscribeId,
    catalogSubscribeId,
    audioSubscribeId,
    videoSubscribeId,
    screenshareSubscribeId,
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

export function computeSubscribeIds(room: Room, announcedUser: string, existingMember?: RemoteMember) {
  const memberIndex = existingMember
    ? Array.from(room.remoteMembers.keys()).indexOf(announcedUser)
    : room.remoteMembers.size
  const baseSubscribeId = memberIndex * 5
  return {
    chatSubscribeId: BigInt(baseSubscribeId),
    catalogSubscribeId: BigInt(baseSubscribeId + 1),
    audioSubscribeId: BigInt(baseSubscribeId + 2),
    videoSubscribeId: BigInt(baseSubscribeId + 3),
    screenshareSubscribeId: BigInt(baseSubscribeId + 4)
  }
}

interface BuildRemoteMemberOptions {
  existingMember?: RemoteMember
  announcedUser: string
  trackNamespace: string[]
  chatSubscribeId: bigint
  audioSubscribeId: bigint
  videoSubscribeId: bigint
  screenshareSubscribeId: bigint
}

export function buildRemoteMember({
  existingMember,
  announcedUser,
  trackNamespace,
  chatSubscribeId,
  audioSubscribeId,
  videoSubscribeId,
  screenshareSubscribeId
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
          subscribeId: chatSubscribeId
        },
        audio: {
          isSubscribing: existingMember.subscribedTracks.audio.isSubscribing,
          isSubscribed: existingMember.subscribedTracks.audio.isSubscribed,
          subscribeId: audioSubscribeId
        },
        video: {
          isSubscribing: existingMember.subscribedTracks.video.isSubscribing,
          isSubscribed: existingMember.subscribedTracks.video.isSubscribed,
          subscribeId: videoSubscribeId
        },
        screenshare: {
          isSubscribing: existingMember.subscribedTracks.screenshare.isSubscribing,
          isSubscribed: existingMember.subscribedTracks.screenshare.isSubscribed,
          subscribeId: screenshareSubscribeId
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
    subscribedTracks: {
      chat: { isSubscribing: false, isSubscribed: false, subscribeId: chatSubscribeId },
      audio: { isSubscribing: false, isSubscribed: false, subscribeId: audioSubscribeId },
      video: { isSubscribing: false, isSubscribed: false, subscribeId: videoSubscribeId },
      screenshare: { isSubscribing: false, isSubscribed: false, subscribeId: screenshareSubscribeId }
    }
  }
}
