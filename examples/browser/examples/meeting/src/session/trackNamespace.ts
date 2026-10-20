import { readMeetingAuth } from './authToken'

const MEETING_NAMESPACE_ROOT = readMeetingAuth().namespaceRoot

export type ParsedTrackNamespace = {
  roomName: string
  userName: string
}

export function buildTrackNamespace(roomName: string, userName: string): string[] {
  return [MEETING_NAMESPACE_ROOT, roomName, userName]
}

export function buildNamespacePrefix(roomName: string): string[] {
  return [MEETING_NAMESPACE_ROOT, roomName]
}

export function parseTrackNamespace(trackNamespace: string[] | undefined): ParsedTrackNamespace | null {
  if (!trackNamespace || trackNamespace.length !== 3 || trackNamespace[0] !== MEETING_NAMESPACE_ROOT) {
    return null
  }
  return { roomName: trackNamespace[1], userName: trackNamespace[2] }
}

/// Where the transcriber bot publishes its transcript (docs/architecture/ai-bots/meeting-transcriber.md):
/// four elements, so it never reads as a member.
export function buildTranscriberNamespace(roomName: string): string[] {
  return [...buildNamespacePrefix(roomName), 'bots', 'transcriber']
}

export function isTranscriberNamespace(trackNamespace: string[] | undefined, roomName: string): boolean {
  return trackNamespace?.join('/') === buildTranscriberNamespace(roomName).join('/')
}

/// The transcriber serves rooms of the anonymous app only, whose root a page without a JWT uses.
export const TRANSCRIBER_AVAILABLE = !readMeetingAuth().token
