import { access } from '../../../../utils/access'
import { botNamespace, sameNamespace } from '../../../../utils/roomBots'

const MEETING_NAMESPACE_ROOT = access.namespaceRoot
export const TRANSCRIBER_BOT_TYPE = 'transcriber'

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
  return botNamespace(buildNamespacePrefix(roomName), TRANSCRIBER_BOT_TYPE)
}

export function isTranscriberNamespace(trackNamespace: string[] | undefined, roomName: string): boolean {
  return trackNamespace !== undefined && sameNamespace(trackNamespace, buildTranscriberNamespace(roomName))
}

/// Only a temporary app's page can start the transcriber: it goes through the temporary-apps service
/// (docs/architecture/bots/architecture.md "Temporary apps").
export const TRANSCRIBER_AVAILABLE = access.kind === 'temporary'
