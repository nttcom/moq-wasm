import { readCallAuth } from './authToken'

const CALL_NAMESPACE_ROOT = readCallAuth().namespaceRoot

export type ParsedTrackNamespace = {
  roomName: string
  userName: string
}

export function buildTrackNamespace(roomName: string, userName: string): string[] {
  return [CALL_NAMESPACE_ROOT, roomName, userName]
}

export function buildNamespacePrefix(roomName: string): string[] {
  return [CALL_NAMESPACE_ROOT, roomName]
}

export function parseTrackNamespace(trackNamespace: string[] | undefined): ParsedTrackNamespace | null {
  if (!trackNamespace || trackNamespace.length !== 3 || trackNamespace[0] !== CALL_NAMESPACE_ROOT) {
    return null
  }
  return { roomName: trackNamespace[1], userName: trackNamespace[2] }
}
