import type { LocalMember, RemoteMember } from './member'

export interface Room {
  name: string
  localMember: LocalMember
  remoteMembers: Map<string, RemoteMember>
}
