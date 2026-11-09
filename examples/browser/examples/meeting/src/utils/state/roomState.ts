import { RemoteMember, SubscriptionState } from '../../types/member'
import { Room } from '../../types/room'

export function addOrUpdateRemoteMember(room: Room, announcedUser: string, trackNamespace: string[]): Room {
  // A rejoin replaces the member, so its chat subscription state starts over.
  const updatedMember: RemoteMember = {
    id: announcedUser,
    name: announcedUser,
    trackNamespace,
    chat: { isSubscribing: false, isSubscribed: false }
  }

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

export function updateMemberChatState(
  room: Room,
  memberId: string,
  updater: (chat: SubscriptionState) => SubscriptionState
): Room {
  const member = room.remoteMembers.get(memberId)
  if (!member) {
    return room
  }
  const updatedMembers = new Map(room.remoteMembers)
  updatedMembers.set(memberId, { ...member, chat: updater(member.chat) })
  return { ...room, remoteMembers: updatedMembers }
}
