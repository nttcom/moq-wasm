import type { LocMetadata } from './loc'

export type VideoChunk = {
  type: EncodedVideoChunkType
  timestamp: number
  descriptionBase64?: string
  data: Uint8Array
}

export function buildVideoChunkFromLoc(loc: LocMetadata, payload: Uint8Array, objectId: bigint): VideoChunk {
  return {
    type: objectId === 0n ? 'key' : 'delta',
    timestamp: loc.captureTimestampMicros ?? 0,
    descriptionBase64: loc.videoConfig?.toBase64(),
    data: payload
  }
}
